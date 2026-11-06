// SPDX-License-Identifier: Apache-2.0
// Snapshots and the split view end to end (DESIGN.md §15, "Snapshots"): Exposure +1 kept as the snapshot "Bright",
// Exposure −1 as "Dark"; Bright set beside the edit, so the canvas is bright left of the line and dark right of it;
// the line dragged, and the boundary following it; Bright opened again; the split turned off.
//
//   node web/scripts/snapshots-smoke.mjs <base-url> <photo path on the server>
//
// Run inside the dev container with --network host, against `./dev/serve`. The photo's edit is reset first.
import { chromium } from 'playwright';

const [base, photo] = process.argv.slice(2);
if (!base || !photo) throw new Error('usage: snapshots-smoke.mjs <base-url> <photo path>');
const browser = await chromium.launch({ headless: true, args: ['--no-sandbox', '--enable-unsafe-webgpu', '--enable-features=Vulkan', '--use-vulkan=native', '--use-angle=vulkan', '--disable-vulkan-surface', '--ignore-gpu-blocklist'] });
const steps = [];
const check = (ok, what) => {
	if (!ok) throw new Error(`failed: ${what}`);
	steps.push(what);
};

/** How far apart two images of one size are in each of `n` vertical bands: mean absolute difference, 0…255. */
async function bandDistance(page, a, b, n) {
	return page.evaluate(
		async ({ a, b, n }) => {
			const load = async (b64) => {
				const bmp = await createImageBitmap(await (await fetch(`data:image/png;base64,${b64}`)).blob());
				const c = new OffscreenCanvas(bmp.width, bmp.height).getContext('2d');
				c.drawImage(bmp, 0, 0);
				return c.getImageData(0, 0, bmp.width, bmp.height);
			};
			const [x, y] = [await load(a), await load(b)];
			const out = new Array(n).fill(0), count = new Array(n).fill(0);
			for (let r = 0; r < x.height; r += 2)
				for (let col = 0; col < x.width; col += 2) {
					const i = (r * x.width + col) * 4, band = Math.floor((col / x.width) * n);
					out[band] += (Math.abs(x.data[i] - y.data[i]) + Math.abs(x.data[i + 1] - y.data[i + 1]) + Math.abs(x.data[i + 2] - y.data[i + 2])) / 3;
					count[band]++;
				}
			return out.map((v, i) => v / count[i]);
		},
		{ a: Buffer.from(a).toString('base64'), b: Buffer.from(b).toString('base64'), n }
	);
}

/** Which of two references each band of `shot` is: 'A', 'B', or '?' where neither is clearly nearer. */
async function sides(page, shot, refA, refB, n) {
	const [da, db] = [await bandDistance(page, shot, refA, n), await bandDistance(page, shot, refB, n)];
	return da.map((v, i) => (v < db[i] * 0.5 ? 'A' : db[i] < v * 0.5 ? 'B' : '?')).join('');
}

try {
	const page = await browser.newPage({ viewport: { width: 1400, height: 1000 } });
	page.on('pageerror', (e) => console.error('[pageerror]', e.message));
	const open = async () => {
		await page.goto(`${base}/open?path=${encodeURIComponent(photo)}`, { waitUntil: 'load' });
		await page.getByRole('slider', { name: 'Exposure' }).first().waitFor({ state: 'attached', timeout: 60000 });
		await page.waitForTimeout(1000);
		return page.url().split('/edit/')[1];
	};
	let key = await open();
	await page.request.post(`${base}/api/assets/${key}/history/checkout`, { data: { node: 'original' } });
	key = await open();
	const history = async () => (await (await page.request.get(`${base}/api/assets/${key}/history`)).json()).nodes;
	// Going back to the original keeps every version, earlier runs' snapshots too: this run's have names of its own.
	const known = new Set((await history()).map((n) => n.id));
	const run = Date.now() % 100000;
	const [bright, dark] = [`Bright ${run}`, `Dark ${run}`];
	const setExposure = async (v) => {
		const label = page.locator('#exposure-label').first();
		await label.scrollIntoViewIfNeeded();
		await label.locator('xpath=following-sibling::button').click();
		await label.locator('xpath=following-sibling::input').fill(String(v));
		await label.locator('xpath=following-sibling::input').press('Enter');
		await page.waitForTimeout(800);
	};
	const snapshot = async (name) => {
		const summary = page.locator('summary').filter({ hasText: /^History$/ }).last();
		await summary.evaluate((s) => {
			for (let d = s.parentElement; d; d = d.parentElement?.closest('details') ?? null) if (!d.open) d.open = true;
		});
		await page.getByRole('button', { name: 'Take snapshot' }).click();
		await page.locator('#snapshot-name').fill(name);
		await page.getByRole('button', { name: 'Keep' }).click();
		await page.getByRole('button', { name: `Open ${name}` }).waitFor({ timeout: 15000 });
	};

	await setExposure(1);
	await snapshot(bright);
	await setExposure(-1);
	await snapshot(dark);
	const named = (await history()).filter((n) => n.label && !known.has(n.id));
	console.log('snapshots', JSON.stringify(named.map((n) => [n.label, n.edit.exposure.ev])));
	check(named.length === 2 && named.find((n) => n.label === bright)?.edit.exposure.ev === 1 && named.find((n) => n.label === dark)?.edit.exposure.ev === -1, 'two snapshots, each holding its own exposure');

	const canvas = page.locator('canvas').first();
	const darkShot = await canvas.screenshot();
	await page.getByRole('button', { name: `${bright} beside the edit` }).click();
	await page.waitForTimeout(1500);
	const half = await canvas.screenshot();

	const handle = page.getByRole('slider', { name: /^Split between/ });
	const box = await canvas.boundingBox();
	const h = await handle.boundingBox();
	await page.mouse.move(h.x + h.width / 2, h.y + h.height / 2);
	await page.mouse.down();
	await page.mouse.move(box.x + box.width * 0.25, h.y + h.height / 2, { steps: 5 });
	await page.mouse.up();
	await page.waitForTimeout(800);
	const quarter = await canvas.screenshot();

	await page.getByRole('button', { name: 'Split', exact: true }).click();
	await page.waitForTimeout(500);
	check(!(await handle.count()), 'Split off takes the line away');
	await page.getByRole('button', { name: `Open ${bright}` }).click();
	await page.waitForTimeout(2500);
	const state = await (await page.request.get(`${base}/api/assets/${key}/edit`)).json();
	check(state.edit.exposure.ev === 1, 'opening the snapshot makes it current again');
	const brightShot = await canvas.screenshot();

	// Bands across the canvas: B where it is the snapshot (Bright), D where it is the edit (Dark).
	const s1 = (await sides(page, half, brightShot, darkShot, 8)).replace(/B/g, 'D').replace(/A/g, 'B');
	const s2 = (await sides(page, quarter, brightShot, darkShot, 8)).replace(/B/g, 'D').replace(/A/g, 'B');
	console.log('split at ½:', s1, ' at ¼:', s2);
	check(/^BBB.?.DDD$/.test(s1), `side by side: the snapshot left of the line, the edit right of it (${s1})`);
	check(/^B.DDDDDD$/.test(s2), `the line dragged to a quarter, and the boundary with it (${s2})`);
	console.log(steps.map((s) => `ok  ${s}`).join('\n'));
} finally {
	await browser.close();
}
