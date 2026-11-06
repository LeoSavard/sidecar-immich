// SPDX-License-Identifier: Apache-2.0
// Focus peaking, refined (DESIGN.md §11, "Focus peaking"): on a portrait, peaking on, each face's eyes rated
// over the photo; Subject on, peaking kept to the subject — less of the frame lit, some of it still lit.
//
//   node web/scripts/peaking-smoke.mjs <base-url> <portrait path on the server>
//
// Run inside the dev container with --network host, against `./dev/serve` with the models (sidecar-ml).
import { chromium } from 'playwright';

const [base, photo] = process.argv.slice(2);
if (!base || !photo) throw new Error('usage: peaking-smoke.mjs <base-url> <portrait path>');
const browser = await chromium.launch({ headless: true, args: ['--no-sandbox', '--enable-unsafe-webgpu', '--enable-features=Vulkan', '--use-vulkan=native', '--use-angle=vulkan', '--disable-vulkan-surface', '--ignore-gpu-blocklist'] });
const steps = [];
const check = (ok, what) => {
	if (!ok) throw new Error(`failed: ${what}`);
	steps.push(what);
};

/** Pixels in the peaking colour (`view.wgsl`'s 0.98, 0.86, 0.15). */
async function peaked(page, bytes) {
	return page.evaluate(async (b64) => {
		const bmp = await createImageBitmap(await (await fetch(`data:image/png;base64,${b64}`)).blob());
		const c = new OffscreenCanvas(bmp.width, bmp.height).getContext('2d');
		c.drawImage(bmp, 0, 0);
		const d = c.getImageData(0, 0, bmp.width, bmp.height).data;
		let n = 0;
		for (let i = 0; i < d.length; i += 4) if (Math.abs(d[i] - 250) < 6 && Math.abs(d[i + 1] - 219) < 6 && Math.abs(d[i + 2] - 38) < 10) n++;
		return n;
	}, Buffer.from(bytes).toString('base64'));
}

try {
	const page = await browser.newPage({ viewport: { width: 1400, height: 1000 } });
	page.on('pageerror', (e) => console.error('[pageerror]', e.message));
	await page.goto(`${base}/open?path=${encodeURIComponent(photo)}`, { waitUntil: 'load' });
	await page.getByRole('slider', { name: 'Exposure' }).first().waitFor({ state: 'attached', timeout: 60000 });
	await page.getByRole('button', { name: 'Focus peaking' }).click();
	const eye = page.getByRole('img', { name: /eye of face 1/ }).first();
	await eye.waitFor({ timeout: 180000 });
	const labels = await page.getByRole('img', { name: /eye of face/ }).evaluateAll((els) => els.map((e) => e.getAttribute('aria-label')));
	console.log(labels.join('\n'));
	check(labels.length >= 2, `each face's eyes are rated over the photo (${labels.length})`);
	const canvas = page.locator('canvas').first();
	await page.waitForTimeout(1000);
	const all = await peaked(page, await canvas.screenshot());
	await page.getByRole('button', { name: 'Subject', exact: true }).click();
	await page.waitForTimeout(2500);
	const subject = await peaked(page, await canvas.screenshot());
	console.log({ all, subject });
	check(all > 0 && subject > 0 && subject < all, `Subject keeps peaking to the subject (${all} → ${subject} pixels)`);
	console.log(steps.map((s) => `ok  ${s}`).join('\n'));
} finally {
	await browser.close();
}
