// SPDX-License-Identifier: Apache-2.0
// Crop, straighten and mask-layer smoke test in headless Chromium (WebGPU on the server GPU): straighten, drag
// the crop, turn, paint a brush mask, lift it a stop, save — then check what was saved, and that the worker's
// render of it has the output's shape and the stop where the stroke was.
// Usage: node scripts/tools-smoke.mjs <base> <assetKey> <outDir>
import { chromium } from 'playwright';

const [base, key, outDir] = process.argv.slice(2);
const browser = await chromium.launch({
	headless: true,
	args: ['--no-sandbox', '--enable-unsafe-webgpu', '--enable-features=Vulkan', '--use-vulkan=native', '--use-angle=vulkan', '--disable-vulkan-surface', '--ignore-gpu-blocklist']
});
const failures = [];
const check = (ok, what) => {
	console.log(`${ok ? 'ok  ' : 'FAIL'} ${what}`);
	if (!ok) failures.push(what);
};

const page = await (await browser.newContext({ viewport: { width: 1400, height: 900 } })).newPage();
page.on('pageerror', (e) => failures.push(`pageerror: ${e.message}`));
page.on('console', (m) => (m.type() === 'error' || m.text().includes('WebGPU')) && console.log(`console.${m.type()}: ${m.text()}`));
// Start from the photo's edit without geometry or masks, saved as a version of its own.
await page.goto(`${base}/`, { waitUntil: 'networkidle' });
await page.evaluate(async (k) => {
	const s = await (await fetch(`/api/assets/${k}/edit`)).json();
	delete s.edit.geometry;
	delete s.edit.masks;
	await fetch(`/api/assets/${k}/edit`, { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ edit: s.edit, base: s.head, checkpoint: false, session: 'tools-smoke' }) });
}, key);
await page.goto(`${base}/edit/${key}`, { waitUntil: 'networkidle' });
await page.locator('#exposure:visible').waitFor({ timeout: 60_000 });
await page.waitForTimeout(2500);
const shot = (name) => page.screenshot({ path: `${outDir}/tools-${name}.png` });

// ---- crop and straighten
await page.locator('summary', { hasText: 'Crop and straighten' }).click();
await page.getByRole('slider', { name: 'Crop nw edge' }).waitFor();
await shot('crop-open');
await page.locator('#geo-angle').focus();
for (let i = 0; i < 5; i++) await page.keyboard.press('Shift+ArrowRight');
await page.waitForTimeout(400);
await shot('crop-straightened');
const nw = await page.getByRole('slider', { name: 'Crop nw edge' }).boundingBox();
await page.mouse.move(nw.x + nw.width / 2, nw.y + nw.height / 2);
await page.mouse.down();
await page.mouse.move(nw.x + 120, nw.y + 90, { steps: 10 });
await page.mouse.up();
await page.waitForTimeout(300);
await shot('crop-dragged');
await page.getByRole('button', { name: '⟳ Right' }).click();
await page.waitForTimeout(400);
await shot('crop-turned');
await page.locator('summary', { hasText: 'Crop and straighten' }).click();
await page.waitForTimeout(600);
await shot('cropped');

// ---- a brush mask, lifted a stop
await page.locator('summary', { hasText: 'Masks' }).click();
await page.getByRole('button', { name: 'Brush', exact: true }).click();
const canvas = await page.locator('canvas').first().boundingBox();
const y = canvas.y + canvas.height * 0.5;
await page.mouse.move(canvas.x + canvas.width * 0.35, y);
await page.mouse.down();
await page.mouse.move(canvas.x + canvas.width * 0.65, y, { steps: 30 });
await page.mouse.up();
await page.locator('#mask-exposure').focus();
for (let i = 0; i < 10; i++) await page.keyboard.press('Shift+ArrowRight');
await page.waitForTimeout(600);
await shot('brushed');
await page.getByRole('radio', { name: 'Off' }).click();
await page.getByRole('button', { name: 'Show', exact: true }).click();
await page.waitForTimeout(400);
await shot('brushed-plain');

await page.getByRole('button', { name: /^(Save|Saved|Saving…)$/ }).click();
await page.waitForTimeout(1500);

// ---- what was saved
const state = await page.evaluate(async (k) => (await fetch(`/api/assets/${k}/edit`)).json(), key);
const g = state.edit.geometry;
check(g && Math.abs(g.angle - 5) < 1e-6, `straighten saved as 5° (${g?.angle})`);
check(g && g.rotate === 1, `turned once (${g?.rotate})`);
const c = g?.crop;
check(c && c.left > 0 && c.top > 0 && c.right <= 1 && c.bottom <= 1 && c.left < c.right && c.top < c.bottom, `crop inside the frame ${JSON.stringify(c)}`);
const layer = state.edit.masks?.[0];
check(layer?.mask === 'brush' && layer.strokes?.length === 1, `one brush stroke on a brush layer`);
const points = layer?.strokes?.[0]?.points?.length ?? 0;
check(points >= 2 && points <= 40, `stroke simplified to ${points} points`);
check(Math.abs((layer?.exposure ?? 0) - 1) < 1e-6, `layer exposure +1 (${layer?.exposure})`);

// ---- the worker's render of it: the output's shape, and the stop where the stroke is
const stats = await page.evaluate(
	async ({ k, edit }) => {
		const render = async (e) => {
			const r = await fetch(`/api/assets/${k}/preview`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ edit: e, colorSpace: 'srgb', longEdge: 1024 }) });
			const bmp = await createImageBitmap(await r.blob());
			const ctx = new OffscreenCanvas(bmp.width, bmp.height).getContext('2d');
			ctx.drawImage(bmp, 0, 0);
			return { w: bmp.width, h: bmp.height, data: ctx.getImageData(0, 0, bmp.width, bmp.height).data };
		};
		const lit = await render(edit);
		const flat = await render({ ...edit, masks: edit.masks.map((m) => ({ ...m, exposure: 0 })) });
		const mean = (img, x0, y0, x1, y1) => {
			let s = 0;
			let n = 0;
			for (let y = Math.floor(y0 * img.h); y < y1 * img.h; y++)
				for (let x = Math.floor(x0 * img.w); x < x1 * img.w; x++) {
					const i = (y * img.w + x) * 4;
					s += img.data[i] + img.data[i + 1] + img.data[i + 2];
					n += 3;
				}
			return s / n;
		};
		const meta = await (await fetch(`/api/assets/${k}/meta`)).json();
		return {
			size: [lit.w, lit.h],
			picture: [meta.info.width, meta.info.height],
			centre: [mean(flat, 0.45, 0.49, 0.55, 0.51), mean(lit, 0.45, 0.49, 0.55, 0.51)],
			corner: [mean(flat, 0.02, 0.02, 0.12, 0.12), mean(lit, 0.02, 0.02, 0.12, 0.12)]
		};
	},
	{ k: key, edit: state.edit }
);
const want = ((c.bottom - c.top) * stats.picture[1]) / ((c.right - c.left) * stats.picture[0]);
const got = stats.size[0] / stats.size[1];
check(Math.abs(got / want - 1) < 0.02, `worker output ${stats.size.join('×')}: width ÷ height ${got.toFixed(3)}, crop turned says ${want.toFixed(3)}`);
check(stats.centre[1] > stats.centre[0] + 8, `stroke lifted in the worker's render: ${stats.centre.map((v) => v.toFixed(1)).join(' → ')}`);
check(Math.abs(stats.corner[1] - stats.corner[0]) < 1.5, `away from the stroke unchanged: ${stats.corner.map((v) => v.toFixed(1)).join(' → ')}`);

const loupe = await page.evaluate(async ({ k, edit }) => {
	const r = await fetch(`/api/assets/${k}/loupe`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ center: [0.5, 0.5], size: [512, 512], edit, colorSpace: 'srgb' }) });
	return r.ok ? `${r.status} ${(await r.blob()).size} bytes` : `${r.status} ${await r.text()}`;
}, { k: key, edit: state.edit });
check(loupe.startsWith('200'), `loupe through the geometry: ${loupe}`);

console.log(failures.length ? `\n${failures.length} failed:\n${failures.join('\n')}` : '\nall passed');
await browser.close();
process.exit(failures.length ? 1 : 0);
