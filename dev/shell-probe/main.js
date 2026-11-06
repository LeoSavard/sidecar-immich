// SPDX-License-Identifier: Apache-2.0
// V2's shell experiment (docs/research/shell-experiment.md): Electron's main process. Opens one real window,
// loads a page, waits for it to publish a result on `window.__probe` or `window.__parity`, captures what the
// window actually shows, writes it all to --out, and quits.
//
//   electron main.js --url=<page> --out=<result.json> [--switches=a,b=c,...] [--parity-out=<dir>]
//
// --switches are Chromium command-line switches, appended before the app is ready: the app sets them, not
// the user, which is the whole point of shipping Chromium.
import { app, BrowserWindow } from 'electron';
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const arg = (name) => process.argv.find((a) => a.startsWith(`--${name}=`))?.slice(name.length + 3);
const url = arg('url');
const out = arg('out');
const parityOut = arg('parity-out');
const switches = (arg('switches') ?? '').split(',').filter(Boolean);
for (const s of switches) {
	const [k, v] = s.split('=');
	v === undefined ? app.commandLine.appendSwitch(k) : app.commandLine.appendSwitch(k, v);
}

const result = { electron: process.versions.electron, chrome: process.versions.chrome, switches, url };

app.whenReady().then(async () => {
	const gpu = await app.getGPUInfo('complete').catch((e) => ({ error: String(e) }));
	result.gpuFeatureStatus = app.getGPUFeatureStatus();
	result.gpuDevices = gpu.gpuDevice;
	result.gpuInfo = { vulkan: gpu.auxAttributes?.vulkanVersion ?? null, glRenderer: gpu.auxAttributes?.glRenderer ?? null };

	const win = new BrowserWindow({ width: 640, height: 480, show: true, backgroundColor: '#000000', webPreferences: { contextIsolation: true } });
	const console_ = [];
	win.webContents.on('console-message', (e) => console_.push(`${e.level}: ${e.message}`));
	await win.loadURL(url);
	const deadline = Date.now() + 180_000;
	let page = null;
	while (Date.now() < deadline) {
		page = await win.webContents.executeJavaScript('window.__probe ?? window.__parity ?? null');
		if (page) break;
		await new Promise((r) => setTimeout(r, 250));
	}
	result.timedOut = !page;
	result.console = console_.slice(-20);
	if (page?.images && parityOut) {
		mkdirSync(parityOut, { recursive: true });
		for (const [name, b64] of Object.entries(page.images)) writeFileSync(join(parityOut, `${name}.browser.f16`), Buffer.from(b64, 'base64'));
		result.parity = { adapter: page.adapter, error: page.error ?? null, renders: Object.keys(page.images).length };
	} else {
		result.page = page;
	}
	// What the window shows, read back from the compositor: the probe paints a known colour into its canvas.
	if (result.page?.paint) {
		await new Promise((r) => setTimeout(r, 500));
		const shot = await win.webContents.capturePage(result.page.paint.rect);
		const { width, height } = shot.getSize();
		const bmp = shot.toBitmap(); // BGRA
		const i = (Math.floor(height / 2) * width + Math.floor(width / 2)) * 4;
		result.presented = { width, height, rgb: [bmp[i + 2], bmp[i + 1], bmp[i]] };
	}
	writeFileSync(out, JSON.stringify(result, null, 2));
	app.exit(0);
});
