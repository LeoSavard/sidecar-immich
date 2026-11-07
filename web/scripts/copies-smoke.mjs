// SPDX-License-Identifier: Apache-2.0
// Virtual copies end to end (DESIGN.md §15, "Virtual copies"): the photo at Exposure +0.5; History → New virtual
// copy "Mono", which opens as its own entry; Exposure −1 there; the photo's own edit untouched by it; the copy in
// Recent with its own name; an export of the copy, named for it and made from its edit (to this device only);
// then the copy removed, and its versions left in the photo's history.
//
//   node web/scripts/copies-smoke.mjs <base-url> <photo path on the server>
//
// Run inside the dev container with --network host, against `./dev/serve`. The photo's edit is reset first.
import { chromium } from 'playwright';

const [base, photo] = process.argv.slice(2);
if (!base || !photo) throw new Error('usage: copies-smoke.mjs <base-url> <photo path>');
const browser = await chromium.launch({ headless: true, args: ['--no-sandbox', '--enable-unsafe-webgpu', '--enable-features=Vulkan', '--use-vulkan=native', '--use-angle=vulkan', '--disable-vulkan-surface', '--ignore-gpu-blocklist'] });
const steps = [];
const check = (ok, what) => {
	if (!ok) throw new Error(`failed: ${what}`);
	steps.push(what);
};

try {
	const page = await browser.newPage({ viewport: { width: 1400, height: 1000 } });
	page.on('pageerror', (e) => console.error('[pageerror]', e.message));
	const ready = async () => {
		await page.getByRole('slider', { name: 'Exposure' }).first().waitFor({ state: 'attached', timeout: 60000 });
		await page.waitForTimeout(1000);
	};
	const open = async () => {
		await page.goto(`${base}/open?path=${encodeURIComponent(photo)}`, { waitUntil: 'load' });
		await ready();
		return page.url().split('/edit/')[1];
	};
	let key = await open();
	await page.request.post(`${base}/api/assets/${key}/history/checkout`, { data: { node: 'original' } });
	// Earlier runs' copies go first, so the names below are this run's.
	for (const c of (await (await page.request.get(`${base}/api/assets/${key}/edit`)).json()).copies ?? []) await page.request.delete(`${base}/api/assets/${key}/copies/${c.id}`);
	key = await open();
	const state = async (k) => (await page.request.get(`${base}/api/assets/${k}/edit`)).json();
	const setExposure = async (v) => {
		const label = page.locator('#exposure-label').first();
		await label.scrollIntoViewIfNeeded();
		await label.locator('xpath=following-sibling::button').click();
		await label.locator('xpath=following-sibling::input').fill(String(v));
		await label.locator('xpath=following-sibling::input').press('Enter');
		await page.waitForTimeout(5500);
	};
	const openHistory = async () => {
		const summary = page.locator('summary').filter({ hasText: /^History$/ }).last();
		await summary.evaluate((s) => {
			for (let d = s.parentElement; d; d = d.parentElement?.closest('details') ?? null) if (!d.open) d.open = true;
		});
	};

	await setExposure(0.5);
	await openHistory();
	await page.getByRole('button', { name: 'New virtual copy' }).click();
	await page.locator('#copy-name').fill('Mono');
	await Promise.all([page.waitForURL(/\/edit\/[0-9a-f]{40}~c[0-9a-f]{6}$/, { timeout: 30000 }), page.getByRole('button', { name: 'Make it' }).click()]);
	await ready();
	const copyKey = page.url().split('/edit/')[1];
	check(copyKey.startsWith(`${key}~c`), `New virtual copy opens the copy as its own entry (${copyKey.slice(40)})`);
	check(await page.getByText('— Mono').first().isVisible(), 'the editor says which copy it is');
	const started = await state(copyKey);
	check(started.edit.exposure.ev === 0.5 && started.copy?.name === 'Mono', 'the copy starts from the version it was made from');

	await setExposure(-1);
	const [own, copy] = [await state(key), await state(copyKey)];
	check(copy.edit.exposure.ev === -1, 'the copy saves its own edit');
	check(own.edit.exposure.ev === 0.5 && own.copies.length === 1, "and the photo's own edit is untouched by it");

	await page.goto(`${base}/`, { waitUntil: 'networkidle' });
	const entry = page.locator(`a[href="/edit/${copyKey}"]`).first();
	check((await entry.count()) === 1 && (await entry.innerText()).includes('— Mono'), 'Recent lists the copy under its own name');

	const job = await (await page.request.post(`${base}/api/exports`, { data: { assetKey: copyKey, editEtag: copy.etag, destination: 'device', longEdge: 600 } })).json();
	let status;
	for (let i = 0; i < 120 && !status?.finished; i++) {
		await page.waitForTimeout(1000);
		status = await (await page.request.get(`${base}/api/exports/${job.jobId}`)).json();
	}
	console.log('export', JSON.stringify(status.last));
	const file = status.last?.data?.file ?? '';
	check(status.finished && /-mono-v\d+(-\w+)*\.jpg$/.test(file), `an export of the copy is named for it (${file})`);
	const stale = await page.request.post(`${base}/api/exports`, { data: { assetKey: copyKey, editEtag: own.etag, destination: 'device' } });
	check(stale.status() === 409, "and is made from the copy's edit, not the photo's");

	const node = copy.head;
	await page.request.delete(`${base}/api/assets/${key}/copies/${copy.copy.id}`);
	const after = await state(key);
	const history = await (await page.request.get(`${base}/api/assets/${key}/history`)).json();
	check(after.copies.length === 0 && history.nodes.some((n) => n.id === node), "removing the copy leaves its versions in the photo's history");
	check((await page.request.get(`${base}/api/assets/${copyKey}/edit`)).status() === 404, 'and its key answers not found');
	console.log(steps.map((s) => `ok  ${s}`).join('\n'));
} finally {
	await browser.close();
}
