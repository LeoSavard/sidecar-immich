// SPDX-License-Identifier: Apache-2.0
// Presets, copy and paste, and sync across a selection end to end (DESIGN.md §15, "Presets, copy and paste,
// sync"): on photo A, Exposure set and the Light section alone copied; pasted onto B, which takes the exposure
// and keeps its own white balance; Light kept from A as a preset; on the home page, B and C selected and the
// preset applied to both, each as a new version; the preset applied in the editor; the preset exported as a file and imported back. The presets
// the test made are deleted at the end.
//
//   node web/scripts/presets-smoke.mjs <base-url> <photo A> <photo B> <photo C>
//
// Run inside the dev container with --network host, against `./dev/serve`. The three photos' edits are reset first.
import { readFileSync } from 'node:fs';
import { chromium } from 'playwright';

const [base, ...photos] = process.argv.slice(2);
if (!base || photos.length !== 3) throw new Error('usage: presets-smoke.mjs <base-url> <photo A> <photo B> <photo C>');
const browser = await chromium.launch({ headless: true, args: ['--no-sandbox', '--enable-unsafe-webgpu', '--enable-features=Vulkan', '--use-vulkan=native', '--use-angle=vulkan', '--disable-vulkan-surface', '--ignore-gpu-blocklist'] });
const steps = [];
const check = (ok, what) => {
	if (!ok) throw new Error(`failed: ${what}`);
	steps.push(what);
};

const context = await browser.newContext({ viewport: { width: 1400, height: 1000 }, acceptDownloads: true });
const page = await context.newPage();
page.on('pageerror', (e) => console.error('[pageerror]', e.message));
const made = [];
try {
	const open = async (photo) => {
		await page.goto(`${base}/open?path=${encodeURIComponent(photo)}`, { waitUntil: 'load' });
		await page.getByRole('slider', { name: 'Exposure' }).first().waitFor({ state: 'attached', timeout: 60000 });
		await page.waitForTimeout(1000);
		return page.url().split('/edit/')[1];
	};
	const edit = async (key) => (await (await page.request.get(`${base}/api/assets/${key}/edit`)).json()).edit;
	const keys = [];
	for (const p of photos) {
		const key = await open(p);
		await page.request.post(`${base}/api/assets/${key}/history/checkout`, { data: { node: 'original' } });
		keys.push(key);
	}
	const [a, b, c] = keys;
	const openSection = async (label) => {
		const summary = page.locator('summary').filter({ hasText: new RegExp(`^${label}$`) }).last();
		await summary.evaluate((s) => {
			for (let d = s.parentElement; d; d = d.parentElement?.closest('details') ?? null) if (!d.open) d.open = true;
		});
	};

	// A: Exposure +0.8, then copy the Light section alone.
	await open(photos[0]);
	const label = page.locator('#exposure-label').first();
	await label.scrollIntoViewIfNeeded();
	await label.locator('xpath=following-sibling::button').click();
	await label.locator('xpath=following-sibling::input').fill('0.8');
	await label.locator('xpath=following-sibling::input').press('Enter');
	await page.waitForTimeout(800);
	await openSection('History');
	await page.getByRole('button', { name: 'Copy…' }).click();
	for (const box of await page.getByRole('checkbox').all()) if (await box.isChecked()) await box.uncheck();
	await page.getByRole('checkbox', { name: 'Light' }).check();
	await page.getByRole('button', { name: 'Copy these' }).click();
	await page.waitForTimeout(5500);
	check((await edit(a)).exposure.ev === 0.8, 'A saves Exposure +0.8');

	// B: paste.
	const wbBefore = (await edit(b)).whiteBalance;
	await open(photos[1]);
	await openSection('History');
	await page.getByRole('button', { name: 'Paste', exact: true }).click();
	await page.waitForTimeout(5500);
	const pastedB = await edit(b);
	check(pastedB.exposure.ev === 0.8, 'pasting the Light section onto B gives it Exposure +0.8');
	check(JSON.stringify(pastedB.whiteBalance) === JSON.stringify(wbBefore), "and leaves B's white balance its own");

	// A: keep Light as a preset.
	await open(photos[0]);
	await openSection('History');
	await page.getByRole('button', { name: 'Save as preset…' }).click();
	await page.locator('#preset-name').fill('Smoke bright');
	for (const box of await page.getByRole('checkbox').all()) if (await box.isChecked()) await box.uncheck();
	await page.getByRole('checkbox', { name: 'Light' }).check();
	await page.getByRole('button', { name: 'Save preset' }).click();
	await page.waitForTimeout(800);
	let presets = await (await page.request.get(`${base}/api/presets`)).json();
	const preset = presets.find((p) => p.name === 'Smoke bright');
	if (preset) made.push(preset.id);
	check(preset && preset.sections.join() === 'light' && preset.edit.exposure.ev === 0.8, 'the preset keeps Light, at +0.8');
	check(preset.edit.whiteBalance.mode === 'asShot', "and nothing of A's other sections");

	// Home: select B and C, apply the preset.
	await page.request.post(`${base}/api/assets/${b}/history/checkout`, { data: { node: 'original' } });
	const versionsC = (await (await page.request.get(`${base}/api/assets/${c}/history`)).json()).nodes.length;
	await page.goto(`${base}/`, { waitUntil: 'networkidle' });
	await page.getByRole('button', { name: 'Select' }).first().click();
	for (const key of [b, c]) await page.locator(`a[href="/edit/${key}"]`).first().click();
	await page.locator('#batch-preset').selectOption(preset.id);
	await page.getByRole('button', { name: 'Apply preset' }).click();
	await page.getByText('Applied to 2 of 2.').waitFor({ timeout: 30000 });
	const [eb, ec] = [await edit(b), await edit(c)];
	check(eb.exposure.ev === 0.8 && ec.exposure.ev === 0.8, 'applied to a selection of two, both take Exposure +0.8');
	const after = (await (await page.request.get(`${base}/api/assets/${c}/history`)).json()).nodes.length;
	check(after === versionsC + 1, `each as a version of its own (C: ${versionsC} → ${after} versions)`);
	check(page.url() === `${base}/`, 'and selecting did not open a photo');

	// In the editor: C back to its original, the preset applied from the History tool.
	await page.request.post(`${base}/api/assets/${c}/history/checkout`, { data: { node: 'original' } });
	await open(photos[2]);
	await openSection('History');
	await page.getByRole('button', { name: 'Smoke bright', exact: true }).click();
	await page.waitForTimeout(5500);
	check((await edit(c)).exposure.ev === 0.8, 'the preset applies in the editor too');

	// Export the preset and import it back.
	await open(photos[0]);
	await openSection('History');
	const [download] = await Promise.all([page.waitForEvent('download'), page.getByRole('button', { name: 'Export Smoke bright' }).click()]);
	const file = await download.path();
	const parsed = JSON.parse(readFileSync(file, 'utf8'));
	check(parsed.format === 'sidecar-preset' && parsed.sections.join() === 'light' && parsed.edit.exposure.ev === 0.8, 'Export writes a preset file');
	await page.locator('input[type=file][aria-label="Import a preset file"]').setInputFiles({ name: 'Smoke bright.sidecar-preset.json', mimeType: 'application/json', buffer: readFileSync(file) });
	await page.waitForTimeout(1000);
	presets = await (await page.request.get(`${base}/api/presets`)).json();
	const twins = presets.filter((p) => p.name === 'Smoke bright');
	for (const p of twins) if (!made.includes(p.id)) made.push(p.id);
	check(twins.length === 2 && twins[1].edit.exposure.ev === 0.8, 'and Import reads it back as a preset');
	console.log(steps.map((s) => `ok  ${s}`).join('\n'));
} finally {
	for (const id of made) await page.request.delete(`${base}/api/presets/${id}`).catch(() => {});
	await browser.close();
}
