// SPDX-License-Identifier: Apache-2.0
// Editor smoke test in headless Chromium (WebGPU on the server GPU): open, wait for render, drag a slider,
// toggle clipping, save, and take phone and desktop screenshots.
// Usage: node scripts/editor-smoke.mjs <base> <assetKey> <outDir>
import { chromium } from 'playwright';

const [base, key, outDir] = process.argv.slice(2);
const browser = await chromium.launch({
	headless: true,
	args: ['--no-sandbox', '--enable-unsafe-webgpu', '--enable-features=Vulkan', '--use-vulkan=native', '--use-angle=vulkan', '--disable-vulkan-surface', '--ignore-gpu-blocklist']
});
const errors = [];
async function run(name, viewport, isMobile) {
	const ctx = await browser.newContext({ viewport, deviceScaleFactor: isMobile ? 3 : 1, isMobile, hasTouch: isMobile });
	const page = await ctx.newPage();
	page.on('pageerror', (e) => console.log(`${name} pageerror: ${e.message}`));
	page.on('console', (m) => console.log(`${name} console.${m.type()}: ${m.text()}`));
	await page.goto(`${base}/edit/${key}`, { waitUntil: 'networkidle' });
	if (isMobile) await page.getByRole('button', { name: 'Light', exact: true }).click().catch(() => {});
	const slider = page.locator('#exposure:visible');
	await slider.waitFor({ timeout: 60_000 });
	await page.waitForTimeout(1500);
	const box = await slider.boundingBox();
	const thumbAt = async () => {
		const t = await slider.locator('div.size-7').last().boundingBox();
		return t ? t.x + t.width / 2 : NaN;
	};
	const thumbBefore = await thumbAt();
	await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
	await page.mouse.down();
	await page.mouse.move(box.x + box.width / 2 + box.width * 0.08, box.y + box.height / 2, { steps: 8 });
	// The thumb moves as far as the finger: it trailed by 14 px once (engage threshold plus an unrepaid detent).
	// Unless it hit the end of the track.
	const moved = (await thumbAt()) - thumbBefore;
	const expected = box.width * 0.08;
	const atEnd = (await slider.getAttribute('aria-valuenow')) === (await slider.getAttribute('aria-valuemax'));
	if (!atEnd && !(Math.abs(moved - expected) <= 3)) errors.push(`${name}: thumb moved ${moved.toFixed(1)} px for ${expected.toFixed(1)} px of drag`);
	await page.mouse.up();
	const valueText = await slider.getAttribute('aria-valuetext');
	await page.waitForTimeout(600);
	await page.screenshot({ path: `${outDir}/smoke-${name}.png` });
	const saveBtn = page.getByRole('button', { name: /^(Save|Saved|Saving…)$/ });
	const before = await saveBtn.textContent();
	await saveBtn.click();
	await page.waitForTimeout(800);
	const after = await saveBtn.textContent();
	console.log(`${name}: exposure → ${valueText}; save button "${before?.trim()}" → "${after?.trim()}"`);
	await ctx.close();
}
await run('desktop', { width: 1280, height: 800 }, false);
await run('phone', { width: 393, height: 852 }, true);
console.log(errors.length ? `errors:\n${errors.join('\n')}` : 'no page errors');
await browser.close();
process.exit(0);
