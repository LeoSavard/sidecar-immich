// SPDX-License-Identifier: Apache-2.0
// The editing folder's limit (DESIGN.md §3), in headless Chromium against a running server: the folder row,
// the two-tap remove, and that a full folder really does stand between the import button and the file
// picker — Playwright's filechooser event is what proves the picker did or didn't open.
//   node scripts/folder-limit-smoke.mjs <base-url> <dir of photos to import> [outDir]
import { chromium } from 'playwright';
import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';

const [base, photoDir, outDir] = process.argv.slice(2);
if (!base || !photoDir) throw new Error('usage: folder-limit-smoke.mjs <base-url> <photo dir> [outDir]');

const browser = await chromium.launch({ headless: true, args: ['--no-sandbox'] });
const failures = [];
const check = (ok, what) => {
	console.log(`${ok ? 'ok  ' : 'FAIL'} ${what}`);
	if (!ok) failures.push(what);
};

const page = await browser.newPage({ viewport: { width: 760, height: 1000 } });
page.on('pageerror', (e) => failures.push(`pageerror: ${e.message}`));
page.on('console', (m) => m.type() === 'error' && console.log(`console.error: ${m.text()}`));

/** Did clicking it open the OS file picker? Resolves false if nothing asks for a file within the timeout. */
async function opensPicker(locator) {
	let opened = false;
	const seen = page.waitForEvent('filechooser', { timeout: 2500 }).then((fc) => {
		opened = true;
		// Dismissing it: an unanswered chooser blocks the page.
		return fc.setFiles([]);
	}).catch(() => {});
	await locator.click();
	await seen;
	return opened;
}

// ---- fill the folder through the API, which is not what is under test here
const photos = readdirSync(photoDir).filter((f) => f.endsWith('.CR2')).sort();
await page.goto(base, { waitUntil: 'networkidle' });
for (const key of (await (await page.request.get(`${base}/api/intake/folder`)).json()).items ?? []) {
	await page.request.delete(`${base}/api/intake/folder/${key.assetKey}`);
}
for (const f of photos.slice(0, 4)) {
	const r = await page.request.post(`${base}/api/intake?to=folder`, {
		multipart: { file: { name: f, mimeType: 'image/x-canon-cr2', buffer: readFileSync(join(photoDir, f)) } }
	});
	if (!r.ok()) throw new Error(`setup import of ${f}: ${await r.text()}`);
}

await page.goto(base, { waitUntil: 'networkidle' });
await page.waitForTimeout(600);

// ---- the folder row
const row = page.locator('section', { has: page.getByRole('heading', { name: 'Imported' }) });
check(await row.count() === 1, 'the imported photos have a row of their own');
const head = (await row.locator('div').first().innerText()).replace(/\s+/g, ' ').trim();
check(head.includes('4 of 4'), `the heading counts them (${head})`);
// One row high whatever is in it: the tiles scroll sideways rather than wrapping.
const listBox = await row.locator('ul').boundingBox();
const tileBox = await row.locator('li').first().boundingBox();
check(listBox.height <= tileBox.height + 8, `one thumbnail row tall (${Math.round(listBox.height)}px for a ${Math.round(tileBox.height)}px tile)`);
check(!(await row.locator('li').first().getByRole('button').isVisible().catch(() => false)), 'with Remove kept behind More info');

// ---- full, nothing exported: the picker must not open
const chooseBtn = page.getByRole('button', { name: 'Choose a photo on this device' });
check(!(await opensPicker(chooseBtn)), 'a full folder stops the file picker opening');
const dialog = page.locator('[role="alertdialog"]');
check(await dialog.isVisible(), 'and says so instead');
check(/nothing in it has been exported/i.test(await dialog.innerText()), 'naming the reason nothing can be removed for you');
check(await page.getByRole('button', { name: 'Remove it and import' }).count() === 0, 'with no way to proceed, since every photo is the only copy of itself');
check(await row.locator('li').first().getByRole('button').isVisible(), 'and More info is turned on, where the answer is');
if (outDir) await page.screenshot({ path: `${outDir}/limit-blocked.png` });

// ---- two taps to remove
const first = row.locator('li').first();
const name = (await first.locator('a').innerText()).trim();
const removeBtn = first.getByRole('button');
await removeBtn.click();
await page.waitForTimeout(200);
check((await removeBtn.innerText()).trim() === 'Remove?', 'one tap arms the Remove button rather than removing');
check(await row.locator('li').count() === 4, 'nothing is gone yet');
if (outDir) await page.screenshot({ path: `${outDir}/limit-armed.png` });
await removeBtn.click();
await page.waitForTimeout(2500);
check(await row.locator('li').count() === 3, `the second tap removes it (${name})`);
check((await row.locator('div').first().innerText()).replace(/\s+/g, ' ').includes('3 of 4'), 'and the count follows');

// ---- with room again, the picker opens
check(await opensPicker(chooseBtn), 'with room in the folder the picker opens as usual');
check(!(await dialog.isVisible().catch(() => false)), 'and no message stands in the way');

// ---- every imported photo has a thumbnail, rendered by Sidecar (Immich has never seen these)
const shown = await row.locator('li img').all();
check(shown.length === 3, `every imported photo has a thumbnail (${shown.length})`);
for (const img of shown) {
	const [src, loaded] = [await img.getAttribute('src'), await img.evaluate((el) => el.complete && el.naturalWidth > 0)];
	check(loaded && src.endsWith('/thumb'), `  ${src?.slice(-52)} rendered`);
}

// ---- full again, but this time something in it has been exported
// photos[4], not one of the four already in there: re-importing one of those is a duplicate and adds
// nothing, which is correct behaviour and would leave the folder short of full.
const spare = photos[4] ?? photos[0];
const fill = await page.request.post(`${base}/api/intake?to=folder`, {
	multipart: { file: { name: spare, mimeType: 'image/x-canon-cr2', buffer: readFileSync(join(photoDir, spare)) } }
});
check(fill.ok(), `the folder takes one more, back to full (${spare})`);
check((await (await page.request.get(`${base}/api/intake/folder`)).json()).full, 'and reports itself full');
const list = (await (await page.request.get(`${base}/api/intake/folder`)).json()).items;
const victim = list[list.length - 1];
const edit = await (await page.request.get(`${base}/api/assets/${victim.assetKey}/edit`)).json();
const job = await (await page.request.post(`${base}/api/exports`, {
	data: { assetKey: victim.assetKey, editEtag: edit.etag, destination: 'device' }
})).json();
for (let i = 0; i < 60; i++) {
	const st = await (await page.request.get(`${base}/api/exports/${job.jobId}`)).json();
	if (st.finished) break;
	await page.waitForTimeout(1000);
}
await page.goto(base, { waitUntil: 'networkidle' });
await page.waitForTimeout(600);
// More info is off after a reload, and its line isn't rendered at all until it is on.
await page.getByRole('button', { name: 'More info' }).click();
await page.waitForTimeout(200);
check((await row.locator('li', { hasText: victim.fileName }).textContent()).includes('exported'), `the exported photo is marked under More info (${victim.fileName})`);
check(!(await opensPicker(chooseBtn)), 'a full folder still stops the picker');
check((await dialog.innerText()).includes(victim.fileName), `the message names what would go (${victim.fileName})`);
if (outDir) await page.screenshot({ path: `${outDir}/limit-exported.png` });
check(await opensPicker(page.getByRole('button', { name: 'Remove it and import' })), 'and taking that way forward opens the picker');

await browser.close();
console.log(failures.length ? `\n${failures.length} failed:\n  ${failures.join('\n  ')}` : '\nall ok');
process.exit(failures.length ? 1 : 0);
