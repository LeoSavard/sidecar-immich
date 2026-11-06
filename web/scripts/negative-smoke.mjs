// SPDX-License-Identifier: Apache-2.0
// Film negatives end to end (DESIGN.md §7, "Negatives"): Film → Film negative on a photo, and what follows — the
// edit saved with a base and a white balance of its own, and the picture inverted in the browser's canvas and in
// the worker's `/preview` alike (where the photo was light it is now dark); then off again.
//
//   node web/scripts/negative-smoke.mjs <base-url> <photo path on the server>
//
// Run inside the dev container with --network host, against `./dev/serve`. The photo's edit is reset first.
import { chromium } from 'playwright';

const [base, photo] = process.argv.slice(2);
if (!base || !photo) throw new Error('usage: negative-smoke.mjs <base-url> <photo path>');
const browser = await chromium.launch({ headless: true, args: ['--no-sandbox', '--enable-unsafe-webgpu', '--enable-features=Vulkan', '--use-vulkan=native', '--use-angle=vulkan', '--disable-vulkan-surface', '--ignore-gpu-blocklist'] });
const steps = [];
const check = (ok, what) => {
	if (!ok) throw new Error(`failed: ${what}`);
	steps.push(what);
};

/** The correlation of two images' luminance over a grid of their middle (PNG or JPEG each). */
async function correlation(page, a, ta, b, tb) {
	return page.evaluate(
		async ({ a, ta, b, tb }) => {
			const load = async (b64, type) => {
				const bmp = await createImageBitmap(await (await fetch(`data:${type};base64,${b64}`)).blob());
				const c = new OffscreenCanvas(64, 64).getContext('2d');
				c.drawImage(bmp, bmp.width * 0.2, bmp.height * 0.2, bmp.width * 0.6, bmp.height * 0.6, 0, 0, 64, 64);
				const d = c.getImageData(0, 0, 64, 64).data;
				return Array.from({ length: 64 * 64 }, (_, i) => 0.2126 * d[i * 4] + 0.7152 * d[i * 4 + 1] + 0.0722 * d[i * 4 + 2]);
			};
			const [x, y] = [await load(a, ta), await load(b, tb)];
			const mean = (v) => v.reduce((s, t) => s + t, 0) / v.length;
			const [mx, my] = [mean(x), mean(y)];
			let [sxy, sxx, syy] = [0, 0, 0];
			for (let i = 0; i < x.length; i++) [sxy, sxx, syy] = [sxy + (x[i] - mx) * (y[i] - my), sxx + (x[i] - mx) ** 2, syy + (y[i] - my) ** 2];
			return sxy / Math.sqrt(sxx * syy);
		},
		{ a: Buffer.from(a).toString('base64'), ta, b: Buffer.from(b).toString('base64'), tb }
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
	await page.waitForTimeout(1500);
	const canvas = page.locator('canvas').first();
	const before = await canvas.screenshot();
	const summary = page.locator('summary').filter({ hasText: /^Film$/ }).last();
	await summary.evaluate((s) => {
		for (let d = s.parentElement; d; d = d.parentElement?.closest('details') ?? null) if (!d.open) d.open = true;
	});
	const toggle = page.getByRole('button', { name: 'Film negative', exact: true }).last();
	await toggle.scrollIntoViewIfNeeded();
	await toggle.click();
	await page.waitForTimeout(6000);
	const saved = (await (await page.request.get(`${base}/api/assets/${key}/edit`)).json()).edit;
	console.log('edit', JSON.stringify({ negative: saved.negative, whiteBalance: saved.whiteBalance }));
	check(saved.negative?.base?.length === 3 && saved.negative.gamma === 0.6, 'Film negative saves a base and γ 0.6');
	check(saved.whiteBalance.mode === 'custom', 'and a white balance for the inverted picture');
	const after = await canvas.screenshot();
	const c = await correlation(page, before, 'image/png', after, 'image/png');
	console.log('canvas correlation', c.toFixed(3));
	check(c < -0.3, `the canvas is inverted: luminance correlation with before ${c.toFixed(2)}`);
	// The worker against itself (the canvas holds the photo with margins, so the two don't frame alike).
	const preview = async (edit) => (await page.request.post(`${base}/api/assets/${key}/preview`, { data: { edit, colorSpace: 'srgb', longEdge: 800 } })).body();
	const positive = { ...saved, whiteBalance: { ...saved.whiteBalance, mode: 'asShot' } };
	delete positive.negative;
	const w = await correlation(page, await preview(positive), 'image/jpeg', await preview(saved), 'image/jpeg');
	console.log('worker correlation', w.toFixed(3));
	check(w < -0.3, `the worker's preview is inverted too (correlation ${w.toFixed(2)})`);
	await toggle.click();
	await page.waitForTimeout(5500);
	const off = (await (await page.request.get(`${base}/api/assets/${key}/edit`)).json()).edit;
	check(!off.negative && off.whiteBalance.mode === 'asShot', 'off again leaves a positive, as shot');
	console.log(steps.map((s) => `ok  ${s}`).join('\n'));
} finally {
	await browser.close();
}
