// SPDX-License-Identifier: Apache-2.0
// The mobile module end to end, at a phone's size and with touch: choose a JPEG, see it developed by the
// browser's own GPU, move Exposure and see the picture brighten, save a copy on the device, then send it to
// the server and check the edit made on the phone is the one the server stored. With --raw, also choose a RAW:
// its embedded preview is shown at once, then the phone decodes and develops the RAW itself, with nothing sent.
//
// Usage: node scripts/mobile-smoke.mjs <sidecar url> <photo.jpg> [--raw <file>] [--swiftshader] [--shots <dir>]
// (CHROMIUM_PATH to pick a browser)
// Needs a running Sidecar whose image (or `server.mobile_root`) has the module, and an editing folder or a
// library to import into.
import { chromium } from 'playwright';
import { createHash } from 'node:crypto';
import { readFileSync, mkdirSync } from 'node:fs';
import { join } from 'node:path';

const [url, photo] = process.argv.slice(2);
if (!url || !photo) throw new Error('usage: mobile-smoke.mjs <url> <photo.jpg> [--swiftshader] [--shots <dir>]');
const swiftshader = process.argv.includes('--swiftshader');
const shotsAt = process.argv.indexOf('--shots');
const shots = shotsAt > 0 ? process.argv[shotsAt + 1] : null;
const rawAt = process.argv.indexOf('--raw');
const raw = rawAt > 0 ? process.argv[rawAt + 1] : null;
if (shots) mkdirSync(shots, { recursive: true });

const gpuArgs = swiftshader
	? ['--enable-unsafe-webgpu', '--use-webgpu-adapter=swiftshader', '--enable-unsafe-swiftshader']
	: ['--enable-unsafe-webgpu', '--enable-features=Vulkan', '--use-vulkan=native', '--use-angle=vulkan', '--disable-vulkan-surface', '--ignore-gpu-blocklist'];
// CHROMIUM_PATH: a Chromium other than the one this Playwright version downloads.
const browser = await chromium.launch({ headless: true, executablePath: process.env.CHROMIUM_PATH || undefined, args: ['--no-sandbox', ...gpuArgs] });
const context = await browser.newContext({ viewport: { width: 390, height: 844 }, deviceScaleFactor: 2, hasTouch: true, isMobile: true, acceptDownloads: true });
// Headless Chromium on SwiftShader develops fine but loses the device when it presents a WebGPU canvas (seen
// with Chromium 1243; CI's parity run never presents one). Under --swiftshader the canvas draws into an
// offscreen texture instead, and the picture is measured through the page's own readback: Save a copy.
if (swiftshader)
	await context.addInitScript(() => {
		const configure = GPUCanvasContext.prototype.configure;
		GPUCanvasContext.prototype.configure = function (c) {
			this.__config = c;
		};
		GPUCanvasContext.prototype.getCurrentTexture = function () {
			const { device, format } = this.__config;
			return device.createTexture({ size: [this.canvas.width, this.canvas.height], format, usage: GPUTextureUsage.RENDER_ATTACHMENT });
		};
		void configure;
	});
const page = await context.newPage();
const errors = [];
page.on('console', (m) => m.type() === 'error' && errors.push(m.text()));
page.on('pageerror', (e) => errors.push(String(e)));

let failed = 0;
const check = (ok, what) => {
	console.log(`${ok ? 'ok  ' : 'FAIL'} ${what}`);
	if (!ok) failed++;
};

/** A copy saved through the page's Save button: the edit rendered by the develop pass and read back. */
async function savedCopy() {
	const [download] = await Promise.all([page.waitForEvent('download', { timeout: 120_000 }), page.getByRole('button', { name: 'Save a copy' }).click()]);
	return { name: download.suggestedFilename(), bytes: readFileSync(await download.path()) };
}

/**
 * Mean luminance of the picture: off a screenshot of the canvas on a real GPU, or off a saved copy under
 * SwiftShader, where the canvas isn't presented.
 */
async function brightness(name) {
	if (shots) await page.screenshot({ path: join(shots, `${name}.png`) });
	const image = swiftshader ? (await savedCopy()).bytes : await page.locator('canvas').screenshot();
	return page.evaluate(async ([b64, type]) => {
		const img = new Image();
		img.src = `data:${type};base64,${b64}`;
		await img.decode();
		const c = new OffscreenCanvas(img.width, img.height).getContext('2d');
		c.drawImage(img, 0, 0);
		const d = c.getImageData(0, 0, img.width, img.height).data;
		let sum = 0;
		for (let i = 0; i < d.length; i += 4) sum += 0.2126 * d[i] + 0.7152 * d[i + 1] + 0.0722 * d[i + 2];
		return sum / (d.length / 4);
	}, [image.toString('base64'), swiftshader ? 'image/jpeg' : 'image/png']);
}

try {
	await page.goto(`${url}/m/`);
	await page.locator('input[accept="image/*"]').setInputFiles(photo);
	await page.locator('canvas').waitFor({ timeout: 30_000 });
	await page.waitForTimeout(swiftshader ? 4000 : 1500);
	const plain = await brightness('1-opened');
	check(plain > 5, `the photo is drawn (mean ${plain.toFixed(1)} of 255)`);

	const exposure = page.getByRole('slider', { name: 'Exposure' }).first();
	await exposure.focus();
	for (let i = 0; i < 10; i++) await exposure.press('Shift+ArrowRight');
	await page.waitForTimeout(swiftshader ? 3000 : 800);
	const brighter = await brightness('2-exposure');
	check(brighter > plain + 5, `Exposure +1 EV brightens it (${plain.toFixed(1)} → ${brighter.toFixed(1)})`);
	check((await exposure.getAttribute('aria-valuenow')) === '1', `the slider reads ${await exposure.getAttribute('aria-valuenow')}`);

	await page.getByRole('button', { name: 'Colour' }).click();
	check(await page.getByRole('slider', { name: 'Saturation' }).first().isVisible(), 'the Colour tab shows the editor’s own Saturation control');
	await page.getByRole('button', { name: 'Light' }).click();

	const saved = await savedCopy();
	const jpeg = saved.bytes;
	check(jpeg[0] === 0xff && jpeg[1] === 0xd8 && jpeg.length > 10_000, `saved ${saved.name}, a ${(jpeg.length / 1e3).toFixed(0)} kB JPEG`);

	await page.getByRole('button', { name: 'Open in Sidecar' }).click();
	await page.waitForURL('**/edit/**', { timeout: 60_000 });
	const key = page.url().split('/edit/')[1];
	const sha1 = createHash('sha1').update(readFileSync(photo)).digest('hex');
	check(key === sha1, `imported as ${key.slice(0, 12)}…, the photo's own SHA-1`);
	const state = await page.evaluate(async (k) => (await fetch(`/api/assets/${k}/edit`)).json(), key);
	check(Math.abs(state.edit.exposure.ev - 1) < 1e-6, `the server holds the phone's edit (exposure ${state.edit.exposure.ev} EV)`);
	if (shots) await page.waitForTimeout(3000), await page.screenshot({ path: join(shots, '3-full-editor.png') });

	if (raw) {
		await page.goto(`${url}/m/`);
		const uploads = [];
		page.on('request', (r) => r.method() === 'POST' && uploads.push(r.url()));
		const started = Date.now();
		await page.locator('input[accept^="image/*,"]').setInputFiles(raw);
		const shown = page.getByRole('img', { name: /camera's own preview/ });
		await shown.waitFor({ timeout: 30_000 });
		await page.waitForFunction(() => document.querySelector('main img')?.complete ?? false);
		const [w, h] = await shown.evaluate((img) => [img.naturalWidth, img.naturalHeight]);
		check(w > 0 && h > 0, `a RAW's embedded preview is shown in ${Date.now() - started} ms (${w}×${h})`);
		if (shots) await page.screenshot({ path: join(shots, '4-raw-preview.png') });
		const decoding = Date.now();
		await page.locator('canvas').waitFor({ timeout: 600_000 });
		check(true, `then the RAW is developed on the phone (${((Date.now() - decoding) / 1e3).toFixed(1)} s here)`);
		check(await page.getByRole('slider', { name: 'Highlight recovery' }).first().isVisible(), 'with the controls a RAW has, Highlight recovery among them');
		await page.waitForTimeout(swiftshader ? 4000 : 1500);
		const rawShown = await brightness('5-raw-developed');
		check(rawShown > 5, `and a picture (mean ${rawShown.toFixed(1)} of 255)`);
		check(uploads.length === 0, 'with nothing sent to the server for any of it');
	}
} finally {
	check(errors.length === 0, `no errors in the page${errors.length ? `: ${errors.join(' | ')}` : ''}`);
	await browser.close();
}
process.exit(failed ? 1 : 0);
