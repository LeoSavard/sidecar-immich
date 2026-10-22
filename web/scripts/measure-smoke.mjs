// SPDX-License-Identifier: Apache-2.0
// The Measure readout (DESIGN.md §8), in headless Chromium against a running server: probe a place in the
// photo, read it, flip to the compared version, and check that the second column arrives with different
// numbers — and that lifting the exposure moves "at white" and the probe the way it must.
//   node scripts/measure-smoke.mjs <base-url> <photo path on the server> [outDir]
import { chromium } from 'playwright';

const [base, photo, outDir] = process.argv.slice(2);
if (!base || !photo) throw new Error('usage: measure-smoke.mjs <base-url> <photo path> [outDir]');

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
// From the original, so the numbers don't depend on what an earlier run left behind.
await page.request.post(`${base}/api/assets/${key}/history/checkout`, { data: { node: 'original' } });
await page.goto(`${base}/edit/${key}`, { waitUntil: 'load' });
await page.getByRole('slider', { name: 'Exposure' }).first().waitFor({ timeout: 60000 });
await page.waitForTimeout(3000);

const canvas = page.locator('canvas').first();
check(!(await page.getByText('This browser has no WebGPU').count()), 'WebGPU is on (the readout needs the GPU passes)');

await page.locator('summary', { hasText: 'Measure' }).click();
const row = async (name) => (await page.locator(`tr:has(th:text-is("${name}")) td`).allInnerTexts()).map((t) => t.trim());
check((await row('at white')).length === 1, 'one column before anything is compared');
const atWhite = async () => parseFloat((await row('at white'))[0]);
const firstWhite = await atWhite();
check(Number.isFinite(firstWhite), `at white reads a number (${firstWhite} %)`);

// ---- the probe
const box = await canvas.boundingBox();
await page.getByRole('button', { name: 'Probe' }).click();
await page.mouse.click(box.x + box.width * 0.5, box.y + box.height * 0.5);
await page.waitForTimeout(1200);
const probe255 = async () => (await row('probe, 0…255'))[0];
const stops = async () => (await row('stops from grey'))[0];
const first255 = await probe255();
check(/^\d+ \d+ \d+$/.test(first255 ?? ''), `the probe reads a triplet (${first255})`);
check(/^[+−]\d+\.\d\d$/.test((await stops()) ?? ''), `and its place in stops (${await stops()})`);
if (outDir) await page.screenshot({ path: `${outDir}/measure-probed.png` });

// ---- the numbers have to move the right way
const exposure = page.getByRole('slider', { name: 'Exposure' }).first();
await exposure.focus();
// Arrow steps are 0.01 EV ×10, Shift another ×10: +0.30 EV in all, which is plenty to move both numbers.
for (let i = 0; i < 20; i++) await page.keyboard.press('ArrowUp');
await page.keyboard.press('Shift+ArrowUp');
await page.waitForTimeout(1500);
const brighter = await probe255();
const lift = (s) => parseFloat(s.replace('−', '-').replace('+', ''));
const before255 = first255.split(' ').map(Number);
const after255 = brighter.split(' ').map(Number);
check(after255.every((v, i) => v >= before255[i]), `lifting the exposure raises the probe (${first255} → ${brighter})`);
check((await atWhite()) >= firstWhite, `and does not lower "at white" (${firstWhite} → ${await atWhite()} %)`);
const stopsUp = lift(await stops());

// ---- the flip brings the second column
await page.locator('canvas').first().press('\\');
await page.waitForTimeout(1500);
const cols = await row('at white');
check(cols.length === 2, `flipping to the compared version puts both columns on screen (${cols.join(' | ')})`);
const headers = (await page.locator('thead th').allInnerTexts()).map((t) => t.trim()).filter(Boolean);
check(headers.length === 2 && headers[0] !== headers[1], `each column is named (${headers.join(' | ')})`);
const shownStops = lift((await row('stops from grey'))[0]);
check(Math.abs(shownStops - stopsUp) > 0.5, `the compared version reads differently at the same place (${shownStops} vs ${stopsUp} stops)`);
if (outDir) await page.screenshot({ path: `${outDir}/measure-compared.png` });

// ---- the text that gets handed to someone else
await page.locator('canvas').first().press('\\');
await page.waitForTimeout(800);
await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
await page.getByRole('button', { name: 'Copy' }).click();
const text = await page.evaluate(() => navigator.clipboard.readText());
console.log('--- copied ---\n' + text + '\n---');
check(text.includes('at white') && text.includes('stops from grey') && text.split('\n').length >= 7, 'Copy puts the whole table on the clipboard');

await browser.close();
console.log(failures.length ? `\n${failures.length} failed:\n  ${failures.join('\n  ')}` : '\nall ok');
process.exit(failures.length ? 1 : 0);
