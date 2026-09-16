// SPDX-License-Identifier: Apache-2.0
// Export sheet smoke test: change exposure, export at 1024 px, wait for the result.
// Usage: node scripts/export-smoke.mjs <base> <assetKey> <outDir>
import { chromium } from 'playwright';

const [base, key, outDir] = process.argv.slice(2);
const browser = await chromium.launch({
	headless: true,
	args: ['--no-sandbox', '--enable-unsafe-webgpu', '--enable-features=Vulkan', '--use-vulkan=native', '--use-angle=vulkan', '--disable-vulkan-surface', '--ignore-gpu-blocklist']
});
const page = await (await browser.newContext({ viewport: { width: 393, height: 852 }, deviceScaleFactor: 3, isMobile: true, hasTouch: true })).newPage();
page.on('pageerror', (e) => console.log(`pageerror: ${e.message}`));
await page.goto(`${base}/edit/${key}`, { waitUntil: 'networkidle' });
const slider = page.locator('#exposure:visible');
await slider.waitFor({ timeout: 60_000 });
await slider.focus();
for (let i = 0; i < 5; i++) await page.keyboard.press('Shift+ArrowLeft');
console.log(`exposure now ${await slider.getAttribute('aria-valuetext')}`);
await page.getByRole('button', { name: 'Export', exact: true }).click();
await page.getByRole('button', { name: '1024 px' }).click();
await page.getByRole('dialog').getByRole('button', { name: 'Export', exact: true }).click();
await page.waitForTimeout(1500);
await page.screenshot({ path: `${outDir}/export-running.png` });
await page.getByText(/Exported|Saved to Immich|Already exported/).waitFor({ timeout: 120_000 });
console.log(`result: ${(await page.getByRole('dialog').innerText()).replace(/\s+/g, ' ')}`);
await page.screenshot({ path: `${outDir}/export-done.png` });
await browser.close();
process.exit(0);
