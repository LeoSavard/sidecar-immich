// SPDX-License-Identifier: Apache-2.0
// The phone's way in end to end, at a phone's size and with touch: choose a JPEG on the home page, see it imported
// into the editing folder and opened in Sidecar's one editor, developed on the phone from its own copy (the notice says so), with
// the full editor's sections; move Exposure and check the server stores the edit. With --raw, also choose a RAW:
// its embedded preview is shown while it uploads, then the editor develops the RAW on the phone.
//
// Usage: node scripts/mobile-smoke.mjs <sidecar url> <photo.jpg> [--raw <file>] [--swiftshader] [--shots <dir>]
// (CHROMIUM_PATH to pick a browser)
// Needs a running Sidecar whose image (or `server.mobile_root`) has the module, with LibRaw built into it for
// --raw, and an editing folder or a library to import into.
import { chromium } from 'playwright';
import { createHash } from 'node:crypto';
import { readFileSync, mkdirSync } from 'node:fs';
import { join } from 'node:path';

const [url, photo] = process.argv.slice(2);
if (!url || !photo) throw new Error('usage: mobile-smoke.mjs <url> <photo.jpg> [--raw <file>] [--swiftshader] [--shots <dir>]');
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
// isMobile and hasTouch make `(pointer: coarse)` true, which is what tells the app it is on a phone.
const context = await browser.newContext({ viewport: { width: 390, height: 844 }, deviceScaleFactor: 2, hasTouch: true, isMobile: true });
// Headless Chromium on SwiftShader develops fine but loses the device when it presents a WebGPU canvas (seen
// with Chromium 1243; CI's parity run never presents one). Under --swiftshader the canvas draws into an
// offscreen texture instead; what was developed is read from the editor's `?inspect` hook.
if (swiftshader)
	await context.addInitScript(() => {
		GPUCanvasContext.prototype.configure = function (c) {
			this.__config = c;
		};
		GPUCanvasContext.prototype.getCurrentTexture = function () {
			const { device, format } = this.__config;
			return device.createTexture({ size: [this.canvas.width, this.canvas.height], format, usage: GPUTextureUsage.RENDER_ATTACHMENT });
		};
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

/** Choose `file` on the home page, and follow it into the editor; `meanwhile` looks at the page while the photo
 *  uploads. The module's own page, /m, sends the installed app there. */
async function open(file, meanwhile = async () => {}) {
	await page.goto(`${url}/m/?inspect`);
	await page.waitForURL((u) => u.pathname === '/', { timeout: 30_000 });
	const started = Date.now();
	await page.locator('input[type=file]').setInputFiles(file);
	await meanwhile(started);
	await page.waitForURL('**/edit/**', { timeout: 300_000 });
	const developed = await page
		.waitForFunction(() => window.__sidecarInspect, null, { timeout: 600_000 })
		.then(() => true)
		.catch(() => false);
	const key = page.url().split('/edit/')[1].split('?')[0];
	return { key, developed, seconds: (Date.now() - started) / 1e3 };
}

try {
	const jpeg = await open(photo);
	const sha1 = createHash('sha1').update(readFileSync(photo)).digest('hex');
	check(jpeg.key === sha1, `imported as ${jpeg.key.slice(0, 12)}…, the photo's own SHA-1, and opened in the editor`);
	check(jpeg.developed, `developed on the phone from its own copy (${jpeg.seconds.toFixed(1)} s from choosing it)`);
	const note = page.getByRole('note');
	check((await note.count()) > 0 && /on this device/.test(await note.textContent()), 'the editor says it was developed on this device');
	check((await page.getByRole('slider', { name: 'Exposure' }).first().count()) > 0, "the full editor's Light controls are there");
	// The groups the small phone editor never had: on a phone the editor shows one at a time, from these.
	for (const group of ['Processing', 'History']) check((await page.getByRole('button', { name: group, exact: true }).count()) > 0, `and its ${group} group`);
	if (shots) await page.screenshot({ path: join(shots, '1-editor.png') });

	const exposure = page.getByRole('slider', { name: 'Exposure' }).first();
	await exposure.focus();
	for (let i = 0; i < 10; i++) await exposure.press('Shift+ArrowRight');
	let saved = NaN;
	for (let i = 0; i < 30 && !(Math.abs(saved - 1) < 1e-6); i++) {
		await page.waitForTimeout(1000);
		saved = (await page.evaluate(async (k) => (await fetch(`/api/assets/${k}/edit`)).json(), jpeg.key)).edit.exposure.ev;
	}
	check(Math.abs(saved - 1) < 1e-6, `the server holds the edit made on the phone (exposure ${saved} EV)`);

	if (raw) {
		const r = await open(raw, async (started) => {
			const previewed = await page
				.getByRole('img', { name: /camera's own preview/ })
				.waitFor({ timeout: 30_000 })
				.then(() => true)
				.catch(() => false);
			check(previewed, `a RAW's embedded preview is shown while it uploads (${Date.now() - started} ms)`);
			if (shots && previewed) await page.screenshot({ path: join(shots, '2-raw-preview.png') });
		});
		check(r.developed, `then the editor develops the RAW on the phone (${r.seconds.toFixed(1)} s here)`);
		check((await page.getByRole('slider', { name: 'Highlight recovery' }).first().count()) > 0, 'with the controls a RAW has, Highlight recovery among them');
		if (shots) await page.screenshot({ path: join(shots, '3-raw-editor.png') });
	}
} finally {
	check(errors.length === 0, `no errors in the page${errors.length ? `: ${errors.join(' | ')}` : ''}`);
	await browser.close();
}
process.exit(failed ? 1 : 0);
