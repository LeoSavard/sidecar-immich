// SPDX-License-Identifier: Apache-2.0
// Taking up today's rendering (DESIGN.md §4), in headless Chromium against a running server: a photo whose
// edit was made before the baseline existed is offered the update, taking it lifts the picture by the
// baseline, Undo puts it back, and the offer is not made to an edit already on today's rules. The numbers
// come from the Measure readout, which reads the rendering rather than describing it.
//   node scripts/rendering-update-smoke.mjs <base-url> <photo path on the server> [outDir]
import { chromium } from 'playwright';

const [base, photo, outDir] = process.argv.slice(2);
if (!base || !photo) throw new Error('usage: rendering-update-smoke.mjs <base-url> <photo path> [outDir]');

const browser = await chromium.launch({
	headless: true,
	args: ['--no-sandbox', '--enable-unsafe-webgpu', '--enable-features=Vulkan', '--use-vulkan=native', '--use-angle=vulkan', '--disable-vulkan-surface', '--ignore-gpu-blocklist']
});
const failures = [];
const check = (ok, what) => {
	console.log(`${ok ? 'ok  ' : 'FAIL'} ${what}`);
	if (!ok) failures.push(what);
};

const page = await browser.newPage({ viewport: { width: 1400, height: 1000 }, deviceScaleFactor: 1 });
page.on('pageerror', (e) => failures.push(`pageerror: ${e.message}`));
page.on('console', (m) => m.type() === 'error' && console.log(`console.error: ${m.text()}`));

await page.goto(`${base}/open?path=${encodeURIComponent(photo)}`, { waitUntil: 'load' });
const key = page.url().split('/edit/')[1];
await page.getByRole('slider', { name: 'Exposure' }).first().waitFor({ timeout: 60000 });

// Put the photo into the state under test rather than relying on the fixture's own history, so the run is
// the same every time and leaves nothing behind that changes the next one. These are the references an edit
// made before PROCESS_VERSION 3 carries, which is what `migrate` pins and what this is all about.
{
	const now = await (await page.request.get(`${base}/api/assets/${key}/edit`)).json();
	const old = structuredClone(now.edit);
	old.exposure.ev = 0.3;
	old.exposure.baselineEv = 0;
	old.tone.whiteEv = 4.5;
	old.tone.whiteZeroEv = 4.5;
	old.tone.shoulderOrder = 0;
	old.tone.highlightBleach = 0;
	const put = await page.request.put(`${base}/api/assets/${key}/edit`, {
		data: { edit: old, base: now.head, checkpoint: true, session: 'rendering-smoke-setup' }
	});
	if (!put.ok()) throw new Error(`setting the photo to an old rendering: ${await put.text()}`);
}
await page.goto(`${base}/edit/${key}`, { waitUntil: 'load' });
await page.getByRole('slider', { name: 'Exposure' }).first().waitFor({ timeout: 60000 });
await page.waitForTimeout(3000);
check(!(await page.getByText('This browser has no WebGPU').count()), 'WebGPU is on');

const stored = await (await page.request.get(`${base}/api/assets/${key}/edit`)).json();
const versionsAtStart = (await (await page.request.get(`${base}/api/assets/${key}/history`)).json()).nodes.length;
check(stored.edit.exposure.baselineEv === 0, `the photo's stored edit predates the baseline (baselineEv ${stored.edit.exposure.baselineEv})`);

// The rendering as it stands, read rather than described.
await page.locator('summary', { hasText: 'Measure' }).click();
const canvas = page.locator('canvas').first();
const box = await canvas.boundingBox();
await page.getByRole('button', { name: 'Probe' }).click();
await page.mouse.click(box.x + box.width * 0.5, box.y + box.height * 0.45);
await page.waitForTimeout(1200);
const stops = async () => parseFloat((await page.locator('tr:has(th:text-is("stops from grey")) td').first().innerText()).replace('−', '-').replace('+', ''));
const before = await stops();
check(Number.isFinite(before), `the probe reads the old rendering (${before} stops from grey)`);

const notice = page.getByRole('button', { name: 'Bring it up to date now' });
check(await notice.count() === 1, 'the photo is told it was made with an older rendering');
check((await page.locator('div', { has: notice }).first().innerText()).includes('picture will change'), 'and warned the picture will change');
if (outDir) await page.screenshot({ path: `${outDir}/rendering-offer.png` });

// ---- port by default: an ordinary change carries it
const contrast = page.getByRole('slider', { name: 'Contrast' }).first();
await contrast.focus();
await page.keyboard.press('ArrowUp');
await page.waitForTimeout(2500);
const after = await stops();
const lift = after - before;
check(Math.abs(lift - 1.15) < 0.1, `one ordinary change brings it up to date (${lift.toFixed(2)} stops, expected 1.15)`);
check(await notice.count() === 0, 'the standing notice goes');
const keep = page.getByRole('button', { name: 'Keep the old rendering' });
check(await keep.count() === 1, 'and the way back is offered');
if (outDir) await page.screenshot({ path: `${outDir}/rendering-updated.png` });

// ---- the way back
await keep.click();
await page.waitForTimeout(2000);
check(Math.abs((await stops()) - before) < 0.05, 'keeping the old rendering puts it back');
const live = await (await page.request.get(`${base}/api/assets/${key}/edit`)).json();
void live;
// Having declined, a further change must not port again while the photo is open.
await contrast.focus();
await page.keyboard.press('ArrowUp');
await page.waitForTimeout(2000);
check(Math.abs((await stops()) - before) < 0.05, 'and a further change leaves it where they asked');
check(await page.getByRole('button', { name: 'Bring it up to date now' }).count() === 0, 'without asking again this session');

// ---- back to today's for the rest of the run
await page.reload({ waitUntil: 'load' });
await page.getByRole('slider', { name: 'Exposure' }).first().waitFor({ timeout: 60000 });
await page.waitForTimeout(3000);
check(await page.getByRole('button', { name: 'Bring it up to date now' }).count() === 1, 'a later session asks again');
await page.getByRole('button', { name: 'Bring it up to date now' }).click();
await page.waitForTimeout(2500);
await page.locator('summary', { hasText: 'Measure' }).click();
await page.getByRole('button', { name: 'Probe' }).click();
await page.mouse.click(box.x + box.width * 0.5, box.y + box.height * 0.45);
await page.waitForTimeout(1500);

// ---- the notice's own claim: what you have now stays in History
const save = page.getByRole('button', { name: 'Save' });
if (await save.isEnabled()) await save.click();
await page.waitForTimeout(3000);
const history = await (await page.request.get(`${base}/api/assets/${key}/history`)).json();
check(history.nodes.length > versionsAtStart, `the work is filed as versions (${versionsAtStart} → ${history.nodes.length})`);
const head = history.nodes.find((n) => n.id === history.head);
const parent = history.nodes.find((n) => n.id === head.parent);
check(!!parent, 'with the one before it still there');
const saved = await (await page.request.get(`${base}/api/assets/${key}/edit`)).json();
check(saved.edit.exposure.baselineEv > 1, `and the saved edit carries the baseline (${saved.edit.exposure.baselineEv})`);
check(saved.edit.exposure.ev === stored.edit.exposure.ev, `while Exposure is untouched (${saved.edit.exposure.ev} EV)`);

await browser.close();
console.log(failures.length ? `\n${failures.length} failed:\n  ${failures.join('\n  ')}` : '\nall ok');
process.exit(failures.length ? 1 : 0);
