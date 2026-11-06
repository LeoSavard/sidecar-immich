// SPDX-License-Identifier: Apache-2.0
// A mask the worker makes, beside the photo it was made from, for looking at: for each photo, `<out>-<n>-<mask>.png`
// (the mask) and `<out>-<n>-photo.jpg` (the worker's preview at its edit), computing the masks if need be.
//
//   node web/scripts/mask-dump.mjs <base-url> <mask name> <out prefix> <photo path on the server>…
//
// Run inside the dev container with --network host, against `./dev/serve` with the models (`./dev/ml-models`).
// The output is the owner's photos: keep it under testdata/cache and delete it when done.
import { chromium } from 'playwright';
import fs from 'node:fs';

const [base, name, out, ...photos] = process.argv.slice(2);
if (!base || !name || !out || !photos.length) throw new Error('usage: mask-dump.mjs <base-url> <mask> <out> <photo>…');
const browser = await chromium.launch({ headless: true, args: ['--no-sandbox'] });
try {
	const page = await browser.newPage();
	for (const [i, photo] of photos.entries()) {
		await page.goto(`${base}/open?path=${encodeURIComponent(photo)}`, { waitUntil: 'load' });
		await page.getByRole('slider', { name: 'Exposure' }).first().waitFor({ state: 'attached', timeout: 60000 });
		const key = page.url().split('/edit/')[1];
		let set;
		for (let t = 0; t < 120; t++) {
			set = await (await page.request.get(`${base}/api/assets/${key}/masks`)).json();
			if (set.available || set.names?.length) break;
			await page.waitForTimeout(2000);
		}
		console.log(photo, JSON.stringify(set.names));
		fs.writeFileSync(`${out}-${i}-${name}.png`, await (await page.request.get(`${base}/api/assets/${key}/masks/${name}`)).body());
		const edit = (await (await page.request.get(`${base}/api/assets/${key}/edit`)).json()).edit;
		const r = await page.request.post(`${base}/api/assets/${key}/preview`, { data: { edit, colorSpace: 'srgb', longEdge: 1024 } });
		fs.writeFileSync(`${out}-${i}-photo.jpg`, await r.body());
	}
} finally {
	await browser.close();
}
