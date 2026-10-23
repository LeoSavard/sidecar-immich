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
await page.waitForTimeout(3000);
check(!(await page.getByText('This browser has no WebGPU').count()), 'WebGPU is on');

const stored = await (await page.request.get(`${base}/api/assets/${key}/edit`)).json();
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

const offer = page.getByRole('button', { name: "Use today's rendering" });
check(await offer.count() === 1, 'the offer is made');
check((await page.locator('div', { has: offer }).first().innerText()).includes('picture will change'), 'and says the picture will change');
if (outDir) await page.screenshot({ path: `${outDir}/rendering-offer.png` });

await offer.click();
await page.waitForTimeout(2500);
const after = await stops();
const lift = after - before;
check(Math.abs(lift - 1.15) < 0.1, `taking it up lifts the picture by the baseline (${lift.toFixed(2)} stops, expected 1.15)`);
check(await offer.count() === 0, 'and the offer goes away, having been taken');
if (outDir) await page.screenshot({ path: `${outDir}/rendering-updated.png` });

// Undo: an ordinary edit, so it comes back.
await page.locator('canvas').first().press('Control+z');
await page.waitForTimeout(2000);
check(Math.abs((await stops()) - before) < 0.05, 'Undo puts the old rendering back');
check(await page.getByRole('button', { name: "Use today's rendering" }).count() === 1, 'and the offer returns with it');

// ---- the notice's own claim: what you have now stays in History
const versionsBefore = (await (await page.request.get(`${base}/api/assets/${key}/history`)).json()).nodes.length;
await page.locator('canvas').first().press('Control+Shift+z'); // redo, back to today's rendering
await page.waitForTimeout(1500);
check(Math.abs((await stops()) - after) < 0.05, 'Redo brings it forward again');
await page.getByRole('button', { name: 'Save' }).click();
await page.waitForTimeout(3000);
const history = await (await page.request.get(`${base}/api/assets/${key}/history`)).json();
check(history.nodes.length === versionsBefore + 1, `saving files it as a version (${versionsBefore} → ${history.nodes.length})`);
const head = history.nodes.find((n) => n.id === history.head);
const parent = history.nodes.find((n) => n.id === head.parent);
check(!!parent, 'with the one before it still there');
const saved = await (await page.request.get(`${base}/api/assets/${key}/edit`)).json();
check(saved.edit.exposure.baselineEv > 1, `and the saved edit carries the baseline (${saved.edit.exposure.baselineEv})`);
check(saved.edit.exposure.ev === stored.edit.exposure.ev, `while Exposure is untouched (${saved.edit.exposure.ev} EV)`);

// Put the fixture back the way it was found, so the next run starts where this one did.
await page.request.post(`${base}/api/assets/${key}/history/checkout`, { data: { node: head.parent } });

await browser.close();
console.log(failures.length ? `\n${failures.length} failed:\n  ${failures.join('\n  ')}` : '\nall ok');
process.exit(failures.length ? 1 : 0);
