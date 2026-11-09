// SPDX-License-Identifier: Apache-2.0
// What each light slider does beyond what it is aimed at (DESIGN.md §7, module 4): the photo is rendered by the
// worker at its original edit and with one slider moved, and the two are compared per tonal band of the
// original — the band's mean OKLab lightness shift, how much of its local contrast it keeps (mean |L − 9×9 mean
// of L|), and how much of its OKLab chroma and saturation (chroma over lightness) it keeps.
//
//   node web/scripts/tone-audit.mjs <base-url> <photo path on the server>… [--json]
//
// Run inside the dev container with --network host, against `./dev/serve`. It writes nothing to the photo.
import { chromium } from 'playwright';

const args = process.argv.slice(2);
const json = args.includes('--json');
const [base, ...photos] = args.filter((a) => !a.startsWith('--'));
if (!base || !photos.length) throw new Error('usage: tone-audit.mjs <base-url> <photo path>… [--json]');

export const VARIANTS = process.env.VARIANTS ? JSON.parse(process.env.VARIANTS) : {
	'Highlights −50': { 'tone.highlights': -50 },
	'Highlights −100': { 'tone.highlights': -100 },
	'Highlights +50': { 'tone.highlights': 50 },
	'Shadows +50': { 'tone.shadows': 50 },
	'Shadows +100': { 'tone.shadows': 100 },
	'Shadows −50': { 'tone.shadows': -50 },
	'Contrast +50': { 'tone.contrast': 1.5 },
	'Contrast −50': { 'tone.contrast': 0.5 },
	'Exposure −1 EV': { 'exposure.ev': -1 }
};
const BANDS = [
	['shadows', 0.0, 0.4],
	['lower mids', 0.4, 0.55],
	['upper mids', 0.55, 0.7],
	['highlights', 0.7, 0.85],
	['near white', 0.85, 0.97]
];

const browser = await chromium.launch({ headless: true, args: ['--no-sandbox'] });
const page = await browser.newPage();

/** OKLab L and chroma per pixel, and a 9 × 9 box mean of L, of a JPEG. */
async function analyse(bytes) {
	return page.evaluate(async (b64) => {
		const bmp = await createImageBitmap(await (await fetch('data:image/jpeg;base64,' + b64)).blob());
		const c = new OffscreenCanvas(bmp.width, bmp.height).getContext('2d');
		c.drawImage(bmp, 0, 0);
		const { width: w, height: h } = bmp;
		const d = c.getImageData(0, 0, w, h).data;
		const lin = (v) => { v /= 255; return v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; };
		const L = new Float32Array(w * h), C = new Float32Array(w * h);
		for (let i = 0; i < w * h; i++) {
			const [r, g, b] = [lin(d[i * 4]), lin(d[i * 4 + 1]), lin(d[i * 4 + 2])];
			const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
			const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
			const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
			L[i] = 0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s;
			const A = 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s;
			const B = 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s;
			C[i] = Math.hypot(A, B);
		}
		// 9 × 9 box mean of L by an integral image.
		const I = new Float64Array((w + 1) * (h + 1));
		for (let y = 0; y < h; y++) for (let x = 0, row = 0; x < w; x++) { row += L[y * w + x]; I[(y + 1) * (w + 1) + x + 1] = I[y * (w + 1) + x + 1] + row; }
		const M = new Float32Array(w * h);
		for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) {
			const [x0, x1, y0, y1] = [Math.max(0, x - 4), Math.min(w, x + 5), Math.max(0, y - 4), Math.min(h, y + 5)];
			M[y * w + x] = (I[y1 * (w + 1) + x1] - I[y0 * (w + 1) + x1] - I[y1 * (w + 1) + x0] + I[y0 * (w + 1) + x0]) / ((x1 - x0) * (y1 - y0));
		}
		return { w, h, L: [...L], C: [...C], M: [...M] };
	}, Buffer.from(bytes).toString('base64'));
}

const results = {};
for (const photo of photos) {
	await page.goto(`${base}/open?path=${encodeURIComponent(photo)}`, { waitUntil: 'load' });
	await page.waitForURL(/\/edit\//, { timeout: 60000 });
	const key = page.url().split('/edit/')[1];
	const hist = await (await page.request.get(`${base}/api/assets/${key}/history`)).json();
	const original = hist.nodes.find((n) => n.id === 'original').edit;
	const render = async (edit) => {
		const r = await page.request.post(`${base}/api/assets/${key}/preview`, { data: { edit, colorSpace: 'srgb', longEdge: 1000 } });
		if (!r.ok()) throw new Error(`preview ${r.status()}`);
		return analyse(await r.body());
	};
	const ref = await render(original);
	const name = photo.split('/').at(-1);
	results[name] = {};
	for (const [label, patch] of Object.entries(VARIANTS)) {
		const e = structuredClone(original);
		for (const [path, v] of Object.entries(patch)) { const ks = path.split('.'); let o = e; for (const k of ks.slice(0, -1)) o = o[k] ??= {}; o[ks.at(-1)] = v; }
		const v = await render(e);
		const row = {};
		for (const [band, lo, hi] of BANDS) {
			let n = 0, dL = 0, lc0 = 0, lc1 = 0, c0 = 0, c1 = 0, s0 = 0, s1 = 0;
			for (let i = 0; i < ref.L.length; i++) {
				const l = ref.L[i];
				if (l < lo || l >= hi) continue;
				n++;
				dL += v.L[i] - l;
				lc0 += Math.abs(l - ref.M[i]);
				lc1 += Math.abs(v.L[i] - v.M[i]);
				c0 += ref.C[i];
				c1 += v.C[i];
				s0 += ref.C[i] / Math.max(l, 0.05);
				s1 += v.C[i] / Math.max(v.L[i], 0.05);
			}
			row[band] = n < 200 ? null : { share: n / ref.L.length, dL: dL / n, detail: lc1 / lc0, chroma: c1 / c0, saturation: s1 / s0 };
		}
		results[name][label] = row;
	}
}
await browser.close();

if (json) console.log(JSON.stringify(results, null, 1));
else
	for (const [name, rows] of Object.entries(results)) {
		console.log(`\n${name}   (per band of the original: ΔL in OKLab ×100 · detail kept · chroma kept · saturation kept)`);
		console.log(''.padEnd(18) + BANDS.map(([b]) => b.padStart(26)).join(''));
		for (const [label, row] of Object.entries(rows))
			console.log(label.padEnd(18) + BANDS.map(([b]) => { const c = row[b]; return (c ? `${(c.dL * 100).toFixed(1).padStart(5)} ${c.detail.toFixed(2)} ${c.chroma.toFixed(2)} ${c.saturation.toFixed(2)}` : '—').padStart(26); }).join(''));
	}
