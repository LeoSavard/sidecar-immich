// SPDX-License-Identifier: Apache-2.0
import { chromium } from 'playwright';
const [base, key, out] = process.argv.slice(2);
const browser = await chromium.launch({ headless: true, args: ['--no-sandbox', '--enable-unsafe-webgpu', '--enable-features=Vulkan', '--use-vulkan=native', '--use-angle=vulkan', '--disable-vulkan-surface', '--ignore-gpu-blocklist'] });
const page = await (await browser.newContext({ viewport: { width: 393, height: 852 }, deviceScaleFactor: 3, isMobile: true, hasTouch: true })).newPage();
page.on('pageerror', (e) => console.log(`pageerror: ${e.message}`));
await page.goto(`${base}/edit/${key}`, { waitUntil: 'networkidle' });
await page.getByRole('button', { name: 'Detail', exact: true }).click();
await page.getByAltText('Detail at 100 %').waitFor({ timeout: 60_000 });
await page.waitForTimeout(800);
await page.screenshot({ path: out });
console.log('detail section rendered');
await browser.close();
process.exit(0);
