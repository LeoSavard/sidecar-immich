// SPDX-License-Identifier: Apache-2.0
// A browser check for spectral colour film (DESIGN.md §7, "Colour film, spectrally"): choose the stock in the
// editor and confirm the picture really renders through it, that its scene light moves it, and that the edit
// keeps it. This is the only end-to-end check of the path the parity harness cannot reach — the server's
// profile, over HTTP, into the wasm packer, onto the GPU — which is where its bugs have been.
//
//   node scripts/slide-check.mjs <base-url> <photo path on the server> <out dir>
//
// Run from web/ inside the dev container with --network host, against a `./dev/serve` whose
// SIDECAR_STATUS_A and SIDECAR_ISO_DAYLIGHT point at the operator's standards; without those the server
// offers no stock and this fails at the first click. The photo's edit is reset to the original first.
// It writes a PNG of the canvas per state, to be looked at: a slide should be denser than the neutral
// rendering, and daylight film under tungsten should go warm.
import { chromium } from 'playwright';
import { writeFileSync } from 'node:fs';

const [base, photo, out] = process.argv.slice(2);
if (!base || !photo || !out) throw new Error('usage: slide-check.mjs <base-url> <photo path> <out dir>');

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
	page.on('pageerror', (e) => log('[pageerror]', e.message));
	page.on('console', (m) => ['error', 'warning'].includes(m.type()) && log(`[console ${m.type()}]`, m.text()));
	page.on('response', (r) => r.url().includes('/api/film/') && log('[net]', r.status(), r.url()));
	page.on('requestfailed', (r) => log('[net failed]', r.url(), r.failure()?.errorText));

	const open = async () => {
		await page.goto(`${base}/open?path=${encodeURIComponent(photo)}`, { waitUntil: 'load' });
		await page.getByRole('slider', { name: 'Exposure' }).first().waitFor({ state: 'attached', timeout: 120000 });
		return page.url().split('/edit/')[1];
	};
	const key = await open();
	await page.request.post(`${base}/api/assets/${key}/history/checkout`, { data: { node: 'original' } });
	await open();
	if (await page.getByText('This browser has no WebGPU').count()) throw new Error('no WebGPU adapter');

	log('stocks the server offers:', await (await page.request.get(`${base}/api/film/stocks`)).text());

	// A WebGPU canvas cannot be read back through createImageBitmap, so the picture is captured as a
	// screenshot of the canvas itself and compared by its bytes — and looked at.
	const { createHash } = await import('node:crypto');
	const shot = async (name) => {
		const png = await page.locator('canvas').first().screenshot();
		writeFileSync(`${out}/${name}.png`, png);
		const hash = createHash('sha1').update(png).digest('hex').slice(0, 12);
		log(`${name}: ${png.length} bytes, sha1 ${hash}`);
		return hash;
	};

	// Open the Film section (desktop accordion: the group, then the section's own header).
	const summary = page.locator('summary').filter({ hasText: /^Film$/ }).last();
	for (let at = await summary.elementHandle(); at; at = null) {
		await page.evaluate((el) => {
			for (let d = el.closest('details'); d; d = d.parentElement?.closest('details')) d.open = true;
		}, at);
	}
	await page.waitForTimeout(500);
	const neutral = await shot('slide-neutral');

	(await visible(page.getByRole('radio', { name: 'Film', exact: true }))).click();
	await page.waitForTimeout(800);
	const film = await shot('slide-film-generic');

	const stock = await visible(page.getByRole('radio', { name: /PROVIA 100F/ }));
	if (!stock) throw new Error('no PROVIA stock offered — the server has no profile for it');
	await stock.click();
	// The profile is a four-megabyte download the first time; give it room.
	await page.waitForTimeout(6000);
	const slide = await shot('slide-provia');
	writeFileSync(`${out}/slide-provia-page.png`, await page.screenshot());

	// Its light, too.
	const tungsten = await visible(page.getByRole('radio', { name: 'Tungsten', exact: true }));
	if (!tungsten) throw new Error("no scene light offered for the stock");
	await tungsten.click();
	await page.waitForTimeout(1200);
	const warm = await shot('slide-provia-tungsten');

	log('profile fetch from inside the page:', await page.evaluate(async () => {
		const r = await fetch('/api/film/stocks/provia-100f');
		const b = await r.arrayBuffer();
		const h = new DataView(b);
		return { status: r.status, bytes: b.byteLength, magic: h.getUint32(0, true).toString(16), version: h.getUint32(4, true), side: h.getUint32(8, true) };
	}));
	const saved = await (await page.request.get(`${base}/api/assets/${key}/edit`)).json();
	log('saved film:', JSON.stringify(saved.edit?.film ?? saved.film ?? null));
	if (slide === film) throw new Error('choosing the stock changed nothing');
	if (warm === slide) throw new Error('the scene light changed nothing');
	if (neutral === film) throw new Error('film rendering changed nothing');
	log('OK');
} finally {
	await browser.close();
}
