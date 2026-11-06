// SPDX-License-Identifier: Apache-2.0
// Upright end to end (DESIGN.md §15, "Upright"): Crop → Upright → Vertical in the editor, the geometry it saves,
// and the worker's render of it — its near-vertical edges leaning less than the photo's did.
//
//   node web/scripts/upright-smoke.mjs <base-url> <photo path on the server> [none]
//
// With `none`, the photo is one without straight lines: the editor says so and the geometry stays as it was.
//
// Run inside the dev container with --network host, against `./dev/serve`. The photo's edit is reset first.
import { chromium } from 'playwright';

const [base, photo, expect] = process.argv.slice(2);
if (!base || !photo) throw new Error('usage: upright-smoke.mjs <base-url> <photo path>');
const browser = await chromium.launch({ headless: true, args: ['--no-sandbox', '--enable-unsafe-webgpu', '--enable-features=Vulkan', '--use-vulkan=native', '--use-angle=vulkan', '--disable-vulkan-surface', '--ignore-gpu-blocklist'] });
const steps = [];
const check = (ok, what) => {
	if (!ok) throw new Error(`failed: ${what}`);
	steps.push(what);
};

/** How far the picture's near-vertical edges lean, on average: the Sobel gradient's angle from horizontal, where it
 * is within 20° of it, weighted by its strength, in degrees. */
async function lean(page, bytes) {
	return page.evaluate(async (b64) => {
		const bmp = await createImageBitmap(await (await fetch(`data:image/jpeg;base64,${b64}`)).blob());
		const c = new OffscreenCanvas(bmp.width, bmp.height).getContext('2d');
		c.drawImage(bmp, 0, 0);
		const { width: w, height: h } = bmp;
		const d = c.getImageData(0, 0, w, h).data;
		const y = (x, yy) => Math.log2(1 + 0.2126 * d[(yy * w + x) * 4] + 0.7152 * d[(yy * w + x) * 4 + 1] + 0.0722 * d[(yy * w + x) * 4 + 2]);
		let s = 0;
		let n = 0;
		for (let yy = 1; yy < h - 1; yy++)
			for (let x = 1; x < w - 1; x++) {
				const gx = y(x + 1, yy - 1) + 2 * y(x + 1, yy) + y(x + 1, yy + 1) - y(x - 1, yy - 1) - 2 * y(x - 1, yy) - y(x - 1, yy + 1);
				const gy = y(x - 1, yy + 1) + 2 * y(x, yy + 1) + y(x + 1, yy + 1) - y(x - 1, yy - 1) - 2 * y(x, yy - 1) - y(x + 1, yy - 1);
				const m = Math.hypot(gx, gy);
				if (m < 1) continue;
				const a = (Math.atan2(gy, gx) * 180) / Math.PI;
				const off = Math.min(Math.abs(a), 180 - Math.abs(a));
				if (off > 20) continue;
				s += m * off;
				n += m;
			}
		return s / n;
	}, Buffer.from(bytes).toString('base64'));
}

try {
	const page = await browser.newPage({ viewport: { width: 1400, height: 1000 } });
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
	const preview = async (edit) => {
		const r = await page.request.post(`${base}/api/assets/${key}/preview`, { data: { edit, colorSpace: 'srgb', longEdge: 1200 } });
		if (!r.ok()) throw new Error(`preview: ${r.status()} ${await r.text()}`);
		return lean(page, await r.body());
	};
	const original = (await (await page.request.get(`${base}/api/assets/${key}/edit`)).json()).edit;
	const before = await preview(original);

	const summary = page.locator('summary').filter({ hasText: /^Crop and straighten$/ }).last();
	await summary.evaluate((s) => {
		for (let d = s.parentElement; d; d = d.parentElement?.closest('details') ?? null) if (!d.open) d.open = true;
	});
	const group = page.getByRole('group', { name: 'Upright' }).last();
	await group.scrollIntoViewIfNeeded();
	await group.getByRole('button', { name: 'Vertical', exact: true }).click();
	await page.waitForTimeout(5500);
	if (expect === 'none') {
		check(await page.getByText('Not enough straight lines in this photo for that.').last().isVisible(), 'a photo without lines says so');
		const kept = (await (await page.request.get(`${base}/api/assets/${key}/edit`)).json()).edit;
		check(!kept.geometry, 'and its geometry is left alone');
		console.log(steps.map((s) => `ok  ${s}`).join('\n'));
		process.exit(0);
	}
	const saved = (await (await page.request.get(`${base}/api/assets/${key}/edit`)).json()).edit;
	console.log('geometry', JSON.stringify(saved.geometry));
	check(saved.geometry && (saved.geometry.angle !== 0 || saved.geometry.vertical !== 0), `Upright → Vertical sets the geometry (${saved.geometry?.angle}°, vertical ${saved.geometry?.vertical})`);
	const after = await preview(saved);
	console.log('lean', JSON.stringify({ before, after }));
	check(after < before, `the near-vertical edges lean less (${before.toFixed(2)}° → ${after.toFixed(2)}°)`);
	console.log(steps.map((s) => `ok  ${s}`).join('\n'));
} finally {
	await browser.close();
}
