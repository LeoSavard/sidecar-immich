// SPDX-License-Identifier: Apache-2.0
// The histogram and overlays without WebGPU (DESIGN.md §11, "Without WebGPU"): the server's preview carries its
// histogram (`x-histogram`, `histogram.wgsl`'s counts) and paints the overlays asked for. Against the API: the
// counts add up to the picture's pixels, and the pixels painted red for clipped highlights are the clipped count.
// In a browser with no WebGPU: the histogram shows, its clipped share reads what the server counted, and
// switching on highlight clipping or focus peaking brings a preview with them painted in.
//
//   node web/scripts/fallback-overlays-smoke.mjs <base-url> <photo path on the server>
//
// Run inside the dev container with --network host, against `./dev/serve`. The photo's edit is reset first.
import { chromium } from 'playwright';

const [base, photo] = process.argv.slice(2);
if (!base || !photo) throw new Error('usage: fallback-overlays-smoke.mjs <base-url> <photo path>');
// No WebGPU flags: the editor falls back to the server's preview.
const browser = await chromium.launch({ headless: true, args: ['--no-sandbox'] });
const steps = [];
const check = (ok, what) => {
	if (!ok) throw new Error(`failed: ${what}`);
	steps.push(what);
};

/** Pixel count, and how many pixels are (near) pure red and (near) the peaking yellow, in a JPEG. */
async function painted(page, bytes) {
	return page.evaluate(async (b64) => {
		const bmp = await createImageBitmap(await (await fetch(`data:image/jpeg;base64,${b64}`)).blob());
		const c = new OffscreenCanvas(bmp.width, bmp.height).getContext('2d');
		c.drawImage(bmp, 0, 0);
		const d = c.getImageData(0, 0, bmp.width, bmp.height).data;
		let [red, yellow] = [0, 0];
		for (let i = 0; i < d.length; i += 4) {
			if (d[i] > 235 && d[i + 1] < 30 && d[i + 2] < 30) red++;
			if (Math.abs(d[i] - 250) < 12 && Math.abs(d[i + 1] - 219) < 14 && d[i + 2] < 70) yellow++;
		}
		return { pixels: bmp.width * bmp.height, red, yellow };
	}, Buffer.from(bytes).toString('base64'));
}

const decode = (b64) => new Uint32Array(Uint8Array.from(Buffer.from(b64, 'base64')).buffer);

try {
	const page = await browser.newPage({ viewport: { width: 1400, height: 1000 } });
	page.on('pageerror', (e) => console.error('[pageerror]', e.message));
	await page.goto(`${base}/open?path=${encodeURIComponent(photo)}`, { waitUntil: 'load' });
	const key = page.url().split('/edit/')[1];
	await page.request.post(`${base}/api/assets/${key}/history/checkout`, { data: { node: 'original' } });
	const edit = (await (await page.request.get(`${base}/api/assets/${key}/edit`)).json()).edit;

	// The API.
	const preview = (overlays) => page.request.post(`${base}/api/assets/${key}/preview`, { data: { edit, colorSpace: 'srgb', longEdge: 1200, overlays } });
	const plain = await preview(0);
	const h = decode(plain.headers()['x-histogram'] ?? '');
	check(h.length === 1026, 'the preview carries a histogram of 1,026 counts');
	const p0 = await painted(page, await plain.body());
	const sum = (a, b) => h.slice(a, b).reduce((x, y) => x + y, 0);
	check([0, 256, 512, 768].every((s) => sum(s, s + 256) === p0.pixels), `each channel counts every pixel (${p0.pixels})`);
	check(h[1024] > 0, `this photo has clipped highlights (${h[1024]} pixels)`);
	const clipped = await painted(page, await (await preview(1)).body());
	console.log('api', JSON.stringify({ clippedHigh: h[1024], red: clipped.red, redBefore: p0.red }));
	check(Math.abs(clipped.red - p0.red - h[1024]) <= h[1024] * 0.1 + 50, `highlight clipping paints the counted pixels red (${clipped.red - p0.red} of ${h[1024]})`);

	// The editor without WebGPU.
	await page.reload({ waitUntil: 'load' });
	await page.getByText('This browser has no WebGPU').waitFor({ timeout: 60000 });
	const clipButton = page.getByRole('button', { name: /^Show highlight clipping/ });
	await clipButton.waitFor({ timeout: 30000 });
	await page.waitForFunction(() => /\(([1-9]|0\.[0-9]*[1-9])/.test(document.querySelector('[aria-label^="Show highlight clipping"]')?.getAttribute('aria-label') ?? ''), null, { timeout: 30000 });
	const label = await clipButton.getAttribute('aria-label');
	const shown = parseFloat(label.match(/\(([\d.]+) %/)[1]);
	const expected = (100 * h[1024]) / p0.pixels;
	console.log('editor', label, 'expected about', expected.toFixed(2));
	check(shown > 0, `the histogram shows without WebGPU, with the clipped share the server counted (${label})`);

	const img = page.locator('img[src^="blob:"]').first();
	const shot = async () => {
		await page.waitForTimeout(2500);
		const src = await img.getAttribute('src');
		const bytes = await page.evaluate(async (u) => Array.from(new Uint8Array(await (await fetch(u)).arrayBuffer())), src);
		return painted(page, Buffer.from(bytes));
	};
	const before = await shot();
	await clipButton.click();
	const withClip = await shot();
	check(withClip.red > before.red + 100, `switching on highlight clipping brings a preview with them red (${before.red} → ${withClip.red})`);
	await clipButton.click();
	await page.getByRole('button', { name: 'Focus peaking' }).click();
	const peaked = await shot();
	check(peaked.yellow > before.yellow + 100, `and focus peaking, painted where the photo is sharp (${before.yellow} → ${peaked.yellow})`);
	console.log(steps.map((s) => `ok  ${s}`).join('\n'));
} finally {
	await browser.close();
}
