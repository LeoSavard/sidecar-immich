// SPDX-License-Identifier: Apache-2.0
// Soft proofing end to end (DESIGN.md §15, "Soft proofing"): Proof on a photo, a printer profile uploaded, and
// what follows — the canvas proofed (duller, its blacks lifted to the print's), the gamut warning greying what
// the print can't hold and gone when turned off, Paper and ink dimming the paper's white, the edit untouched
// throughout, and the photo as it was once the proof is off. The profile is deleted from the server at the end.
//
//   node web/scripts/proof-smoke.mjs <base-url> <photo path on the server> <printer profile .icc>
//
// Run inside the dev container with --network host, against `./dev/serve`. The photo's edit is reset first. Real
// printer profiles are licensed, so the profile is the caller's: ghostscript's `default_cmyk.icc` (SWOP) will do.
import { readFileSync } from 'node:fs';
import { basename } from 'node:path';
import { chromium } from 'playwright';

const [base, photo, icc] = process.argv.slice(2);
if (!base || !photo || !icc) throw new Error('usage: proof-smoke.mjs <base-url> <photo path> <profile.icc>');
const browser = await chromium.launch({ headless: true, args: ['--no-sandbox', '--enable-unsafe-webgpu', '--enable-features=Vulkan', '--use-vulkan=native', '--use-angle=vulkan', '--disable-vulkan-surface', '--ignore-gpu-blocklist'] });
const steps = [];
const check = (ok, what) => {
	if (!ok) throw new Error(`failed: ${what}`);
	steps.push(what);
};

/** Over an image's middle: mean chroma (max − min of RGB), the darkest 1 % of luminance, the brightest 1 %, and the share of pixels that are exactly the warning's mid grey. */
async function stats(page, bytes) {
	return page.evaluate(
		async (b64) => {
			const bmp = await createImageBitmap(await (await fetch(`data:image/png;base64,${b64}`)).blob());
			const w = 160, h = 120;
			const c = new OffscreenCanvas(w, h).getContext('2d');
			c.imageSmoothingEnabled = false;
			c.drawImage(bmp, bmp.width * 0.2, bmp.height * 0.2, bmp.width * 0.6, bmp.height * 0.6, 0, 0, w, h);
			const d = c.getImageData(0, 0, w, h).data;
			const lum = [];
			let [chroma, grey] = [0, 0];
			for (let i = 0; i < d.length; i += 4) {
				const [r, g, b] = [d[i], d[i + 1], d[i + 2]];
				chroma += Math.max(r, g, b) - Math.min(r, g, b);
				lum.push(0.2126 * r + 0.7152 * g + 0.0722 * b);
				if (Math.abs(r - 128) <= 1 && Math.abs(g - 128) <= 1 && Math.abs(b - 128) <= 1) grey++;
			}
			lum.sort((a, b) => a - b);
			const n = lum.length;
			return { chroma: chroma / n, dark: lum[Math.floor(n * 0.01)], bright: lum[Math.floor(n * 0.99)], grey: grey / n };
		},
		Buffer.from(bytes).toString('base64')
	);
}
const fmt = (s) => `chroma ${s.chroma.toFixed(1)}, dark ${s.dark.toFixed(1)}, bright ${s.bright.toFixed(1)}, warning ${(s.grey * 100).toFixed(1)} %`;

let proofId;
try {
	const page = await browser.newPage({ viewport: { width: 1400, height: 1000 }, deviceScaleFactor: 1 });
	page.on('pageerror', (e) => console.error('[pageerror]', e.message));
	await page.addInitScript(() => localStorage.removeItem('sidecar.proof'));
	const open = async () => {
		await page.goto(`${base}/open?path=${encodeURIComponent(photo)}`, { waitUntil: 'load' });
		await page.getByRole('slider', { name: 'Exposure' }).first().waitFor({ state: 'attached', timeout: 60000 });
		return page.url().split('/edit/')[1];
	};
	let key = await open();
	await page.request.post(`${base}/api/assets/${key}/history/checkout`, { data: { node: 'original' } });
	key = await open();
	await page.waitForTimeout(1500);
	const editBefore = (await (await page.request.get(`${base}/api/assets/${key}/edit`)).json()).edit;
	const canvas = page.locator('canvas').first();
	const shot = async () => {
		await page.waitForTimeout(1500);
		return stats(page, await canvas.screenshot());
	};
	const before = await shot();
	console.log('before   ', fmt(before));

	await page.getByRole('button', { name: 'Proof', exact: true }).click();
	await page.locator('input[type=file][accept=".icc,.icm"]').first().setInputFiles({ name: basename(icc), mimeType: 'application/vnd.iccprofile', buffer: readFileSync(icc) });
	await page.getByRole('radio', { name: 'Perceptual' }).waitFor({ timeout: 20000 });
	const warned = await shot();
	console.log('warning  ', fmt(warned));
	check(warned.grey > before.grey + 0.01, `the gamut warning greys what the print can't hold (${(warned.grey * 100).toFixed(1)} % of the picture)`);

	await page.getByRole('button', { name: 'Gamut warning' }).click();
	const proofed = await shot();
	console.log('proofed  ', fmt(proofed));
	check(proofed.grey < warned.grey, 'and is gone when turned off');
	check(proofed.chroma < before.chroma, `the proof is duller than the screen (chroma ${before.chroma.toFixed(1)} → ${proofed.chroma.toFixed(1)})`);
	check(proofed.dark > before.dark + 2, `its blacks lifted to the print's (${before.dark.toFixed(1)} → ${proofed.dark.toFixed(1)})`);

	await page.getByRole('button', { name: 'Paper and ink' }).click();
	const paper = await shot();
	console.log('paper    ', fmt(paper));
	check(paper.bright < proofed.bright - 3, `Paper and ink dims the whites to the paper's (${proofed.bright.toFixed(1)} → ${paper.bright.toFixed(1)})`);

	await page.waitForTimeout(5000);
	const editAfter = (await (await page.request.get(`${base}/api/assets/${key}/edit`)).json()).edit;
	check(JSON.stringify(editAfter) === JSON.stringify(editBefore), 'the edit is untouched: a proof is a view');
	proofId = (await (await page.request.get(`${base}/api/proofs`)).json()).find((p) => p.name === basename(icc).replace(/\.ic[cm]$/, ''))?.id;
	check(!!proofId, 'the server lists the profile');

	await page.getByRole('button', { name: 'Proof', exact: true }).click();
	const off = await shot();
	check(Math.abs(off.chroma - before.chroma) < 0.5 && Math.abs(off.dark - before.dark) < 1, `off again, the photo is as it was (${fmt(off)})`);
	console.log(steps.map((s) => `ok  ${s}`).join('\n'));
} finally {
	if (proofId) await fetch(`${base}/api/proofs/${proofId}`, { method: 'DELETE' }).catch(() => {});
	await browser.close();
}
