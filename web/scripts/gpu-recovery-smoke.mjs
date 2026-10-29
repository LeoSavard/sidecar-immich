// SPDX-License-Identifier: Apache-2.0
// Does the editor rebuild itself after the browser takes its GPU device away?
//
// iOS drops the WebGPU device when Safari is backgrounded, which used to cost a reload. The page now rebuilds
// its passes and re-uploads the proxy it kept. `?inspect` exposes a real loss (`device.destroy()`, which
// invalidates every pipeline and texture) followed by the real recovery, so what runs here is the rebuild and
// nothing else.
//
// The test is the picture: the canvas after the rebuild must hold the frame it held before, pixel for pixel.
// A texture view left over from the dead device is a validation error the page never sees — the frame simply
// stops arriving — so "no error banner" on its own proves nothing, and neither does a device being present.
//
//   node scripts/gpu-recovery-smoke.mjs <editor url>
import { chromium } from 'playwright';

const [url] = process.argv.slice(2);
if (!url) {
	console.error('usage: node scripts/gpu-recovery-smoke.mjs <editor url>');
	process.exit(2);
}
const browser = await chromium.launch({
	headless: true,
	args: ['--no-sandbox', '--enable-unsafe-webgpu', '--enable-features=Vulkan', '--use-vulkan=native', '--use-angle=vulkan', '--disable-vulkan-surface', '--ignore-gpu-blocklist']
});
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
const complaints = [];
page.on('console', (m) => {
	const line = m.text().split('\n')[0];
	if (/Invalid|cannot be used with/.test(line)) complaints.push(line);
});
page.on('pageerror', (e) => complaints.push(`pageerror: ${e.message}`));

const target = new URL(url);
target.searchParams.set('inspect', '');
await page.goto(target.toString(), { waitUntil: 'networkidle', timeout: 120_000 });
await page.waitForTimeout(4000);

if ((await page.evaluate(() => typeof window.__sidecarGpu?.lose)) !== 'function') {
	console.error('FAIL: no ?inspect GPU hook — is this a WebGPU browser, and did the page reach the editor?');
	await browser.close();
	process.exit(1);
}

/**
 * The photo's half of the window, as the compositor shows it. A WebGPU canvas drawn into a 2D context comes
 * back empty here, so the screenshot is the only honest read of what is on screen.
 */
const photo = { x: 0, y: 0, width: 1120, height: 900 };
const shot = () => page.screenshot({ clip: photo });

/** How much of a screenshot is not black, decoded by the browser that took it. */
const litFraction = (png) =>
	page.evaluate(async (base64) => {
		const img = new Image();
		img.src = `data:image/png;base64,${base64}`;
		await img.decode();
		const off = new OffscreenCanvas(img.width, img.height);
		const ctx = off.getContext('2d');
		ctx.drawImage(img, 0, 0);
		const d = ctx.getImageData(0, 0, img.width, img.height).data;
		let lit = 0;
		for (let i = 0; i < d.length; i += 4) if (d[i] + d[i + 1] + d[i + 2] > 24) lit++;
		return lit / (d.length / 4);
	}, png.toString('base64'));

const before = await shot();
await page.evaluate(() => window.__sidecarGpu.lose());
await page.waitForTimeout(5000);
const after = await shot();
const lit = await litFraction(after);

const banner = await page.locator('.text-danger').allInnerTexts();
await browser.close();

console.log(`the frame after the rebuild is ${before.equals(after) ? 'identical to' : 'NOT'} the frame before it`);
console.log(`lit after the rebuild: ${(lit * 100).toFixed(1)} % of the photo pane`);
console.log(`error banner: ${JSON.stringify(banner)}`);
if (complaints.length) console.log(`GPU complaints:\n  ${complaints.join('\n  ')}`);

const drew = lit > 0.05;
const same = before.equals(after);
if (!drew) console.error('FAIL: the photo pane is blank after the rebuild');
if (!same) console.error('FAIL: the frame after the rebuild is not the frame before it');
if (banner.length) console.error('FAIL: the page is showing an error');
if (complaints.length) console.error('FAIL: the GPU rejected something');
const ok = drew && same && !banner.length && !complaints.length;
console.log(ok ? 'PASS: the editor rebuilt itself and drew the same frame' : 'FAIL');
process.exit(ok ? 0 : 1);
