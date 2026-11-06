// SPDX-License-Identifier: Apache-2.0
// Retouch end to end (DESIGN.md §15, "Retouch"): Heal armed, a tap on the photo, and the spot it makes — saved
// with a source found for it, drawn on the photo, changing the picture there in the browser's canvas and in the
// worker's `/preview` alike, and nowhere else; then removed.
//
//   node web/scripts/retouch-smoke.mjs <base-url> <photo path on the server>
//
// Run inside the dev container with --network host, against `./dev/serve`. The photo's edit is reset first.
import { chromium } from 'playwright';

const [base, photo] = process.argv.slice(2);
if (!base || !photo) throw new Error('usage: retouch-smoke.mjs <base-url> <photo path>');
const browser = await chromium.launch({ headless: true, args: ['--no-sandbox', '--enable-unsafe-webgpu', '--enable-features=Vulkan', '--use-vulkan=native', '--use-angle=vulkan', '--disable-vulkan-surface', '--ignore-gpu-blocklist'] });
const steps = [];
const check = (ok, what) => {
	if (!ok) throw new Error(`failed: ${what}`);
	steps.push(what);
};

/** Mean absolute difference of two images (PNG or JPEG) inside and outside a circle (fractions of the frame). */
async function differ(page, a, b, type, c, r) {
	return page.evaluate(
		async ({ a, b, type, c, r }) => {
			const load = async (b64) => {
				const bmp = await createImageBitmap(await (await fetch(`data:${type};base64,${b64}`)).blob());
				const cx = new OffscreenCanvas(bmp.width, bmp.height).getContext('2d');
				cx.drawImage(bmp, 0, 0);
				return { w: bmp.width, h: bmp.height, d: cx.getImageData(0, 0, bmp.width, bmp.height).data };
			};
			const [p, q] = [await load(a), await load(b)];
			let [inSum, inN, outSum, outN] = [0, 0, 0, 0];
			for (let y = 0; y < p.h; y += 2)
				for (let x = 0; x < p.w; x += 2) {
					const i = (y * p.w + x) * 4;
					const d = Math.abs(p.d[i] - q.d[i]) + Math.abs(p.d[i + 1] - q.d[i + 1]) + Math.abs(p.d[i + 2] - q.d[i + 2]);
					const dist = Math.hypot(x / p.w - c[0], (y / p.h - c[1]) * (p.h / p.w));
					if (dist < r) [inSum, inN] = [inSum + d, inN + 1];
					else if (dist > 2.5 * r) [outSum, outN] = [outSum + d, outN + 1];
				}
			return { inside: inSum / inN, outside: outSum / outN };
		},
		{ a: Buffer.from(a).toString('base64'), b: Buffer.from(b).toString('base64'), type, c, r }
	);
}

try {
	const page = await browser.newPage({ viewport: { width: 1400, height: 1000 }, deviceScaleFactor: 1 });
	page.on('pageerror', (e) => console.error('[pageerror]', e.message));
	const open = async () => {
		await page.goto(`${base}/open?path=${encodeURIComponent(photo)}`, { waitUntil: 'load' });
		await page.getByRole('slider', { name: 'Exposure' }).first().waitFor({ state: 'attached', timeout: 60000 });
		return page.url().split('/edit/')[1];
	};
	let key = await open();
	await page.request.post(`${base}/api/assets/${key}/history/checkout`, { data: { node: 'original' } });
	key = await open();
	await page.waitForTimeout(1500);
	const canvas = page.locator('canvas').first();
	const preview = async (edit) => {
		const r = await page.request.post(`${base}/api/assets/${key}/preview`, { data: { edit, colorSpace: 'srgb', longEdge: 800 } });
		if (!r.ok()) throw new Error(`preview: ${r.status()} ${await r.text()}`);
		return r.body();
	};

	// Retouch → Heal, then a tap on the middle of the photo.
	const summary = page.locator('summary').filter({ hasText: /^Retouch$/ }).last();
	await summary.evaluate((s) => {
		for (let d = s.parentElement; d; d = d.parentElement?.closest('details') ?? null) if (!d.open) d.open = true;
	});
	const tools = page.getByRole('group', { name: 'Retouch tool' }).last();
	await tools.scrollIntoViewIfNeeded();
	await tools.getByRole('button', { name: 'Heal', exact: true }).click();
	check((await tools.getByRole('button', { name: 'Heal', exact: true }).getAttribute('aria-pressed')) === 'true', 'Heal is armed');
	const before = await canvas.screenshot();
	const box = await canvas.boundingBox();
	await page.mouse.click(box.x + box.width * 0.45, box.y + box.height * 0.5);
	await page.waitForTimeout(6000);
	const saved = (await (await page.request.get(`${base}/api/assets/${key}/edit`)).json()).edit;
	const spot = saved.spots?.[0];
	console.log('spot', JSON.stringify(spot));
	check(saved.spots?.length === 1 && spot.kind === 'heal', 'a tap makes one heal spot, saved');
	check(Math.hypot(spot.from[0] - spot.at[0], spot.from[1] - spot.at[1]) > 1.5 * spot.radius, 'with a source found away from it');
	check(await page.getByRole('button', { name: "Spot 1's source", exact: true }).isVisible(), 'drawn on the photo with its source');
	const after = await canvas.screenshot();
	const shot = await differ(page, before, after, 'image/png', [0.45, 0.5], 0.03);
	console.log('canvas', JSON.stringify(shot));
	check(shot.inside > 4 * Math.max(shot.outside, 0.25), `the canvas changes at the spot (${shot.inside.toFixed(2)}) and hardly elsewhere (${shot.outside.toFixed(2)})`);
	// Against the saved edit without its spots: the editor's first save also brings the edit's references
	// current, which moves the whole picture and isn't what is being checked.
	const without = { ...saved };
	delete without.spots;
	const worker = await differ(page, await preview(without), await preview(saved), 'image/jpeg', spot.at, spot.radius * 0.8);
	console.log('worker', JSON.stringify(worker));
	check(worker.inside > 4 * Math.max(worker.outside, 0.25), `the worker's preview agrees (${worker.inside.toFixed(2)} at the spot, ${worker.outside.toFixed(2)} elsewhere)`);
	await page.getByRole('button', { name: 'Remove Heal 1', exact: true }).click();
	await page.waitForTimeout(5500);
	const gone = (await (await page.request.get(`${base}/api/assets/${key}/edit`)).json()).edit;
	check(!gone.spots, 'removing it leaves no spots in the edit');
	console.log(steps.map((s) => `ok  ${s}`).join('\n'));
} finally {
	await browser.close();
}
