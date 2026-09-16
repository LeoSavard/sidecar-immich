// SPDX-License-Identifier: Apache-2.0
// Renders the parity fixtures in headless Chromium's WebGPU and writes <case>.browser.f16 next to the native renders.
// Usage: node scripts/parity-browser.mjs <outDir> [--swiftshader]
import { chromium } from 'playwright';
import { spawn } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const outDir = process.argv[2] ?? '../testdata/parity-out';
const swiftshader = process.argv.includes('--swiftshader');
const port = 4179;

const server = spawn(process.execPath, ['node_modules/vite/bin/vite.js', 'preview', '--host', '127.0.0.1', '--port', String(port), '--strictPort'], { stdio: 'ignore' });
let exited = false;
server.on('exit', () => (exited = true));
for (let i = 0; ; i++) {
	if (exited) throw new Error('vite preview exited');
	if (i > 300) throw new Error('vite preview did not start');
	try {
		if ((await fetch(`http://127.0.0.1:${port}/parity/index.json`)).ok) break;
	} catch {}
	await new Promise((r) => setTimeout(r, 100));
}

const gpuArgs = swiftshader
	? ['--enable-unsafe-webgpu', '--use-webgpu-adapter=swiftshader', '--enable-unsafe-swiftshader']
	: ['--enable-unsafe-webgpu', '--enable-features=Vulkan', '--use-vulkan=native', '--use-angle=vulkan', '--disable-vulkan-surface', '--ignore-gpu-blocklist'];
const browser = await chromium.launch({ headless: true, args: ['--no-sandbox', ...gpuArgs] });
try {
	const page = await browser.newPage();
	page.on('console', (m) => m.type() === 'error' && console.error('[browser]', m.text()));
	await page.goto(`http://127.0.0.1:${port}/debug/parity`);
	await page.waitForFunction(() => window.__parity, null, { timeout: 120_000 });
	const result = await page.evaluate(() => window.__parity);
	console.log(`browser adapter: ${result.adapter || '(unknown)'}`);
	if (result.error) throw new Error(result.error);
	mkdirSync(outDir, { recursive: true });
	for (const [name, b64] of Object.entries(result.images)) {
		writeFileSync(join(outDir, `${name}.browser.f16`), Buffer.from(b64, 'base64'));
	}
	console.log(`wrote ${Object.keys(result.images).length} browser renders`);
} finally {
	await browser.close();
	server.kill();
}
process.exit(0);
