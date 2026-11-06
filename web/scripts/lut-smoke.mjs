// SPDX-License-Identifier: Apache-2.0
// 3D LUTs end to end (DESIGN.md §15, "3D LUTs"): Film → LUT → Upload .cube on a photo, with a table that swaps red
// and blue — unmistakable, and exact through an sRGB LUT — then the edit saved naming it, the browser's canvas
// and the worker's `/preview` both swapped, Amount 0 back to the photo, None leaving no LUT in the edit; the
// test's LUT is deleted from the server at the end.
//
//   node web/scripts/lut-smoke.mjs <base-url> <photo path on the server>
//
// Run inside the dev container with --network host, against `./dev/serve`. The photo's edit is reset first.
import { chromium } from 'playwright';

const [base, photo] = process.argv.slice(2);
if (!base || !photo) throw new Error('usage: lut-smoke.mjs <base-url> <photo path>');
const browser = await chromium.launch({ headless: true, args: ['--no-sandbox', '--enable-unsafe-webgpu', '--enable-features=Vulkan', '--use-vulkan=native', '--use-angle=vulkan', '--disable-vulkan-surface', '--ignore-gpu-blocklist'] });
const steps = [];
const check = (ok, what) => {
	if (!ok) throw new Error(`failed: ${what}`);
	steps.push(what);
};

/** A 9³ table that swaps red and blue. */
function swapCube() {
	const n = 9;
	let s = 'TITLE "swap red and blue"\nLUT_3D_SIZE 9\n';
	for (let b = 0; b < n; b++) for (let g = 0; g < n; g++) for (let r = 0; r < n; r++) s += `${b / (n - 1)} ${g / (n - 1)} ${r / (n - 1)}\n`;
	return s;
}

/** Mean red, green and blue (0…255) over an image's middle. */
async function means(page, bytes, type) {
	return page.evaluate(
		async ({ b64, type }) => {
			const bmp = await createImageBitmap(await (await fetch(`data:${type};base64,${b64}`)).blob());
			const c = new OffscreenCanvas(64, 64).getContext('2d');
			c.drawImage(bmp, bmp.width * 0.2, bmp.height * 0.2, bmp.width * 0.6, bmp.height * 0.6, 0, 0, 64, 64);
			const d = c.getImageData(0, 0, 64, 64).data;
			const m = [0, 0, 0];
			for (let i = 0; i < d.length; i += 4) for (let k = 0; k < 3; k++) m[k] += d[i + k] / (64 * 64);
			return m;
		},
		{ b64: Buffer.from(bytes).toString('base64'), type }
	);
}
const fmt = (m) => m.map((v) => v.toFixed(1)).join(' ');

let lutId;
try {
	const page = await browser.newPage({ viewport: { width: 1400, height: 1000 }, deviceScaleFactor: 1 });
	page.on('pageerror', (e) => console.error('[pageerror]', e.message));
	const open = async () => {
		await page.goto(`${base}/open?path=${encodeURIComponent(photo)}`, { waitUntil: 'load' });
		await page.getByRole('slider', { name: 'Exposure' }).first().waitFor({ state: 'attached', timeout: 60000 });
		return page.url().split('/edit/')[1];
	};
	let key = await open();
	await page.request.post(`${base}/api/assets/${key}/history/checkout`, { data: { node: 'original' } });
	key = await open();
	await page.waitForTimeout(1500);
	const canvas = page.locator('canvas').first();
	const before = await means(page, await canvas.screenshot(), 'image/png');

	const upload = page.locator('input[type=file][accept=".cube"]').first();
	await upload.evaluate((el) => {
		for (let d = el.closest('details'); d; d = d.parentElement?.closest('details') ?? null) if (!d.open) d.open = true;
	});
	await upload.setInputFiles({ name: 'swap-red-blue.cube', mimeType: 'text/plain', buffer: Buffer.from(swapCube()) });
	await page.getByRole('slider', { name: 'Amount' }).last().waitFor({ state: 'attached', timeout: 20000 });
	await page.waitForTimeout(2000);
	const after = await means(page, await canvas.screenshot(), 'image/png');
	console.log('canvas', fmt(before), '→', fmt(after));
	check(Math.abs(after[0] - before[2]) < 3 && Math.abs(after[2] - before[0]) < 3 && Math.abs(after[1] - before[1]) < 3, 'the canvas has red and blue swapped');
	check(Math.abs(before[0] - before[2]) > 3, `on a photo where that shows (red ${before[0].toFixed(1)}, blue ${before[2].toFixed(1)})`);

	await page.waitForTimeout(5500);
	const saved = (await (await page.request.get(`${base}/api/assets/${key}/edit`)).json()).edit;
	console.log('edit', JSON.stringify(saved.lut));
	lutId = saved.lut?.id;
	check(/^[0-9a-f]{16}$/.test(saved.lut?.id ?? '') && saved.lut.name === 'swap-red-blue' && saved.lut.amount === 100 && saved.lut.space === 'srgb', 'the edit names the LUT, at 100 % in sRGB');
	const listed = await (await page.request.get(`${base}/api/luts`)).json();
	check(listed.some((l) => l.id === lutId && l.size === 9 && l.title === 'swap red and blue'), 'the server lists it with its size and title');

	const preview = async (edit) => (await page.request.post(`${base}/api/assets/${key}/preview`, { data: { edit, colorSpace: 'srgb', longEdge: 800 } })).body();
	const plain = { ...saved };
	delete plain.lut;
	const [wa, wb] = [await means(page, await preview(plain), 'image/jpeg'), await means(page, await preview(saved), 'image/jpeg')];
	console.log('worker', fmt(wa), '→', fmt(wb));
	check(Math.abs(wb[0] - wa[2]) < 3 && Math.abs(wb[2] - wa[0]) < 3, "the worker's preview has them swapped too");

	const label = page.locator('#lut-amount-label').first();
	await label.scrollIntoViewIfNeeded();
	await label.locator('xpath=following-sibling::button').click();
	const input = label.locator('xpath=following-sibling::input');
	await input.fill('0');
	await input.press('Enter');
	await page.waitForTimeout(1500);
	const none = await means(page, await canvas.screenshot(), 'image/png');
	check([0, 1, 2].every((k) => Math.abs(none[k] - before[k]) < 1.5), `Amount 0 is the photo again (${fmt(none)})`);

	await page.locator('#lut-choice').selectOption('');
	await page.waitForTimeout(5500);
	const off = (await (await page.request.get(`${base}/api/assets/${key}/edit`)).json()).edit;
	check(!off.lut, 'None leaves no LUT in the edit');
	console.log(steps.map((s) => `ok  ${s}`).join('\n'));
} finally {
	if (lutId) await fetch(`${base}/api/luts/${lutId}`, { method: 'DELETE' }).catch(() => {});
	await browser.close();
}
