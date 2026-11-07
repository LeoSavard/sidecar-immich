// SPDX-License-Identifier: Apache-2.0
// Detail per layer end to end (DESIGN.md §15, "Detail per layer"): a brush layer over the left half of the photo
// with Noise reduction +100 and no global noise reduction, asked of the worker's 1:1 loupe at the photo's centre —
// the left of the tile denoised, the right exactly as without the layer.
//
//   node web/scripts/detail-layer-smoke.mjs <base-url> <photo path on the server>
//
// Run inside the dev container with --network host, against `./dev/serve`.
import { chromium } from 'playwright';

const [base, photo] = process.argv.slice(2);
if (!base || !photo) throw new Error('usage: detail-layer-smoke.mjs <base-url> <photo path>');
const browser = await chromium.launch({ headless: true, args: ['--no-sandbox'] });
const steps = [];
const check = (ok, what) => {
	if (!ok) throw new Error(`failed: ${what}`);
	steps.push(what);
};

/** The noise of each half of a PNG: the mean absolute difference of luminance from its 3 × 3 neighbourhood. */
async function halves(page, bytes) {
	return page.evaluate(async (b64) => {
		const bmp = await createImageBitmap(await (await fetch(`data:image/png;base64,${b64}`)).blob());
		const c = new OffscreenCanvas(bmp.width, bmp.height).getContext('2d');
		c.drawImage(bmp, 0, 0);
		const { width: w, height: h } = bmp;
		const d = c.getImageData(0, 0, w, h).data;
		const y = (x, yy) => {
			const i = (yy * w + x) * 4;
			return 0.2126 * d[i] + 0.7152 * d[i + 1] + 0.0722 * d[i + 2];
		};
		const band = (x0, x1) => {
			let s = 0;
			let n = 0;
			for (let yy = 4; yy < h - 4; yy++)
				for (let x = x0; x < x1; x++) {
					let m = 0;
					for (let j = -1; j <= 1; j++) for (let i = -1; i <= 1; i++) m += y(x + i, yy + j);
					s += Math.abs(y(x, yy) - m / 9);
					n++;
				}
			return s / n;
		};
		return { left: band(8, w / 2 - 24), right: band(w / 2 + 24, w - 8) };
	}, Buffer.from(bytes).toString('base64'));
}

try {
	const page = await browser.newPage();
	await page.goto(`${base}/open?path=${encodeURIComponent(photo)}`, { waitUntil: 'load' });
	await page.getByRole('slider', { name: 'Exposure' }).first().waitFor({ state: 'attached', timeout: 60000 });
	const key = page.url().split('/edit/')[1];
	const original = (await (await page.request.get(`${base}/api/assets/${key}/edit`)).json()).edit;
	const plain = { ...original, masks: [], detail: { ...original.detail, denoiseLuminance: 0, denoiseChroma: 0, sharpenAmount: 0 } };
	const layer = { mask: 'brush', invert: false, amount: 100, exposure: 0, contrast: 0, saturation: 0, temperature: 0, tint: 0, noise: 100, strokes: [{ erase: false, radius: 0.25, feather: 0, flow: 1, points: [[0.25, -0.2], [0.25, 0.5], [0.25, 1.2]] }] };
	const layered = { ...plain, masks: [layer] };
	const loupe = async (edit) => {
		const r = await page.request.post(`${base}/api/assets/${key}/loupe`, { data: { center: [0.5, 0.5], size: [512, 256], edit, colorSpace: 'srgb' } });
		if (!r.ok()) throw new Error(`loupe: ${r.status()} ${await r.text()}`);
		return halves(page, await r.body());
	};
	const a = await loupe(plain);
	const b = await loupe(layered);
	console.log(JSON.stringify({ without: a, with: b }));
	check(b.left < 0.8 * a.left, `the layer's half is denoised (${a.left.toFixed(3)} → ${b.left.toFixed(3)})`);
	check(Math.abs(b.right - a.right) < 0.02 * a.right, `the other half is as it was (${a.right.toFixed(3)} → ${b.right.toFixed(3)})`);
	console.log(steps.map((s) => `ok  ${s}`).join('\n'));
} finally {
	await browser.close();
}
