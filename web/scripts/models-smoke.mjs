// SPDX-License-Identifier: Apache-2.0
// Settings → Models (DESIGN.md §12, "Models"): the models Sidecar can run, each with what it enables, its licence
// and whether this server has it — from the API, and as Settings shows them.
//
//   node web/scripts/models-smoke.mjs <base-url> [--expect-installed]
//
// Run inside the dev container with --network host, against `./dev/serve`; pass --expect-installed where the
// server runs `sidecar-ml` with its weights (the dev server does).
import { chromium } from 'playwright';

const base = process.argv[2];
const installed = process.argv.includes('--expect-installed');
if (!base) throw new Error('usage: models-smoke.mjs <base-url> [--expect-installed]');
const browser = await chromium.launch({ headless: true, args: ['--no-sandbox'] });
const steps = [];
const check = (ok, what) => {
	if (!ok) throw new Error(`failed: ${what}`);
	steps.push(what);
};
try {
	const page = await browser.newPage();
	const list = await (await page.request.get(`${base}/api/models`)).json();
	console.log(JSON.stringify(list.map((m) => [m.id, m.licence, m.state])));
	check(['birefnet', 'multiclass', 'yunet', 'depth'].every((id) => list.some((m) => m.id === id)), 'the four shipped models are listed');
	check(list.every((m) => m.licence && m.enables), 'each with its licence and what it enables');
	if (installed) check(list.every((m) => m.state === 'installed'), 'and all on this server');
	await page.goto(`${base}/settings`, { waitUntil: 'networkidle' });
	const section = page.locator('section[aria-labelledby="models-heading"]');
	await section.getByText('Depth Anything V2 Small').waitFor({ timeout: 10000 });
	check((await section.locator('li').count()) === list.length, 'Settings lists them all');
	check(await section.getByText('The subject mask · MIT').isVisible(), 'with what each enables and its licence');
	console.log(steps.map((s) => `ok  ${s}`).join('\n'));
} finally {
	await browser.close();
}
