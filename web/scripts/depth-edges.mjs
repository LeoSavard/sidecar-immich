// SPDX-License-Identifier: Apache-2.0
// A browser check for the depth map's edges (ROADMAP, "Depth, beyond what shipped"): put a strong lens blur
// on a photo and capture the canvas, so the edge between the sharp subject and the blurred background can be
// looked at. The refinement runs in the browser's own WebGPU here, from the same WGSL the worker runs, so
// this is what catches a pipeline or uniform mistake that the engine's test cannot.
//
//   node scripts/depth-edges.mjs <base-url> <photo path on the server> <out dir>
//
// Run from web/ inside the dev container with --network host, against `./dev/serve` built with the ML image
// (the depth map comes from sidecar-ml; without it the photo has no depth and this says so). The photo's edit
// is reset to the original first.
import { chromium } from 'playwright';
import { writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';

const [base, photo, out] = process.argv.slice(2);
if (!base || !photo || !out) throw new Error('usage: depth-edges.mjs <base-url> <photo path> <out dir>');

const gpuArgs = ['--enable-unsafe-webgpu', '--enable-features=Vulkan', '--use-vulkan=native', '--use-angle=vulkan', '--disable-vulkan-surface', '--ignore-gpu-blocklist'];
const browser = await chromium.launch({ headless: true, args: ['--no-sandbox', ...gpuArgs] });
const log = (...a) => console.log(...a);
const visible = async (locator) => {
	for (let t = 0; t < 60; t++) {
		for (const l of await locator.all()) if (await l.isVisible()) return l;
		await new Promise((r) => setTimeout(r, 100));
	}
	return null;
};

try {
	const page = await browser.newPage({ viewport: { width: 1400, height: 1000 }, deviceScaleFactor: 1 });
	const problems = [];
	page.on('pageerror', (e) => problems.push(`[pageerror] ${e.message}`));
	page.on('console', (m) => ['error', 'warning'].includes(m.type()) && !m.text().includes('WebGPU unavailable') && problems.push(`[console ${m.type()}] ${m.text()}`));

	const open = async () => {
		await page.goto(`${base}/open?path=${encodeURIComponent(photo)}`, { waitUntil: 'load' });
		await page.getByRole('slider', { name: 'Exposure' }).first().waitFor({ state: 'attached', timeout: 120000 });
		return page.url().split('/edit/')[1];
	};
	const key = await open();
	await page.request.post(`${base}/api/assets/${key}/history/checkout`, { data: { node: 'original' } });
	await open();
	if (await page.getByText('This browser has no WebGPU').count()) throw new Error('no WebGPU adapter');

	const masks = await (await page.request.get(`${base}/api/assets/${key}/masks`)).json();
	log('masks:', JSON.stringify(masks).slice(0, 200));
	if (!masks?.available) throw new Error('this build has no masks, so no depth map: run the ML image');

	// Open Depth (in Processing) and put a strong blur on, focused on the nearest subject.
	const summary = page.locator('summary').filter({ hasText: /^Depth$/ }).last();
	if (!(await summary.count())) throw new Error('no Depth section');
	await summary.evaluate((s) => {
		for (let d = s.parentElement; d; d = d.parentElement?.closest('details') ?? null) if (!d.open) d.open = true;
	});
	const set = async (id, value) => {
		const button = await visible(page.locator(`#${id}-label`).locator('xpath=following-sibling::button'));
		if (!button) throw new Error(`no slider ${id}`);
		await button.click();
		const input = await visible(page.locator(`#${id}-label`).locator('xpath=following-sibling::input'));
		await input.fill(String(value));
		await input.press('Enter');
	};
	const shot = async (name) => {
		const png = await page.locator('canvas').first().screenshot();
		writeFileSync(`${out}/${name}.png`, png);
		log(`${name}: ${png.length} bytes, sha1 ${createHash('sha1').update(png).digest('hex').slice(0, 12)}`);
	};

	await page.waitForTimeout(2000);
	await shot('depth-edges-plain');
	// Focused on the nearest thing with a real depth of field, so the near subject stays sharp against a
	// blurred background: that transition is what the refinement is for.
	await set('blur-amount', 100);
	await set('blur-focus', 100);
	await set('blur-range', 20);
	await page.waitForTimeout(3000);
	await shot('depth-edges-blurred');

	log('saved edit:', JSON.stringify((await (await page.request.get(`${base}/api/assets/${key}/edit`)).json()).edit?.lensBlur ?? null));
	if (problems.length) {
		for (const p of problems) log(p);
		throw new Error(`${problems.length} console or page errors`);
	}
	log('OK — look at the two PNGs: the blur\'s edge should follow the subject, not drift into a halo around it.');
} finally {
	await browser.close();
}
