// SPDX-License-Identifier: Apache-2.0
// Defringe end to end (DESIGN.md §15, "Defringe"): Lens → Defringe → Purple on a photo, the edit saved with it,
// the browser's canvas and the worker's `/preview` both changed where there are fringes and nowhere much else;
// then Purple back to 0, which leaves no defringe in the edit.
//
//   node web/scripts/defringe-smoke.mjs <base-url> <photo path on the server>
//
// Run inside the dev container with --network host, against `./dev/serve`. The photo's edit is reset first.
import { chromium } from 'playwright';

const [base, photo] = process.argv.slice(2);
if (!base || !photo) throw new Error('usage: defringe-smoke.mjs <base-url> <photo path>');
const browser = await chromium.launch({
	headless: true,
	args: ['--no-sandbox', '--enable-unsafe-webgpu', '--enable-features=Vulkan', '--use-vulkan=native', '--use-angle=vulkan', '--disable-vulkan-surface', '--ignore-gpu-blocklist']
});
const steps = [];
const check = (ok, what) => {
	if (!ok) throw new Error(`failed: ${what}`);
	steps.push(what);
};

/** How two images of one size differ: the share of pixels that moved by more than 4/255, and the mean move. */
async function difference(page, a, ta, b, tb) {
	return page.evaluate(
		async ({ a, ta, b, tb }) => {
			const load = async (b64, type) => {
				const bmp = await createImageBitmap(await (await fetch(`data:${type};base64,${b64}`)).blob());
				const c = new OffscreenCanvas(bmp.width, bmp.height).getContext('2d');
				c.drawImage(bmp, 0, 0);
				return c.getImageData(0, 0, bmp.width, bmp.height).data;
			};
			const [x, y] = [await load(a, ta), await load(b, tb)];
			let [moved, sum] = [0, 0];
			for (let i = 0; i < x.length; i += 4) {
				const d = Math.max(Math.abs(x[i] - y[i]), Math.abs(x[i + 1] - y[i + 1]), Math.abs(x[i + 2] - y[i + 2]));
				if (d > 4) moved++;
				sum += d;
			}
			return { moved: moved / (x.length / 4), mean: sum / (x.length / 4) };
		},
		{
			a: Buffer.from(a).toString('base64'),
			ta,
			b: Buffer.from(b).toString('base64'),
			tb
		}
	);
}

try {
	const page = await browser.newPage({
		viewport: { width: 1400, height: 1000 },
		deviceScaleFactor: 1
	});
	page.on('pageerror', (e) => console.error('[pageerror]', e.message));
	const open = async () => {
		await page.goto(`${base}/open?path=${encodeURIComponent(photo)}`, {
			waitUntil: 'load'
		});
		await page.getByRole('slider', { name: 'Exposure' }).first().waitFor({ state: 'attached', timeout: 60000 });
		return page.url().split('/edit/')[1];
	};
	let key = await open();
	await page.request.post(`${base}/api/assets/${key}/history/checkout`, {
		data: { node: 'original' }
	});
	key = await open();
	await page.waitForTimeout(1500);
	const canvas = page.locator('canvas').first();
	const before = await canvas.screenshot();

	const set = async (id, value) => {
		const label = page.locator(`#${id}-label`).first();
		await label.evaluate((s) => {
			for (let d = s.closest('details'); d; d = d.parentElement?.closest('details') ?? null) if (!d.open) d.open = true;
		});
		await label.scrollIntoViewIfNeeded();
		await label.locator('xpath=following-sibling::button').click();
		const input = label.locator('xpath=following-sibling::input');
		await input.fill(String(value));
		await input.press('Enter');
		await page.waitForTimeout(1500);
	};
	await set('fringe-purple', 100);
	await set('fringe-green', 100);
	const after = await canvas.screenshot();
	const browserMoved = await difference(page, before, 'image/png', after, 'image/png');
	console.log('browser', JSON.stringify(browserMoved));
	check(browserMoved.moved > 0, `the canvas changes (${(browserMoved.moved * 100).toFixed(3)} % of pixels)`);
	check(browserMoved.moved < 0.05 && browserMoved.mean < 1, 'but only at fringes: under 5 % of pixels, under 1/255 on average');

	await page.waitForTimeout(5500);
	const saved = (await (await page.request.get(`${base}/api/assets/${key}/edit`)).json()).edit;
	console.log('edit', JSON.stringify(saved.defringe));
	check(saved.defringe?.purple === 100 && saved.defringe?.green === 100, 'the edit saves both amounts');
	check(saved.defringe.purpleHue?.length === 2 && saved.defringe.greenHue?.length === 2, 'and their hue bands');

	const preview = async (edit) =>
		(
			await page.request.post(`${base}/api/assets/${key}/preview`, {
				data: { edit, colorSpace: 'srgb', longEdge: 1600 }
			})
		).body();
	const plain = { ...saved };
	delete plain.defringe;
	const workerMoved = await difference(page, await preview(plain), 'image/jpeg', await preview(saved), 'image/jpeg');
	console.log('worker', JSON.stringify(workerMoved));
	check(workerMoved.moved > 0, `the worker's preview changes too (${(workerMoved.moved * 100).toFixed(3)} % of pixels)`);
	check(workerMoved.moved < 0.05 && workerMoved.mean < 1, 'and as little');

	await set('fringe-purple', 0);
	await set('fringe-green', 0);
	await page.waitForTimeout(5500);
	const off = (await (await page.request.get(`${base}/api/assets/${key}/edit`)).json()).edit;
	check(!off.defringe, 'both amounts at 0 leave no defringe in the edit');
	console.log(steps.map((s) => `ok  ${s}`).join('\n'));
} finally {
	await browser.close();
}
