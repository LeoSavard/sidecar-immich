// SPDX-License-Identifier: Apache-2.0
// Gradient layers end to end (DESIGN.md §15, "Gradients"): a linear gradient added in the editor, darkened two
// stops, its handle dragged, saved — and the picture checked in both renderers: the browser's canvas and the
// worker's `/preview`, each darker at the top than before and unchanged at the bottom.
//
//   node web/scripts/gradient-smoke.mjs <base-url> <photo path on the server>
//
// Run inside the dev container with --network host, against `./dev/serve`. The photo's edit is reset first.
import { chromium } from 'playwright';

const [base, photo] = process.argv.slice(2);
if (!base || !photo) throw new Error('usage: gradient-smoke.mjs <base-url> <photo path>');
const browser = await chromium.launch({ headless: true, args: ['--no-sandbox', '--enable-unsafe-webgpu', '--enable-features=Vulkan', '--use-vulkan=native', '--use-angle=vulkan', '--disable-vulkan-surface', '--ignore-gpu-blocklist'] });
const steps = [];
const check = (ok, what) => {
	if (!ok) throw new Error(`failed: ${what}`);
	steps.push(what);
};

/** Mean luminance of a PNG or JPEG's top and bottom fifths, decoded in the page. */
async function bands(page, bytes, type) {
	return page.evaluate(
		async ({ b64, type }) => {
			const blob = await (await fetch(`data:${type};base64,${b64}`)).blob();
			const bmp = await createImageBitmap(blob);
			const c = new OffscreenCanvas(bmp.width, bmp.height).getContext('2d');
			c.drawImage(bmp, 0, 0);
			const d = c.getImageData(0, 0, bmp.width, bmp.height).data;
			const mean = (y0, y1) => {
				let s = 0;
				let n = 0;
				for (let y = Math.floor(y0 * bmp.height); y < Math.floor(y1 * bmp.height); y++)
					for (let x = 0; x < bmp.width; x += 2) {
						const i = (y * bmp.width + x) * 4;
						s += 0.2126 * d[i] + 0.7152 * d[i + 1] + 0.0722 * d[i + 2];
						n++;
					}
				return s / n;
			};
			return { top: mean(0.02, 0.18), bottom: mean(0.82, 0.98) };
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
	const before = await bands(page, await canvas.screenshot(), 'image/png');
	const preview = async (edit) => {
		const r = await page.request.post(`${base}/api/assets/${key}/preview`, { data: { edit, colorSpace: 'srgb', longEdge: 800 } });
		if (!r.ok()) throw new Error(`preview: ${r.status()} ${await r.text()}`);
		return bands(page, await r.body(), 'image/jpeg');
	};
	const original = (await (await page.request.get(`${base}/api/assets/${key}/edit`)).json()).edit;
	const workerBefore = await preview(original);

	// The Masks section, a linear gradient, two stops down.
	const summary = page.locator('summary').filter({ hasText: /^Masks$/ }).last();
	await summary.evaluate((s) => {
		for (let d = s.parentElement; d; d = d.parentElement?.closest('details') ?? null) if (!d.open) d.open = true;
	});
	await summary.scrollIntoViewIfNeeded();
	await page.getByRole('button', { name: 'Linear gradient', exact: true }).click();
	check(await page.getByRole('group', { name: 'Gradient handles' }).isVisible(), 'adding a linear gradient shows its handles');
	await page.locator('#mask-exposure-label').locator('xpath=following-sibling::button').click();
	const input = page.locator('#mask-exposure-label').locator('xpath=following-sibling::input');
	await input.fill('-2');
	await input.press('Enter');
	// A new layer shows its mask (greying out what it leaves out): off, to see the picture itself.
	const show = page.getByRole('button', { name: 'Show', exact: true }).first();
	if ((await show.getAttribute('aria-pressed')) === 'true') await show.click();
	await page.waitForTimeout(1500);
	const after = await bands(page, await canvas.screenshot(), 'image/png');
	console.log('browser', JSON.stringify({ before, after }));
	// The canvas holds the photo with margins around it, so the browser's drop is diluted; the worker's below is exact.
	check(after.top < before.top * 0.8, `the browser darkens the top (${before.top.toFixed(1)} → ${after.top.toFixed(1)})`);
	check(Math.abs(after.bottom - before.bottom) < 2, `and leaves the bottom (${before.bottom.toFixed(1)} → ${after.bottom.toFixed(1)})`);

	// Drag the "no effect" handle down: the gradient's end moves with it.
	const end = page.getByRole('button', { name: 'No effect' });
	const box = await end.boundingBox();
	await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
	await page.mouse.down();
	await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2 + 120, { steps: 8 });
	await page.mouse.up();
	const saveButton = page.getByRole('button', { name: 'Save' }).first();
	if (await saveButton.isVisible()) await saveButton.click();
	await page.waitForFunction(() => [...document.querySelectorAll('button')].some((b) => b.textContent?.trim() === 'Saved'), null, { timeout: 15000 });
	const saved = (await (await page.request.get(`${base}/api/assets/${key}/edit`)).json()).edit;
	const g = saved.masks?.[0]?.gradient;
	console.log('saved', JSON.stringify(saved.masks?.[0]));
	check(saved.masks?.[0]?.mask === 'gradient' && g?.kind === 'linear', 'the saved edit holds a linear gradient layer');
	check(g.to[1] > 0.58, `dragging the handle moved the gradient's end (to ${g.to.map((v) => v.toFixed(3))})`);

	// The worker renders the same layer. Two stops on a sky partly above white land in the tone curve's shoulder,
	// so the drop in display values is smaller than two stops would be lower down.
	const workerAfter = await preview(saved);
	console.log('worker', JSON.stringify({ workerBefore, workerAfter }));
	check(workerAfter.top < workerBefore.top * 0.8, `the worker darkens the top (${workerBefore.top.toFixed(1)} → ${workerAfter.top.toFixed(1)})`);
	check(Math.abs(workerAfter.bottom - workerBefore.bottom) < 2, `and leaves the bottom (${workerBefore.bottom.toFixed(1)} → ${workerAfter.bottom.toFixed(1)})`);
	console.log(steps.map((s) => '✓ ' + s).join('\n'));
} finally {
	await browser.close();
}
