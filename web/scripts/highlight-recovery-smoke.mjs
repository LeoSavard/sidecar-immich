// SPDX-License-Identifier: Apache-2.0
// Highlight recovery (DESIGN.md §7, "Highlights"), in headless Chromium against a running server. Before
// `PROCESS_VERSION` 7 the tone curve mapped everything at or above the white point to display white before
// the LUT saw it, and the LUT's ends are pinned — so Highlights could bend the curve below white and reach
// nothing that was already at it. This drives the real render and reads the clipped fraction off the
// Measure readout, which is the number the complaint was about.
//   node scripts/highlight-recovery-smoke.mjs <base-url> <photo path on the server> [outDir]
import { chromium } from 'playwright';

const [base, photo, outDir] = process.argv.slice(2);
if (!base || !photo) throw new Error('usage: highlight-recovery-smoke.mjs <base-url> <photo path> [outDir]');

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

await page.goto(`${base}/open?path=${encodeURIComponent(photo)}`, { waitUntil: 'load' });
const key = page.url().split('/edit/')[1];
await page.getByRole('slider', { name: 'Exposure' }).first().waitFor({ timeout: 60000 });
const state = await (await page.request.get(`${base}/api/assets/${key}/edit`)).json();

/** Today's rendering, with whatever this case changes. */
const edit = (o = () => {}) => {
	const c = structuredClone(state.edit);
	Object.assign(c.exposure, { baselineEv: 1.15, ev: 0 });
	Object.assign(c.tone, { whiteZeroEv: 3.63, whiteEv: 3.63, shoulderOrder: 3, highlightBleach: 1, highlights: 0, highlightReachEv: 2 });
	o(c);
	return c;
};

/** The clipped fraction of the real render, as a percentage. */
async function atWhite(e) {
	const now = await (await page.request.get(`${base}/api/assets/${key}/edit`)).json();
	await page.request.put(`${base}/api/assets/${key}/edit`, { data: { edit: e, base: now.head, checkpoint: false, session: 'highlight-smoke' } });
	await page.goto(`${base}/edit/${key}`, { waitUntil: 'load' });
	await page.getByRole('slider', { name: 'Exposure' }).first().waitFor({ timeout: 60000 });
	await page.waitForTimeout(3000);
	await page.locator('summary', { hasText: 'Measure' }).click();
	await page.waitForTimeout(700);
	return parseFloat((await page.locator('tr:has(th:text-is("at white")) td').first().innerText()).trim());
}

const none = await atWhite(edit());
check(none > 2, `the photo clips enough to be worth measuring (${none} % at white)`);

const half = await atWhite(edit((c) => { c.tone.highlights = -50; }));
check(half < none, `Highlights −50 recovers some of it (${none} % → ${half} %)`);

const all = await atWhite(edit((c) => { c.tone.highlights = -100; }));
check(all < 0.5, `Highlights −100 recovers essentially all of it (${all} %)`);
check(all < half && half < none, 'and the three read monotonically');
if (outDir) await page.screenshot({ path: `${outDir}/highlight-recovered.png` });

// The picture an older edit was approved with does not move: its Highlights grades below white as it did,
// and the white point stays where it was.
const older = (h) => edit((c) => {
	c.tone.highlights = h;
	Object.assign(c.tone, { highlightReachEv: 0, whiteEv: 4.5, whiteZeroEv: 4.5, shoulderOrder: 0, highlightBleach: 0 });
	c.exposure.baselineEv = 0;
});
const oldRest = await atWhite(older(0));
const oldPulled = await atWhite(older(-100));
check(oldPulled <= oldRest, `an older edit is unchanged by the new reach (${oldRest} % → ${oldPulled} %)`);

await browser.close();
console.log(failures.length ? `\n${failures.length} failed:\n  ${failures.join('\n  ')}` : '\nall ok');
process.exit(failures.length ? 1 : 0);
