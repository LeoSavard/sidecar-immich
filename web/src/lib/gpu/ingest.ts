// SPDX-License-Identifier: Apache-2.0
// GPU ingest in the browser: raw mosaic → black level → highlight reconstruction → demosaic (RCD for Bayer, 3×3
// binning for X-Trans) → proxy, camera-native linear RGB with clip coverage in alpha. The same composed WGSL the
// worker runs, in the same order with the same parameters: this is crates/engine/src/ingest.rs `Ingest::proxy`
// followed by `RgbaF32::oriented`, ported pass for pass, so a proxy a phone decoded is the one the server would
// have sent. A change to either side is a change to both (docs/MOBILE.md, stage 2).
import normalizeWgsl from './wgsl/ingest_normalize.wgsl?raw';
import halfWgsl from './wgsl/ingest_hilite_half.wgsl?raw';
import reduceWgsl from './wgsl/ingest_hilite_reduce.wgsl?raw';
import assembleWgsl from './wgsl/ingest_hilite_assemble.wgsl?raw';
import doubWgsl from './wgsl/ingest_hilite_doub.wgsl?raw';
import rcdConvWgsl from './wgsl/ingest_rcd_conv.wgsl?raw';
import rcdGreenWgsl from './wgsl/ingest_rcd_green.wgsl?raw';
import rcdDiagWgsl from './wgsl/ingest_rcd_rb_diag.wgsl?raw';
import rcdFinalWgsl from './wgsl/ingest_rcd_rb_green.wgsl?raw';
import xtransBinWgsl from './wgsl/ingest_xtrans_bin.wgsl?raw';
import downsampleWgsl from './wgsl/ingest_downsample.wgsl?raw';
import resampleWgsl from './wgsl/ingest_resample.wgsl?raw';
import { kernel, type Binding } from './depth';
import { mosaicClip } from './levels';

/** A raw mosaic as LibRaw's shim hands it over (`sc_mosaic`; `sidecar_engine::MosaicInput`). */
export type Mosaic = {
	width: number;
	height: number;
	data: Uint16Array;
	/** Per CFA colour index 0…3 (R, G, B, G2), raw units. */
	black: [number, number, number, number];
	white: number;
	/** Colour index of (row % period, col % period), 6 × 6 row-major. */
	pattern: number[];
	period: number;
	/** LibRaw's orientation bits, applied after the proxy is read back, as the worker does. */
	flip: number;
};

/** A proxy as `DevelopPass.setSource` takes it: RGBA16F, row-major, oriented. */
export type Proxy = { width: number; height: number; f16: Uint16Array };

/** `IngestParams` (shaders/ingest/common.wgsl; `sidecar_engine::ingest::IngestParams`), 224 bytes. */
const PARAMS_BYTES = 224;
type Params = {
	inSize: [number, number];
	outSize: [number, number];
	black: number[];
	wb: number[];
	/** Where each CFA colour saturates, normalized; see `clip_levels` in crates/engine/src/ingest.rs. */
	clip: number[];
	white: number;
	desat: number;
	sensor: number;
	pattern: Uint32Array;
};

function packParams(p: Params): ArrayBuffer {
	const buf = new ArrayBuffer(PARAMS_BYTES);
	const u = new Uint32Array(buf);
	const f = new Float32Array(buf);
	u.set(p.inSize, 0);
	u.set(p.outSize, 2);
	f.set(p.black, 4);
	f.set(p.wb, 8);
	f.set(p.clip, 12);
	f[16] = p.white;
	f[17] = p.desat;
	u[19] = p.sensor;
	u.set(p.pattern, 20);
	return buf;
}

/** Crop offsets that put the mosaic on the phase the shaders expect: `phase` in ingest.rs. */
function phase(m: Mosaic): { ox: number; oy: number; w: number; h: number; sensor: number } {
	const colour = (y: number, x: number) => {
		const c = m.pattern[(y % m.period) * 6 + (x % m.period)];
		return c === 3 ? 1 : c;
	};
	if (m.period === 2) {
		for (const [oy, ox] of [
			[0, 0],
			[0, 1],
			[1, 0],
			[1, 1]
		]) {
			if (colour(oy, ox) === 0 && colour(oy, ox + 1) === 1 && colour(oy + 1, ox) === 1 && colour(oy + 1, ox + 1) === 2) {
				return { ox, oy, w: Math.floor((m.width - ox) / 2) * 2, h: Math.floor((m.height - oy) / 2) * 2, sensor: 0 };
			}
		}
		throw new Error('unsupported Bayer layout');
	}
	// X-Trans: greens at the corners and centre of each 3×3 block, blue at (x 0, y 1) on even blocks.
	for (let oy = 0; oy < 6; oy++) {
		for (let ox = 0; ox < 6; ox++) {
			const g = (x: number, y: number) => colour(oy + y, ox + x) === 1;
			const block = (bx: number, by: number) => {
				const [x0, y0] = [bx * 3, by * 3];
				const greens = g(x0, y0) && g(x0 + 2, y0) && g(x0 + 1, y0 + 1) && g(x0, y0 + 2) && g(x0 + 2, y0 + 2);
				const even = (bx + by) % 2 === 0;
				const [edgeV, edgeH] = even ? [2, 0] : [0, 2];
				return (
					greens &&
					colour(oy + y0 + 1, ox + x0) === edgeV &&
					colour(oy + y0 + 1, ox + x0 + 2) === edgeV &&
					colour(oy + y0, ox + x0 + 1) === edgeH &&
					colour(oy + y0 + 2, ox + x0 + 1) === edgeH
				);
			};
			if (block(0, 0) && block(1, 0) && block(0, 1) && block(1, 1)) {
				return { ox, oy, w: Math.floor((m.width - ox) / 3) * 3, h: Math.floor((m.height - oy) / 3) * 3, sensor: 1 };
			}
		}
	}
	throw new Error('unsupported X-Trans layout');
}

const F = { r32: { storage: 'r32float' }, rgba32: { storage: 'rgba32float' }, rgba16: { storage: 'rgba16float' } } as const;

type Pass = { pipeline: GPUComputePipeline };

export class IngestPass {
	private constructor(
		private device: GPUDevice,
		private passes: Record<string, Pass>,
		private sampler: GPUSampler
	) {}

	static async create(device: GPUDevice): Promise<IngestPass> {
		const defs: Record<string, [string, Binding[]]> = {
			normalize: [normalizeWgsl, ['uniform', 'uint', F.r32]],
			half: [halfWgsl, ['uniform', 'texture', F.rgba32]],
			reduce: [reduceWgsl, ['uniform', 'texture', F.rgba32]],
			assemble: [assembleWgsl, ['uniform', 'texture', 'texture', F.rgba32]],
			doub: [doubWgsl, ['uniform', 'texture', 'texture', F.r32]],
			rcdConv: [rcdConvWgsl, ['uniform', 'texture', F.r32, F.r32, F.r32]],
			rcdGreen: [rcdGreenWgsl, ['uniform', 'texture', 'texture', 'texture', F.r32]],
			rcdDiag: [rcdDiagWgsl, ['uniform', 'texture', 'texture', 'texture', F.rgba32]],
			rcdFinal: [rcdFinalWgsl, ['uniform', 'texture', 'texture', 'texture', 'texture', F.rgba16]],
			xtransBin: [xtransBinWgsl, ['uniform', 'texture', 'texture', F.rgba16]],
			downsample: [downsampleWgsl, ['uniform', 'texture', F.rgba16]],
			resample: [resampleWgsl, ['uniform', 'filtered', 'sampler', F.rgba16]]
		};
		const names = Object.keys(defs);
		const pipelines = await Promise.all(names.map((n) => kernel(device, `ingest ${n}`, defs[n][0], defs[n][1])));
		const passes = Object.fromEntries(names.map((n, i) => [n, { pipeline: pipelines[i] }]));
		const sampler = device.createSampler({ magFilter: 'linear', minFilter: 'linear' });
		return new IngestPass(device, passes, sampler);
	}

	private texture(w: number, h: number, format: GPUTextureFormat): GPUTexture {
		return this.device.createTexture({
			size: [w, h],
			format,
			usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.STORAGE_BINDING | GPUTextureUsage.COPY_SRC
		});
	}

	/** One dispatch of `pass` over `size`, in 16 × 16 workgroups, as `Kernel::run` does. */
	private run(enc: GPUCommandEncoder, pass: string, params: Params, size: [number, number], resources: (GPUTexture | GPUSampler)[]) {
		const { pipeline } = this.passes[pass];
		const ubo = this.device.createBuffer({ size: PARAMS_BYTES, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST });
		this.device.queue.writeBuffer(ubo, 0, packParams(params));
		const bind = this.device.createBindGroup({
			layout: pipeline.getBindGroupLayout(0),
			entries: [
				{ binding: 0, resource: { buffer: ubo } },
				...resources.map((r, i) => ({ binding: i + 1, resource: r instanceof GPUTexture ? r.createView() : r }))
			]
		});
		const c = enc.beginComputePass();
		c.setPipeline(pipeline);
		c.setBindGroup(0, bind);
		c.dispatchWorkgroups(Math.ceil(size[0] / 16), Math.ceil(size[1] / 16));
		c.end();
	}

	/**
	 * Develop the mosaic to camera-native linear RGB, at no more than `longEdge` on its long side (never
	 * upscaled), oriented: `Ingest::proxy` then `RgbaF32::oriented(m.flip)`. `wb` is the as-shot multipliers.
	 */
	async proxy(m: Mosaic, wb: [number, number, number], longEdge: number): Promise<Proxy> {
		const textures: GPUTexture[] = [];
		const t = (w: number, h: number, format: GPUTextureFormat) => {
			const tex = this.texture(w, h, format);
			textures.push(tex);
			return tex;
		};
		// A phone's GPU can run out of memory or refuse a size partway through; without these scopes that would
		// read back as a black picture rather than an error the page can act on. Each is popped exactly once.
		this.device.pushErrorScope('out-of-memory');
		this.device.pushErrorScope('validation');
		let read: Proxy | undefined;
		let thrown: unknown;
		try {
			const rgb = this.demosaic(m, wb, t);
			read = await this.read(this.downsample(rgb, longEdge, t));
		} catch (e) {
			thrown = e;
		} finally {
			for (const tex of textures) tex.destroy();
		}
		const validation = await this.device.popErrorScope();
		const memory = await this.device.popErrorScope();
		if (thrown) throw thrown;
		const failure = validation ?? memory;
		if (failure || !read) throw new Error(failure?.message ?? 'the proxy was not read back');
		return oriented(read, m.flip);
	}

	private demosaic(m: Mosaic, wb: [number, number, number], t: (w: number, h: number, f: GPUTextureFormat) => GPUTexture): GPUTexture {
		const { ox, oy, w, h, sensor } = phase(m);
		const pattern = new Uint32Array(36);
		for (let i = 0; i < 36; i++) pattern[i] = m.pattern[((Math.floor(i / 6) + oy) % m.period) * 6 + ((i % 6) + ox) % m.period];
		// Where each colour saturates, measured from these samples as `Mosaic::clip` measures them on the
		// server: the same detector, so a proxy made here is the one the server would have sent.
		const clip = mosaicClip(m.data, m.width, m.white, m.black, m.pattern, m.period);
		const base: Params = {
			inSize: [w, h],
			outSize: [w, h],
			black: m.black,
			wb: [wb[0], wb[1], wb[2], wb[1]],
			clip,
			white: m.white,
			desat: 0.3,
			sensor,
			pattern
		};
		const sized = (inSize: [number, number], outSize: [number, number]): Params => ({ ...base, inSize, outSize });

		// The cropped mosaic.
		const cropped = new Uint16Array(w * h);
		for (let y = 0; y < h; y++) cropped.set(m.data.subarray((y + oy) * m.width + ox, (y + oy) * m.width + ox + w), y * w);
		const raw = this.device.createTexture({ size: [w, h], format: 'r16uint', usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST });
		this.device.queue.writeTexture({ texture: raw }, cropped, { bytesPerRow: w * 2 }, [w, h]);

		const enc = this.device.createCommandEncoder();
		const norm = t(w, h, 'r32float');
		this.run(enc, 'normalize', base, [w, h], [raw, norm]);

		// Highlight reconstruction pyramid.
		const block = sensor === 1 ? 3 : 2;
		const [hw, hh] = [Math.floor(w / block), Math.floor(h / block)];
		const half = t(hw, hh, 'rgba32float');
		this.run(enc, 'half', sized([w, h], [hw, hh]), [hw, hh], [norm, half]);
		const levels = [half];
		for (;;) {
			const prev = levels[levels.length - 1];
			const [pw, ph] = [prev.width, prev.height];
			if (pw <= 1 || ph <= 1 || levels.length >= 15) break;
			const [cw, ch] = [Math.floor((pw - 1) / 2) + 1, Math.floor((ph - 1) / 2) + 1];
			const next = t(cw, ch, 'rgba32float');
			this.run(enc, 'reduce', sized([pw, ph], [cw, ch]), [cw, ch], [prev, next]);
			levels.push(next);
		}
		let coarse = levels.pop()!;
		for (let fine = levels.pop(); fine; fine = levels.pop()) {
			const out = t(fine.width, fine.height, 'rgba32float');
			this.run(enc, 'assemble', sized([coarse.width, coarse.height], [fine.width, fine.height]), [fine.width, fine.height], [fine, coarse, out]);
			coarse = out;
		}
		const cfa = t(w, h, 'r32float');
		this.run(enc, 'doub', sized([w, h], [hw, hh]), [hw, hh], [norm, coarse, cfa]);

		let rgb: GPUTexture;
		if (sensor === 0) {
			const [vh, pq, lp] = [t(w, h, 'r32float'), t(w, h, 'r32float'), t(w, h, 'r32float')];
			this.run(enc, 'rcdConv', base, [w, h], [cfa, vh, pq, lp]);
			const g = t(w, h, 'r32float');
			this.run(enc, 'rcdGreen', base, [w, h], [cfa, vh, lp, g]);
			const rb = t(w, h, 'rgba32float');
			this.run(enc, 'rcdDiag', base, [w, h], [cfa, pq, g, rb]);
			rgb = t(w, h, 'rgba16float');
			this.run(enc, 'rcdFinal', base, [w, h], [vh, g, rb, norm, rgb]);
		} else {
			rgb = t(hw, hh, 'rgba16float');
			this.run(enc, 'xtransBin', sized([w, h], [hw, hh]), [hw, hh], [cfa, norm, rgb]);
		}
		this.device.queue.submit([enc.finish()]);
		raw.destroy();
		return rgb;
	}

	/** Box-reduce then bilinear-resample to `longEdge`: `downsample_and_read` in ingest.rs, before the read. */
	private downsample(rgb: GPUTexture, longEdge: number, t: (w: number, h: number, f: GPUTextureFormat) => GPUTexture): GPUTexture {
		const zero: Params = { inSize: [0, 0], outSize: [0, 0], black: [0, 0, 0, 0], wb: [0, 0, 0, 0], clip: [0, 0, 0, 0], white: 0, desat: 0, sensor: 0, pattern: new Uint32Array(36) };
		const sized = (inSize: [number, number], outSize: [number, number]): Params => ({ ...zero, inSize, outSize });
		const [sw, sh] = [rgb.width, rgb.height];
		const scale = Math.min(1, longEdge / Math.max(sw, sh));
		const [tw, th] = [Math.max(1, Math.round(sw * scale)), Math.max(1, Math.round(sh * scale))];
		const enc = this.device.createCommandEncoder();
		let cur = rgb;
		while (cur.width >= tw * 2 && cur.height >= th * 2) {
			const [nw, nh] = [Math.ceil(cur.width / 2), Math.ceil(cur.height / 2)];
			const next = t(nw, nh, 'rgba16float');
			this.run(enc, 'downsample', sized([cur.width, cur.height], [nw, nh]), [nw, nh], [cur, next]);
			cur = next;
		}
		if (cur.width !== tw || cur.height !== th) {
			const next = t(tw, th, 'rgba16float');
			this.run(enc, 'resample', sized([cur.width, cur.height], [tw, th]), [tw, th], [cur, this.sampler, next]);
			cur = next;
		}
		this.device.queue.submit([enc.finish()]);
		return cur;
	}

	private async read(tex: GPUTexture): Promise<Proxy> {
		const [w, h] = [tex.width, tex.height];
		const bytesPerRow = Math.ceil((w * 8) / 256) * 256;
		const buf = this.device.createBuffer({ size: bytesPerRow * h, usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ });
		const enc = this.device.createCommandEncoder();
		enc.copyTextureToBuffer({ texture: tex }, { buffer: buf, bytesPerRow }, [w, h]);
		this.device.queue.submit([enc.finish()]);
		await buf.mapAsync(GPUMapMode.READ);
		const mapped = new Uint16Array(buf.getMappedRange());
		const f16 = new Uint16Array(w * h * 4);
		for (let y = 0; y < h; y++) f16.set(mapped.subarray((y * bytesPerRow) / 2, (y * bytesPerRow) / 2 + w * 4), y * w * 4);
		buf.unmap();
		buf.destroy();
		return { width: w, height: h, f16 };
	}
}

/**
 * Apply a LibRaw orientation: `RgbaF32::oriented`. `flip` is a bit set — 4 transposes, 2 mirrors top to bottom,
 * 1 mirrors left to right — so 3 is 180°, 5 is 90° counter-clockwise, 6 is 90° clockwise.
 */
export function oriented(p: Proxy, flip: number): Proxy {
	flip &= 7;
	if (flip === 0) return p;
	const { width: w, height: h } = p;
	const transpose = (flip & 4) !== 0;
	const [nw, nh] = transpose ? [h, w] : [w, h];
	const out = new Uint16Array(p.f16.length);
	let o = 0;
	for (let ny = 0; ny < nh; ny++) {
		for (let nx = 0; nx < nw; nx++) {
			let [x, y] = transpose ? [ny, nx] : [nx, ny];
			if (flip & 2) y = h - 1 - y;
			if (flip & 1) x = w - 1 - x;
			const i = (y * w + x) * 4;
			out[o++] = p.f16[i];
			out[o++] = p.f16[i + 1];
			out[o++] = p.f16[i + 2];
			out[o++] = p.f16[i + 3];
		}
	}
	return { width: nw, height: nh, f16: out };
}
