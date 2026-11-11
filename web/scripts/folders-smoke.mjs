// SPDX-License-Identifier: Apache-2.0
// Browsing folders end to end (DESIGN.md §3, "Browsing folders"): on the home page, a root chosen, folders walked
// down to a day, a photo opened by choosing it — and the editor opening it; the API refusing a path out of the
// root. Looking writes nothing: the folder holds the same files before and after listing it.
//
//   node web/scripts/folders-smoke.mjs <base-url> <root name> <folder path in it> <file name>
//
// Run inside the dev container with --network host, against `./dev/serve`, on a root that is a test copy: opening
// ingests the photo, and the editor may save beside it.
import { readdirSync } from 'node:fs';
import { chromium } from 'playwright';

const [base, rootName, dir, file] = process.argv.slice(2);
if (!file) throw new Error('usage: folders-smoke.mjs <base-url> <root name> <folder> <file>');
const browser = await chromium.launch({ headless: true, args: ['--no-sandbox', '--enable-unsafe-webgpu', '--enable-features=Vulkan', '--use-vulkan=native', '--use-angle=vulkan', '--disable-vulkan-surface', '--ignore-gpu-blocklist'] });
const steps = [];
const check = (ok, what) => {
	if (!ok) throw new Error(`failed: ${what}`);
	steps.push(what);
};

try {
	const page = await browser.newPage({ viewport: { width: 1400, height: 1000 } });
	page.on('pageerror', (e) => console.error('[pageerror]', e.message));
	const roots = await (await page.request.get(`${base}/api/folders`)).json();
	const root = roots.find((r) => r.name === rootName);
	check(!!root, `the root "${rootName}" is browsable`);
	const list = await (await page.request.get(`${base}/api/folders/${root.index}/list?dir=${encodeURIComponent(dir)}`)).json();
	const mountDir = process.env.ROOT_MOUNT ? `${process.env.ROOT_MOUNT}/${dir}` : null;
	const before = mountDir ? readdirSync(mountDir).sort().join() : null;
	check(list.files.some((f) => f.name === file), `listing ${dir} shows ${file}`);
	check((await page.request.get(`${base}/api/folders/${root.index}/list?dir=..`)).status() === 404, 'a path out of the root is refused');
	check((await page.request.post(`${base}/api/folders/${root.index}/open`, { data: { path: '../../etc/passwd' } })).status() === 404, 'and cannot be opened');

	await page.goto(`${base}/`, { waitUntil: 'networkidle' });
	await page.getByRole('button', { name: rootName, exact: true }).click();
	for (const part of dir.split('/')) await page.getByRole('button', { name: `📁 ${part}` }).click();
	await page.getByRole('button', { name: new RegExp(`^${file.replace('.', '\\.')}`) }).waitFor({ timeout: 10000 });
	if (mountDir) check(readdirSync(mountDir).sort().join() === before, 'looking wrote nothing into the folder');
	await Promise.all([page.waitForURL(/\/edit\/[0-9a-f]{40}$/, { timeout: 60000 }), page.getByRole('button', { name: new RegExp(`^${file.replace('.', '\\.')}`) }).click()]);
	await page.getByRole('slider', { name: 'Exposure' }).first().waitFor({ state: 'attached', timeout: 60000 });
	check(await page.getByText(file).first().isVisible(), `choosing ${file} opens it in the editor`);
	console.log(steps.map((s) => `ok  ${s}`).join('\n'));
} finally {
	await browser.close();
}
