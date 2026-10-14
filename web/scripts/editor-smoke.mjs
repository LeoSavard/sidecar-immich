// SPDX-License-Identifier: Apache-2.0
// Drives every editor section against a running server and prints the edit it saved, so a refactor of the
// editor can be checked by running this before and after and comparing the output.
//
//   node scripts/editor-smoke.mjs <base-url> <photo path on the server> [--narrow] [--fallback]
//
// --narrow uses a phone-sized viewport (the tab bar) instead of the desktop accordion; --fallback leaves
// WebGPU off, as in a browser without it. Run from web/ inside the dev container with --network host, against
// `./dev/serve`. The photo's edit is reset to the original first.
import { chromium } from 'playwright';

const [base, photo] = process.argv.slice(2).filter((a) => !a.startsWith('--'));
const narrow = process.argv.includes('--narrow');
const fallback = process.argv.includes('--fallback');
if (!base || !photo) throw new Error('usage: editor-smoke.mjs <base-url> <photo path> [--narrow] [--fallback]');

const gpuArgs = fallback ? [] : ['--enable-unsafe-webgpu', '--enable-features=Vulkan', '--use-vulkan=native', '--use-angle=vulkan', '--disable-vulkan-surface', '--ignore-gpu-blocklist'];
const browser = await chromium.launch({ headless: true, args: ['--no-sandbox', ...gpuArgs] });
const log = (...a) => console.error(...a);
/** The first visible match — controls exist twice at some widths (tab bar and accordion). Waits up to 5 s: a
 *  section's content can render a moment after it opens. */
const visible = async (locator) => {
	for (let t = 0; t < 50; t++) {
		for (const l of await locator.all()) if (await l.isVisible()) return l;
		await new Promise((r) => setTimeout(r, 100));
	}
	return null;
};

try {
	const page = await browser.newPage({ viewport: narrow ? { width: 390, height: 844 } : { width: 1400, height: 1000 }, deviceScaleFactor: 1 });
	page.on('pageerror', (e) => log('[pageerror]', e.message));
	page.on('console', (m) => ['error', 'warning'].includes(m.type()) && !m.text().includes('WebGPU unavailable') && log(`[console ${m.type()}]`, m.text()));

	const open = async () => {
		await page.goto(`${base}/open?path=${encodeURIComponent(photo)}`, { waitUntil: 'load' });
		await page.getByRole('slider', { name: 'Exposure' }).first().waitFor({ state: 'attached', timeout: 60000 });
		return page.url().split('/edit/')[1];
	};
	const key = await open();
	const state = async () => (await page.request.get(`${base}/api/assets/${key}/edit`)).json();
	await page.request.post(`${base}/api/assets/${key}/history/checkout`, { data: { node: 'original' } });
	await open();
	const mode = (await page.getByText('This browser has no WebGPU').count()) ? 'fallback' : 'webgpu';
	log('mode:', mode);
	if (mode === 'webgpu' && fallback) throw new Error('expected the fallback');
	if (mode === 'fallback' && !fallback) throw new Error('no WebGPU adapter: run on the GPU host, or pass --fallback');

	async function section(label) {
		if (narrow) {
			await page.getByRole('button', { name: label, exact: true }).first().click();
			return;
		}
		const summary = await visible(page.locator('summary', { hasText: label }));
		if (!summary) throw new Error(`no section ${label}`);
		const open = await summary.evaluate((s) => s.parentElement.open);
		if (!open) await summary.click();
	}
	async function set(id, value) {
		const button = await visible(page.locator(`#${id}-label`).locator('xpath=following-sibling::button'));
		if (!button) throw new Error(`no slider ${id}`);
		await button.click();
		const input = await visible(page.locator(`#${id}-label`).locator('xpath=following-sibling::input'));
		await input.fill(String(value));
		await input.press('Enter');
	}
	async function click(name) {
		const b = await visible(page.getByRole('button', { name, exact: true }));
		if (!b) throw new Error(`no button ${name}`);
		await b.click();
	}

	const steps = [];
	const step = async (name, run) => {
		await run();
		steps.push(name);
	};

	await step('white balance', async () => {
		await section('White balance');
		await set('wb-temp', 4800);
		await set('wb-tint', 8);
	});
	await step('light', async () => {
		await section('Light');
		await set('exposure', 0.5);
		await set('contrast', 20);
		await set('highlights', -30);
		await set('shadows', 25);
		await set('whites', 10);
		await set('blacks', -5);
		await set('hl-blend', 70);
	});
	await step('curve', async () => {
		await section('Curve');
		const s = await visible(page.locator('svg[aria-label^="Tone curve"]'));
		await s.scrollIntoViewIfNeeded();
		const before = await s.getAttribute('aria-label');
		// Relative to the element, so a layout shift between measuring and clicking can't move the point.
		const b = await s.boundingBox();
		await s.click({ position: { x: 0.4 * b.width, y: 0.5 * b.height } });
		await page.waitForFunction((was) => document.querySelector('svg[aria-label^="Tone curve"]')?.getAttribute('aria-label') !== was, before, { timeout: 5000 });
	});
	await step('color', async () => {
		await section('Color');
		await set('saturation', 15);
		await (await visible(page.getByRole('radio', { name: 'Blue' }))).click();
		await set('mix-sat', -40);
	});
	await step('detail', async () => {
		await section('Detail');
		await set('sharpen', 20);
		await set('denoise-luma', 10);
	});
	await step('crop', async () => {
		await section('Crop and straighten');
		await (await visible(page.getByRole('radio', { name: '1:1' }))).click();
		await set('geo-angle', 2);
	});
	await step('lens', async () => {
		await section('Lens');
		const toggle = await visible(page.getByRole('button', { name: /^(On|Off)$/ }));
		if (toggle && (await toggle.textContent()) === 'On') await set('lens-distortion', 80).catch(() => {});
	});
	await step('undo and redo', async () => {
		await click('Undo');
		await click('Redo');
	});
	await step('save', async () => {
		const saveButton = await visible(page.getByRole('button', { name: 'Save' }));
		if (saveButton) await saveButton.click();
		await page.waitForFunction(() => [...document.querySelectorAll('button')].some((b) => b.textContent?.trim() === 'Saved'), null, { timeout: 15000 });
	});
	await step('history', async () => {
		await section('History');
		await page.getByText('Loading history…').waitFor({ state: 'detached', timeout: 15000 }).catch(() => {});
	});
	await step('copy and paste look', async () => {
		await click('Copy look');
		await click('Paste look');
	});

	const saved = (await state()).edit;
	console.log(JSON.stringify({ mode, layout: narrow ? 'narrow' : 'wide', steps, edit: saved }, null, 1));
} catch (e) {
	log('FAIL', e.message);
	process.exitCode = 1;
} finally {
	await browser.close();
}
