// SPDX-License-Identifier: Apache-2.0
// Colour ranges end to end (DESIGN.md §15, "Colour ranges"): Masks → Colour range on a photo, the layer's Exposure
// raised, and what brightens checked against what colour it was — skin hues for the default range, blues once the
// hue is moved there — in the browser's canvas and in the worker's preview alike; the range saved with the edit.
//
//   node web/scripts/color-range-smoke.mjs <base-url> <photo path on the server>
//
// Run inside the dev container with --network host, against `./dev/serve`. The photo's edit is reset first.
import { chromium } from 'playwright';

const [base, photo] = process.argv.slice(2);
if (!base || !photo) throw new Error('usage: color-range-smoke.mjs <base-url> <photo path>');
const browser = await chromium.launch({ headless: true, args: ['--no-sandbox', '--enable-unsafe-webgpu', '--enable-features=Vulkan', '--use-vulkan=native', '--use-angle=vulkan', '--disable-vulkan-surface', '--ignore-gpu-blocklist'] });
const steps = [];
const check = (ok, what) => {
	if (!ok) throw new Error(`failed: ${what}`);
	steps.push(what);
};

/**
 * Of the pixels that got brighter from `a` to `b` (same size), the share whose colour in `a` had an OKLab hue
 * within `[from, to]` degrees, and how many brightened at all.
 */
async function brightened(page, a, ta, b, tb, from, to) {
	return page.evaluate(
		async ({ a, ta, b, tb, from, to }) => {
			const load = async (b64, type) => {
				const bmp = await createImageBitmap(await (await fetch(`data:${type};base64,${b64}`)).blob());
				const c = new OffscreenCanvas(bmp.width, bmp.height).getContext('2d');
				c.drawImage(bmp, 0, 0);
				return c.getImageData(0, 0, bmp.width, bmp.height).data;
			};
			const [x, y] = [await load(a, ta), await load(b, tb)];
			const lin = (v) => ((v /= 255) <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);
			const hue = (r, g, bb) => {
				[r, g, bb] = [lin(r), lin(g), lin(bb)];
				const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * bb);
				const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * bb);
				const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * bb);
				const A = 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s;
				const B = 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s;
				return ((Math.atan2(B, A) * 180) / Math.PI + 360) % 360;
			};
			let [n, inside] = [0, 0];
			for (let i = 0; i < x.length; i += 4) {
				const before = x[i] + x[i + 1] + x[i + 2];
				if (y[i] + y[i + 1] + y[i + 2] - before < 30 || before < 30) continue;
				n++;
				const h = hue(x[i], x[i + 1], x[i + 2]);
				if (from <= to ? h >= from && h <= to : h >= from || h <= to) inside++;
			}
			return { n, share: n ? inside / n : 0 };
		},
		{ a: Buffer.from(a).toString('base64'), ta, b: Buffer.from(b).toString('base64'), tb, from, to }
	);
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
	const canvas = page.locator('canvas').first();
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
		await page.waitForTimeout(2500);
	};
	const summary = page.locator('summary').filter({ hasText: /^Masks$/ }).last();
	await summary.evaluate((s) => {
		for (let d = s.parentElement; d; d = d.parentElement?.closest('details') ?? null) if (!d.open) d.open = true;
	});
	const before = await canvas.screenshot();
	await page.getByRole('button', { name: 'Colour range', exact: true }).click();
	await set('mask-exposure', 1.5);
	const skinShot = await canvas.screenshot();
	const skin = await brightened(page, before, 'image/png', skinShot, 'image/png', 15, 95);
	console.log('skin range, canvas', JSON.stringify(skin));
	check(skin.n > 500 && skin.share > 0.85, `the default range brightens skin-coloured pixels: ${(skin.share * 100).toFixed(0)} % of ${skin.n} were warm hues`);

	await set('range-hue', 240);
	const blueShot = await canvas.screenshot();
	const blue = await brightened(page, before, 'image/png', blueShot, 'image/png', 195, 285);
	console.log('blue range, canvas', JSON.stringify(blue));
	check(blue.n > 200 && blue.share > 0.85, `moved to 240°, it brightens blues: ${(blue.share * 100).toFixed(0)} % of ${blue.n}`);

	await page.waitForTimeout(5500);
	const saved = (await (await page.request.get(`${base}/api/assets/${key}/edit`)).json()).edit;
	const layer = saved.masks?.[0];
	console.log('layer', JSON.stringify({ mask: layer?.mask, color: layer?.color, exposure: layer?.exposure }));
	check(layer?.mask === 'color' && layer.color?.hue === 240 && layer.exposure === 1.5, 'the edit saves the range with its layer');

	const preview = async (edit) => (await page.request.post(`${base}/api/assets/${key}/preview`, { data: { edit, colorSpace: 'srgb', longEdge: 1000 } })).body();
	const plain = { ...saved, masks: undefined };
	const w = await brightened(page, await preview(plain), 'image/jpeg', await preview(saved), 'image/jpeg', 195, 285);
	console.log('blue range, worker', JSON.stringify(w));
	check(w.n > 200 && w.share > 0.8, `the worker's preview brightens the same blues: ${(w.share * 100).toFixed(0)} % of ${w.n}`);
	console.log(steps.map((s) => `ok  ${s}`).join('\n'));
} finally {
	await browser.close();
}
