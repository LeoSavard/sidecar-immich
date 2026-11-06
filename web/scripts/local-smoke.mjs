// SPDX-License-Identifier: Apache-2.0
// Texture and Clarity end to end (DESIGN.md §7, "Local contrast"): Clarity raised in the editor, and the picture
// checked in both renderers — the browser's canvas and the worker's `/preview` — for more local contrast at the
// same overall brightness; then Clarity and Texture lowered for less; and the saved edit for its `presence`.
//
//   node web/scripts/local-smoke.mjs <base-url> <photo path on the server>
//
// Run inside the dev container with --network host, against `./dev/serve`. The photo's edit is reset first.
import { chromium } from 'playwright';

const [base, photo] = process.argv.slice(2);
if (!base || !photo) throw new Error('usage: local-smoke.mjs <base-url> <photo path>');
const browser = await chromium.launch({ headless: true, args: ['--no-sandbox', '--enable-unsafe-webgpu', '--enable-features=Vulkan', '--use-vulkan=native', '--use-angle=vulkan', '--disable-vulkan-surface', '--ignore-gpu-blocklist'] });
const steps = [];
const check = (ok, what) => {
	if (!ok) throw new Error(`failed: ${what}`);
	steps.push(what);
};

/** Mean luminance, and local contrast: the mean absolute difference between each pixel's log luminance and its
 * 9 × 9 neighbourhood's mean, over the middle of a PNG or JPEG, decoded in the page. */
async function measure(page, bytes, type) {
	return page.evaluate(
		async ({ b64, type }) => {
			const blob = await (await fetch(`data:${type};base64,${b64}`)).blob();
			const bmp = await createImageBitmap(blob);
			const c = new OffscreenCanvas(bmp.width, bmp.height).getContext('2d');
			c.drawImage(bmp, 0, 0);
			const { width: w, height: h } = bmp;
			const d = c.getImageData(0, 0, w, h).data;
			const lum = new Float32Array(w * h);
			for (let i = 0; i < w * h; i++) lum[i] = Math.log2(1 + 0.2126 * d[i * 4] + 0.7152 * d[i * 4 + 1] + 0.0722 * d[i * 4 + 2]);
			let mean = 0;
			let contrast = 0;
			let n = 0;
			for (let y = Math.floor(h * 0.2); y < Math.floor(h * 0.8); y += 3)
				for (let x = Math.floor(w * 0.2); x < Math.floor(w * 0.8); x += 3) {
					let s = 0;
					for (let j = -4; j <= 4; j++) for (let i = -4; i <= 4; i++) s += lum[(y + j) * w + x + i];
					contrast += Math.abs(lum[y * w + x] - s / 81);
					mean += lum[y * w + x];
					n++;
				}
			return { mean: mean / n, contrast: contrast / n };
		},
		{ b64: Buffer.from(bytes).toString('base64'), type }
	);
}

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
	const canvas = page.locator('canvas').first();
	await page.waitForTimeout(1500);
	const before = await measure(page, await canvas.screenshot(), 'image/png');
	const preview = async (edit) => {
		const r = await page.request.post(`${base}/api/assets/${key}/preview`, { data: { edit, colorSpace: 'srgb', longEdge: 1200 } });
		if (!r.ok()) throw new Error(`preview: ${r.status()} ${await r.text()}`);
		return measure(page, await r.body(), 'image/jpeg');
	};
	const original = (await (await page.request.get(`${base}/api/assets/${key}/edit`)).json()).edit;
	const workerBefore = await preview(original);

	const set = async (id, value) => {
		const label = page.locator(`#${id}-label`).first();
		await label.evaluate((s) => {
			for (let d = s.closest('details'); d; d = d.parentElement?.closest('details') ?? null) if (!d.open) d.open = true;
		});
		await label.scrollIntoViewIfNeeded();
		await label.locator('xpath=following-sibling::button').click();
		const input = label.locator('xpath=following-sibling::input');
		await input.fill(String(value));
		await input.press('Enter');
		await page.waitForTimeout(1500);
	};
	await set('clarity', 80);
	const after = await measure(page, await canvas.screenshot(), 'image/png');
	console.log('browser', JSON.stringify({ before, after }));
	check(after.contrast > before.contrast * 1.1, `Clarity +80 raises the browser's local contrast (${before.contrast.toFixed(4)} → ${after.contrast.toFixed(4)})`);
	check(Math.abs(after.mean - before.mean) < 0.15, `at about the same brightness (${before.mean.toFixed(3)} → ${after.mean.toFixed(3)} log₂)`);

	await page.waitForTimeout(5500);
	const saved = (await (await page.request.get(`${base}/api/assets/${key}/edit`)).json()).edit;
	check(saved.presence?.clarity === 80, `the saved edit holds it (${JSON.stringify(saved.presence)})`);
	const workerAfter = await preview(saved);
	console.log('worker', JSON.stringify({ workerBefore, workerAfter }));
	check(workerAfter.contrast > workerBefore.contrast * 1.1, `the worker's preview agrees (${workerBefore.contrast.toFixed(4)} → ${workerAfter.contrast.toFixed(4)})`);

	await set('clarity', -80);
	await set('texture', -80);
	const softer = await measure(page, await canvas.screenshot(), 'image/png');
	console.log('softer', JSON.stringify(softer));
	check(softer.contrast < before.contrast * 0.9, `Clarity and Texture at −80 lower it (${before.contrast.toFixed(4)} → ${softer.contrast.toFixed(4)})`);
	await set('clarity', 0);
	await set('texture', 0);
	await page.waitForTimeout(5500);
	const rest = (await (await page.request.get(`${base}/api/assets/${key}/edit`)).json()).edit;
	check(rest.presence === undefined, `both back at 0 leaves no presence in the edit (${JSON.stringify(rest.presence)})`);
	console.log(steps.map((s) => `ok  ${s}`).join('\n'));
} finally {
	await browser.close();
}
