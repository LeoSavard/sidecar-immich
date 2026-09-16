// SPDX-License-Identifier: Apache-2.0
// Smoke test: open a URL in headless Chromium with WebGPU, wait until the page settles, save a screenshot.
// Usage: node scripts/screenshot.mjs <url> <out.png> [width] [height]
import { chromium } from 'playwright';

const [url, out, width = '1280', height = '800'] = process.argv.slice(2);
const browser = await chromium.launch({
	headless: true,
	args: ['--no-sandbox', '--enable-unsafe-webgpu', '--enable-features=Vulkan', '--use-vulkan=native', '--use-angle=vulkan', '--disable-vulkan-surface', '--ignore-gpu-blocklist']
});
const page = await browser.newPage({ viewport: { width: Number(width), height: Number(height) } });
page.on('console', (m) => console.log(`[${m.type()}] ${m.text()}`));
page.on('pageerror', (e) => console.log(`[pageerror] ${e.message}`));
const resp = await page.goto(url, { waitUntil: 'networkidle', timeout: 120_000 });
console.log(`landed on ${page.url()} (${resp?.status()})`);
await page.waitForTimeout(3000);
await page.screenshot({ path: out });
await browser.close();
process.exit(0);
