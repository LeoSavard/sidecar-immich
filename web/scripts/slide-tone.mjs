// SPDX-License-Identifier: Apache-2.0
// A browser check that the tone controls reach a spectral colour stock (DESIGN.md §7, "Grading what comes
// out"). With the stock chosen at Amount 100, Contrast and Blacks used to move and do nothing, because they
// reached the picture only through the neutral rendering the stock had replaced. Each one must now change the
// canvas, and the stock itself must still look like the stock.
//
//   node scripts/slide-tone.mjs <base-url> <photo path on the server> <out dir>
//
// Run from web/ inside the dev container with --network host, against a `./dev/serve` whose
// SIDECAR_STATUS_A and SIDECAR_ISO_DAYLIGHT point at the operator's standards.
import { chromium } from 'playwright';
import { writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';

const [base, photo, out] = process.argv.slice(2);
if (!base || !photo || !out) throw new Error('usage: slide-tone.mjs <base-url> <photo path> <out dir>');

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
	page.on('console', (m) => m.type() === 'error' && problems.push(`[console] ${m.text()}`));

	const open = async () => {
		await page.goto(`${base}/open?path=${encodeURIComponent(photo)}`, { waitUntil: 'load' });
		await page.getByRole('slider', { name: 'Exposure' }).first().waitFor({ state: 'attached', timeout: 120000 });
		return page.url().split('/edit/')[1];
	};
	const key = await open();
	await page.request.post(`${base}/api/assets/${key}/history/checkout`, { data: { node: 'original' } });
	await open();
	if (await page.getByText('This browser has no WebGPU').count()) throw new Error('no WebGPU adapter');

	const openSection = async (name) => {
		const summary = page.locator('summary').filter({ hasText: new RegExp(`^${name}$`) }).last();
		if (!(await summary.count())) throw new Error(`no ${name} section`);
		await summary.evaluate((s) => {
			for (let d = s.parentElement; d; d = d.parentElement?.closest('details') ?? null) if (!d.open) d.open = true;
		});
	};
	const set = async (id, value) => {
		const button = await visible(page.locator(`#${id}-label`).locator('xpath=following-sibling::button'));
		if (!button) throw new Error(`no slider ${id}`);
		await button.click();
		const input = await visible(page.locator(`#${id}-label`).locator('xpath=following-sibling::input'));
		await input.fill(String(value));
		await input.press('Enter');
		await page.waitForTimeout(900);
	};
	const shot = async (name) => {
		const png = await page.locator('canvas').first().screenshot();
		writeFileSync(`${out}/${name}.png`, png);
		const h = createHash('sha1').update(png).digest('hex').slice(0, 12);
		log(`${name.padEnd(26)} sha1 ${h}`);
		return h;
	};

	await openSection('Film');
	(await visible(page.getByRole('radio', { name: 'Film', exact: true }))).click();
	await page.waitForTimeout(600);
	const stock = await visible(page.getByRole('radio', { name: /PROVIA 100F/ }));
	if (!stock) throw new Error('no PROVIA stock offered — the server has no profile for it');
	await stock.click();
	await page.waitForTimeout(6000);
	const plain = await shot('slide-tone-plain');

	await openSection('Light');
	const results = {};
	// All three are bipolar sliders with 0 as their default.
	for (const [id, value] of [
		['contrast', 80],
		['blacks', 60],
		['whites', -60]
	]) {
		await set(id, value);
		results[id] = await shot(`slide-tone-${id}`);
		await set(id, 0); // back to default before the next one
		await page.waitForTimeout(600);
	}

	for (const [id, h] of Object.entries(results)) {
		if (h === plain) throw new Error(`${id} changed nothing with the stock on — it is still dead`);
		log(`${id}: changes the picture`);
	}
	if (problems.length) {
		for (const p of problems) log(p);
		throw new Error(`${problems.length} console or page errors`);
	}
	log('OK — every tone control reaches the stock.');
} finally {
	await browser.close();
}
