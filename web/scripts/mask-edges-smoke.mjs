// SPDX-License-Identifier: Apache-2.0
// Mask edges from the photo (DESIGN.md §15, "Hair-accurate edges"): a part mask (hair, skin, clothes) is served
// refined — at the proxy's 1,024 px rather than the model's own size — and the subject mask, already the model's
// 1,024², as it was; a Hair layer then renders in the editor without an error.
//
//   node web/scripts/mask-edges-smoke.mjs <base-url> <photo path on the server>
//
// Run inside the dev container with --network host, against `./dev/serve` with the models (sidecar-ml).
import { chromium } from 'playwright';

const [base, photo] = process.argv.slice(2);
if (!base || !photo) throw new Error('usage: mask-edges-smoke.mjs <base-url> <photo path>');
const browser = await chromium.launch({ headless: true, args: ['--no-sandbox', '--enable-unsafe-webgpu', '--enable-features=Vulkan', '--use-vulkan=native', '--use-angle=vulkan', '--disable-vulkan-surface', '--ignore-gpu-blocklist'] });
const steps = [];
const check = (ok, what) => {
	if (!ok) throw new Error(`failed: ${what}`);
	steps.push(what);
};
const size = async (page, bytes) =>
	page.evaluate(async (b64) => {
		const bmp = await createImageBitmap(await (await fetch(`data:image/png;base64,${b64}`)).blob());
		return [bmp.width, bmp.height];
	}, Buffer.from(bytes).toString('base64'));

try {
	const page = await browser.newPage({ viewport: { width: 1400, height: 1000 } });
	const errors = [];
	page.on('pageerror', (e) => errors.push(e.message));
	await page.goto(`${base}/open?path=${encodeURIComponent(photo)}`, { waitUntil: 'load' });
	const key = page.url().split('/edit/')[1];
	await page.request.post(`${base}/api/assets/${key}/history/checkout`, { data: { node: 'original' } });
	for (let i = 0; i < 60; i++) {
		const set = await (await page.request.get(`${base}/api/assets/${key}/masks`)).json();
		if (set.available) break;
		await page.waitForTimeout(3000);
	}
	const hair = await page.request.get(`${base}/api/assets/${key}/masks/hair`);
	const subject = await page.request.get(`${base}/api/assets/${key}/masks/subject`);
	const [hs, ss] = [await size(page, await hair.body()), await size(page, await subject.body())];
	console.log({ hair: hs, subject: ss });
	check(Math.max(...hs) === 1024, `the hair mask is served refined, at the proxy's 1,024 px (${hs.join(' × ')})`);
	check(Math.max(...ss) === 1024, `the subject mask as it was (${ss.join(' × ')})`);

	await page.reload({ waitUntil: 'load' });
	await page.getByRole('slider', { name: 'Exposure' }).first().waitFor({ state: 'attached', timeout: 60000 });
	const summary = page.locator('summary').filter({ hasText: /^Masks$/ }).last();
	await summary.evaluate((s) => {
		for (let d = s.parentElement; d; d = d.parentElement?.closest('details') ?? null) if (!d.open) d.open = true;
	});
	await page.getByRole('button', { name: 'Hair', exact: true }).click();
	await page.waitForTimeout(3000);
	check(errors.length === 0, `a Hair layer renders in the editor${errors.length ? `: ${errors.join('; ')}` : ''}`);
	console.log(steps.map((s) => `ok  ${s}`).join('\n'));
} finally {
	await browser.close();
}
