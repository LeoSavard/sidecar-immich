// SPDX-License-Identifier: Apache-2.0
// A RAW decoded on the phone against the same RAW decoded by the server (docs/MOBILE.md, stage 2): the colour
// model the phone's LibRaw reads must be the one the worker gives the file, and the proxy — LibRaw in wasm, the
// ingest shaders in WebGPU — the server's proxy at the same size, sample for sample, within what two GPUs' float
// arithmetic allows. The phone's way in is the real one: chosen on the home page, imported, and developed by the editor.
//
// Usage: node scripts/raw-parity.mjs <sidecar url> <RAW file>… [--swiftshader] [--dump <dir>]   (CHROMIUM_PATH to pick a browser)
// --dump writes both proxies as raw RGBA16F, <name>.phone.f16 and <name>.server.f16, for a closer look.
// Needs a running Sidecar with the module and an editing folder to import into, and a worker with a GPU (the
// server's proxy must come from its GPU ingest, as the phone's does).
import { chromium } from 'playwright';
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { basename } from 'node:path';

const args = process.argv.slice(2);
const swiftshader = args.includes('--swiftshader');
const dumpAt = args.indexOf('--dump');
const dump = dumpAt >= 0 ? args.splice(dumpAt, 2)[1] : null;
if (dump) mkdirSync(dump, { recursive: true });
const [url, ...files] = args.filter((a) => !a.startsWith('--'));

const gpuArgs = swiftshader
	? ['--enable-unsafe-webgpu', '--use-webgpu-adapter=swiftshader', '--enable-unsafe-swiftshader']
	: ['--enable-unsafe-webgpu', '--enable-features=Vulkan', '--use-vulkan=native', '--use-angle=vulkan', '--disable-vulkan-surface', '--ignore-gpu-blocklist'];
const browser = await chromium.launch({ headless: true, executablePath: process.env.CHROMIUM_PATH || undefined, args: ['--no-sandbox', ...gpuArgs] });
// A phone: `(pointer: coarse)`, which is what has the editor develop on the device. At 3× the editor asks for a
// 2,560-pixel proxy, as it does on an iPhone.
const context = await browser.newContext({ viewport: { width: 390, height: 844 }, deviceScaleFactor: 3, isMobile: true, hasTouch: true });
// SwiftShader can't present a WebGPU canvas headless (see mobile-smoke.mjs); nothing here needs the screen.
if (swiftshader)
	await context.addInitScript(() => {
		GPUCanvasContext.prototype.configure = function (c) {
			this.__config = c;
		};
		GPUCanvasContext.prototype.getCurrentTexture = function () {
			return this.__config.device.createTexture({ size: [this.canvas.width, this.canvas.height], format: this.__config.format, usage: GPUTextureUsage.RENDER_ATTACHMENT });
		};
	});

/** IEEE half bits → value. */
const half = (h) => {
	const e = (h >> 10) & 0x1f;
	const f = h & 0x3ff;
	const s = h & 0x8000 ? -1 : 1;
	return e === 0 ? s * f * 2 ** -24 : e === 31 ? (f ? NaN : s * Infinity) : s * (1 + f / 1024) * 2 ** (e - 15);
};

function close(a, b, path = '') {
	if (typeof a === 'number' && typeof b === 'number') return Math.abs(a - b) <= 1e-9 * Math.max(1, Math.abs(a)) ? [] : [`${path}: ${a} ≠ ${b}`];
	if (Array.isArray(a) && Array.isArray(b)) return a.length !== b.length ? [`${path}: lengths`] : a.flatMap((v, i) => close(v, b[i], `${path}[${i}]`));
	if (a && b && typeof a === 'object') return [...new Set([...Object.keys(a), ...Object.keys(b)])].flatMap((k) => close(a[k], b[k], `${path}.${k}`));
	return a === b ? [] : [`${path}: ${JSON.stringify(a)} ≠ ${JSON.stringify(b)}`];
}

let failed = 0;
for (const file of files) {
	const name = basename(file);
	// The phone's decode: chosen on the home page, imported, and developed on the phone by the editor.
	const page = await context.newPage();
	const errors = [];
	const notHere = [];
	page.on('pageerror', (e) => errors.push(String(e)));
	page.on('console', (m) => /Not developed on this device/.test(m.text()) && notHere.push(m.text()));
	await page.goto(`${url}/?inspect`);
	const started = Date.now();
	await page.locator('input[type=file]').setInputFiles(file);
	await page.waitForURL('**/edit/**', { timeout: 600_000 });
	const assetKey = page.url().split('/edit/')[1].split('?')[0];
	await page.waitForFunction(() => window.__sidecarInspect, null, { timeout: 600_000 }).catch(() => {});
	const seconds = (Date.now() - started) / 1e3;
	const phone = await page.evaluate(() => {
		const i = window.__sidecarInspect;
		if (!i) return {};
		const b = new Uint8Array(i.proxy.f16.buffer);
		let s = '';
		for (let k = 0; k < b.length; k += 0x8000) s += String.fromCharCode(...b.subarray(k, k + 0x8000));
		return { width: i.proxy.width, height: i.proxy.height, f16: btoa(s), camera: i.camera };
	});
	await page.close();
	if (!phone.f16) {
		// The editor declined and said why (an sRAW, a frame too large…) and asked the server: reported, not compared.
		console.log(`skip ${name}: not developed on the phone: ${notHere.join(' ') || 'no reason given'}`);
		continue;
	}
	// The server's decode of the same import: its metadata, and its proxy at the size the phone made.
	const meta = await (await fetch(`${url}/api/assets/${assetKey}/meta`)).json();
	const served = await fetch(`${url}/api/assets/${assetKey}/proxy?long_edge=${Math.max(phone.width, phone.height)}`);
	const [sw, sh] = [Number(served.headers.get('x-width')), Number(served.headers.get('x-height'))];
	const server = new Uint16Array(new Uint8Array(await served.arrayBuffer()).buffer);
	const mine = new Uint16Array(Buffer.from(phone.f16, 'base64').buffer.slice(0));
	if (dump) {
		writeFileSync(join(dump, `${name}.phone.f16`), new Uint8Array(mine.buffer));
		writeFileSync(join(dump, `${name}.server.f16`), new Uint8Array(server.buffer));
	}

	const colour = close(phone.camera, meta.info.color);
	const sized = phone.width === sw && phone.height === sh;
	let sum = 0;
	let signal = 0;
	let worst = 0;
	let clipDiff = 0;
	// Samples that aren't finite, on either side, and where both are (NaN's bits differ between GPUs, so where
	// matters, not what); the rest are compared.
	const nonFinite = { phone: 0, server: 0, both: 0 };
	const diffs = [];
	if (sized) {
		for (let i = 0; i < mine.length; i++) {
			const [a, b] = [half(mine[i]), half(server[i])];
			if (!Number.isFinite(a) || !Number.isFinite(b)) {
				if (!Number.isFinite(a)) nonFinite.phone++;
				if (!Number.isFinite(b)) nonFinite.server++;
				if (!Number.isFinite(a) && !Number.isFinite(b)) nonFinite.both++;
				continue;
			}
			if (i % 4 === 3) {
				clipDiff += Math.abs(a - b);
				continue;
			}
			const d = Math.abs(a - b);
			sum += d;
			signal += Math.abs(b);
			worst = Math.max(worst, d);
			if (i % 97 === 0) diffs.push(d);
		}
	}
	diffs.sort((x, y) => x - y);
	const relative = sized ? sum / signal : Infinity;
	const p99 = diffs.length ? diffs[Math.floor(diffs.length * 0.99)] : Infinity;
	const ok = colour.length === 0 && sized && relative < 1e-3 && errors.length === 0 && nonFinite.phone === nonFinite.both && nonFinite.server === nonFinite.both;
	if (!ok) failed++;
	console.log(
		`${ok ? 'ok  ' : 'FAIL'} ${name}: ${phone.width}×${phone.height} in ${seconds.toFixed(1)} s; colour model ${colour.length ? colour.slice(0, 3).join('; ') : 'identical'}; ` +
			`proxy ${sized ? `mean |Δ| ${(relative * 100).toFixed(4)} % of signal, p99 ${p99.toExponential(2)}, max ${worst.toExponential(2)}, clip coverage Δ ${(clipDiff / (mine.length / 4)).toExponential(2)}` : `size ${phone.width}×${phone.height} vs ${sw}×${sh}`}` +
			(nonFinite.phone || nonFinite.server ? `; non-finite samples: phone ${nonFinite.phone}, server ${nonFinite.server}, at the same places ${nonFinite.both}` : '') +
			(errors.length ? `; page errors: ${errors.join(' | ')}` : '')
	);
}
await browser.close();
process.exit(failed ? 1 : 0);
