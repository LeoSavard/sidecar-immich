// SPDX-License-Identifier: Apache-2.0
// Develop pass in the browser. Bind group layout, formats and the draw mirror crates/engine/src/develop.rs,
// and the WGSL is the same composed file, so the parity harness can compare the two renders directly.
import developWgsl from './wgsl/client_develop.wgsl?raw';
import densityDownWgsl from './wgsl/client_density_down.wgsl?raw';
import densityBlurWgsl from './wgsl/client_density_blur.wgsl?raw';
import { DEVELOP_UNIFORM_BYTES } from '$lib/color/pack';
import { kernel } from './depth';

/** `MaskParams` (shaders/develop/masks.wgsl), and the texture slots it describes. */
const MASK_UNIFORM_BYTES = 1568;
const MAX_MASKS = 8;
/** Each layer's curve: a row of this many entries. */
const LUT = 4096;
/** The adjacency map's long edge, at most: `sidecar_color::developer::MAP_LONG`. */
const MAP_LONG = 1024;
/** Float offsets in `DevelopParams` (shaders/develop/params.wgsl): `mono`, `developer` and `slide`. */
const MONO = 72;
const DEVELOPER = 80;
const SLIDE = 92;

/** One of a spectral stock's tables: `side`³ of four floats, read corner by corner (rgba32float can't filter). */
function slideTexture(device: GPUDevice, label: string, side: number): GPUTexture {
	return device.createTexture({
		label,
		size: [side, side, side],
		dimension: '3d',
		format: 'rgba32float',
		usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST
	});
}

export class DevelopPass {
	readonly device: GPUDevice;
	private pipeline: GPURenderPipeline;
	private layout: GPUBindGroupLayout;
	private ubo: GPUBuffer;
	private source?: GPUTexture;
	/** What develop reads instead of the proxy: the lens pass's output, while a correction is on. */
	private input?: GPUTexture;
	private lut?: GPUTexture;
	private target?: GPUTexture;
	private bindGroup?: GPUBindGroup;
	private maskLayout: GPUBindGroupLayout;
	private maskUbo: GPUBuffer;
	private maskSampler: GPUSampler;
	/** What an unused mask slot binds: 1×1, zero. */
	private noMask: GPUTexture;
	private maskTextures: (GPUTexture | undefined)[] = [];
	private maskBind?: GPUBindGroup;
	/** The layers' curves, `MAX_MASKS` rows. */
	private layerCurves: GPUTexture;
	/** Black and white's table over the colour cube; 1×1×1 in colour. */
	private mono: GPUTexture;
	private slideExposures: GPUTexture;
	private slideResponses: GPUTexture;
	/** The response table already on the GPU, so a slider move doesn't rewrite four megabytes. */
	private boundResponses: Float32Array = new Float32Array(0);
	/** The uniforms last set, which say whether the developer needs its adjacency map. */
	private uniform = new Float32Array(DEVELOP_UNIFORM_BYTES / 4);
	/** Black and white's adjacency map and the textures it is made in (`sidecar_color::developer`). */
	private density?: { small: GPUTexture; exposure: GPUTexture; across: GPUTexture; map: GPUTexture; ubo: GPUBuffer; params: GPUBuffer[] };
	/** The map the main bind group holds, so it is rebuilt when the map comes or goes. */
	private boundDensity?: GPUTexture;
	private densityDown: GPUComputePipeline;
	private densityBlur: GPUComputePipeline;

	private constructor(device: GPUDevice, pipeline: GPURenderPipeline, layout: GPUBindGroupLayout, maskLayout: GPUBindGroupLayout, densityDown: GPUComputePipeline, densityBlur: GPUComputePipeline) {
		this.device = device;
		this.pipeline = pipeline;
		this.layout = layout;
		this.maskLayout = maskLayout;
		this.densityDown = densityDown;
		this.densityBlur = densityBlur;
		this.ubo = device.createBuffer({ label: 'develop params', size: DEVELOP_UNIFORM_BYTES, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST });
		// Zeroed on creation: no masks until `setMasks` says otherwise.
		this.maskUbo = device.createBuffer({ label: 'develop masks', size: MASK_UNIFORM_BYTES, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST });
		this.maskSampler = device.createSampler({ magFilter: 'linear', minFilter: 'linear' });
		this.noMask = device.createTexture({ label: 'no mask', size: [1, 1], format: 'r8unorm', usage: GPUTextureUsage.TEXTURE_BINDING });
		this.layerCurves = device.createTexture({ label: 'layer curves', size: [LUT, MAX_MASKS], format: 'r32float', usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST });
		const identity = new Float32Array(LUT * MAX_MASKS).map((_, i) => (i % LUT) / (LUT - 1));
		device.queue.writeTexture({ texture: this.layerCurves }, identity, { bytesPerRow: LUT * 4 }, [LUT, MAX_MASKS]);
		this.mono = device.createTexture({ label: 'mono table', size: [1, 1, 1], dimension: '3d', format: 'r32float', usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST });
		this.slideExposures = slideTexture(device, 'slide exposures', 1);
		this.slideResponses = slideTexture(device, 'slide responses', 1);
	}

	static async create(device: GPUDevice): Promise<DevelopPass> {
		const module = device.createShaderModule({ label: 'develop', code: developWgsl });
		const info = await module.getCompilationInfo();
		const errors = info.messages.filter((m) => m.type === 'error');
		if (errors.length) throw new Error(`develop.wgsl: ${errors.map((e) => `${e.lineNum}:${e.linePos} ${e.message}`).join('; ')}`);
		const layout = device.createBindGroupLayout({
			label: 'develop',
			entries: [
				{ binding: 0, visibility: GPUShaderStage.FRAGMENT, buffer: { type: 'uniform' } },
				{ binding: 1, visibility: GPUShaderStage.FRAGMENT, texture: { sampleType: 'float', viewDimension: '2d' } },
				{ binding: 2, visibility: GPUShaderStage.FRAGMENT, texture: { sampleType: 'unfilterable-float', viewDimension: '1d' } },
				{ binding: 3, visibility: GPUShaderStage.FRAGMENT, texture: { sampleType: 'unfilterable-float', viewDimension: '3d' } },
				{ binding: 4, visibility: GPUShaderStage.FRAGMENT, texture: { sampleType: 'float', viewDimension: '2d' } },
				{ binding: 5, visibility: GPUShaderStage.FRAGMENT, texture: { sampleType: 'unfilterable-float', viewDimension: '3d' } },
				{ binding: 6, visibility: GPUShaderStage.FRAGMENT, texture: { sampleType: 'unfilterable-float', viewDimension: '3d' } }
			]
		});
		// Mirrors crates/engine/src/develop.rs: the mask uniforms, a linear sampler, a texture per slot.
		const maskLayout = device.createBindGroupLayout({
			label: 'develop masks',
			entries: [
				{ binding: 0, visibility: GPUShaderStage.FRAGMENT, buffer: { type: 'uniform' } },
				{ binding: 1, visibility: GPUShaderStage.FRAGMENT, sampler: { type: 'filtering' } },
				...Array.from({ length: MAX_MASKS }, (_, i) => ({ binding: 2 + i, visibility: GPUShaderStage.FRAGMENT, texture: { sampleType: 'float' as const, viewDimension: '2d' as const } })),
				{ binding: 2 + MAX_MASKS, visibility: GPUShaderStage.FRAGMENT, texture: { sampleType: 'unfilterable-float', viewDimension: '2d' } }
			]
		});
		const pipeline = await device.createRenderPipelineAsync({
			label: 'develop',
			layout: device.createPipelineLayout({ bindGroupLayouts: [layout, maskLayout] }),
			vertex: { module, entryPoint: 'vs' },
			fragment: { module, entryPoint: 'fs', targets: [{ format: 'rgba16float' }] },
			primitive: { topology: 'triangle-list' }
		});
		const [densityDown, densityBlur] = await Promise.all([
			kernel(device, 'density down', densityDownWgsl, ['uniform', 'texture', 'storage']),
			kernel(device, 'density blur', densityBlurWgsl, ['uniform', 'texture', 'storage'])
		]);
		return new DevelopPass(device, pipeline, layout, maskLayout, densityDown, densityBlur);
	}

	get output(): GPUTexture | undefined {
		return this.target;
	}

	/** The proxy as uploaded, for the lens pass to read. */
	get proxy(): GPUTexture | undefined {
		return this.source;
	}

	/** Develop from `texture` (the proxy's size) instead of the proxy; `undefined` goes back to the proxy. */
	setInput(texture?: GPUTexture) {
		if (texture === this.input) return;
		this.input = texture;
		this.bindGroup = undefined;
	}

	/** Upload the proxy once: raw little-endian RGBA16F bytes, row-major, top-left origin. */
	setSource(width: number, height: number, f16: ArrayBuffer | ArrayBufferView) {
		this.source?.destroy();
		this.target?.destroy();
		this.input = undefined;
		this.source = this.device.createTexture({ label: 'source', size: [width, height], format: 'rgba16float', usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST | GPUTextureUsage.COPY_SRC });
		this.device.queue.writeTexture({ texture: this.source }, f16, { bytesPerRow: width * 8 }, [width, height]);
		this.target = this.device.createTexture({
			label: 'developed',
			size: [width, height],
			format: 'rgba16float',
			usage: GPUTextureUsage.RENDER_ATTACHMENT | GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_SRC
		});
		this.bindGroup = undefined;
	}

	setUniforms(bytes: ArrayBuffer | ArrayBufferView) {
		if (bytes.byteLength !== DEVELOP_UNIFORM_BYTES) throw new Error(`develop uniforms must be ${DEVELOP_UNIFORM_BYTES} bytes`);
		this.device.queue.writeBuffer(this.ubo, 0, bytes);
		const view = ArrayBuffer.isView(bytes) ? bytes : new Uint8Array(bytes);
		new Uint8Array(this.uniform.buffer).set(new Uint8Array(view.buffer, view.byteOffset, view.byteLength));
	}

	/** Black and white, a developer with adjacency: `crates/engine`'s `needs_density_map`. */
	private get needsDensityMap(): boolean {
		const u = this.uniform;
		return u[MONO] > 0.5 && u[MONO + 2] < 0.5 && u[DEVELOPER] !== 0;
	}

	private storage(label: string, w: number, h: number, usage = 0): GPUTexture {
		return this.device.createTexture({ label, size: [w, h], format: 'rgba16float', usage: GPUTextureUsage.STORAGE_BINDING | GPUTextureUsage.TEXTURE_BINDING | usage });
	}

	private dispatch(encoder: GPUCommandEncoder, pipeline: GPUComputePipeline, params: GPUBuffer, from: GPUTexture, to: GPUTexture) {
		const bind = this.device.createBindGroup({
			layout: pipeline.getBindGroupLayout(0),
			entries: [
				{ binding: 0, resource: { buffer: params } },
				{ binding: 1, resource: from.createView() },
				{ binding: 2, resource: to.createView() }
			]
		});
		const pass = encoder.beginComputePass();
		pass.setPipeline(pipeline);
		pass.setBindGroup(0, bind);
		pass.dispatchWorkgroups(Math.ceil(to.width / 16), Math.ceil(to.height / 16));
		pass.end();
	}

	/**
	 * The adjacency map over the picture develop reads, as `DevelopPass::density_map` makes it on the server:
	 * averaged down by a whole number, developed to the film's log exposure, blurred over the developer's reach.
	 */
	private buildDensityMap(masks: GPUBindGroup) {
		const input = this.input ?? this.source!;
		const [w, h] = [input.width, input.height];
		const k = Math.max(1, Math.ceil(Math.max(w, h) / MAP_LONG));
		const [mw, mh] = [Math.ceil(w / k), Math.ceil(h / k)];
		if (this.density?.map.width !== mw || this.density?.map.height !== mh) {
			if (this.density) for (const t of [this.density.small, this.density.exposure, this.density.across, this.density.map]) t.destroy();
			const small = (label: string) => this.storage(label, mw, mh);
			const params = [0, 1, 2].map(() => this.device.createBuffer({ size: 16, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST }));
			this.density = {
				small: small('density source'),
				exposure: this.device.createTexture({ label: 'density exposure', size: [mw, mh], format: 'rgba16float', usage: GPUTextureUsage.RENDER_ATTACHMENT | GPUTextureUsage.TEXTURE_BINDING }),
				across: small('density across'),
				map: small('density map'),
				ubo: this.density?.ubo ?? this.device.createBuffer({ label: 'density develop params', size: DEVELOP_UNIFORM_BYTES, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST }),
				params
			};
			this.bindGroup = undefined;
		}
		const d = this.density;
		const own = this.uniform.slice();
		own[MONO + 2] = 1;
		this.device.queue.writeBuffer(d.ubo, 0, own);
		const sigma = (this.uniform[DEVELOPER + 1] * Math.max(w, h)) / k;
		this.device.queue.writeBuffer(d.params[0], 0, new Float32Array([k, 0, 0, 0]));
		this.device.queue.writeBuffer(d.params[1], 0, new Float32Array([sigma, 1, 0, 0]));
		this.device.queue.writeBuffer(d.params[2], 0, new Float32Array([sigma, 0, 1, 0]));
		let encoder = this.device.createCommandEncoder();
		this.dispatch(encoder, this.densityDown, d.params[0], input, d.small);
		this.device.queue.submit([encoder.finish()]);
		this.drawWith(this.groupFor(d.ubo, d.small, this.noMask), d.exposure, masks);
		encoder = this.device.createCommandEncoder();
		this.dispatch(encoder, this.densityBlur, d.params[1], d.exposure, d.across);
		this.dispatch(encoder, this.densityBlur, d.params[2], d.across, d.map);
		this.device.queue.submit([encoder.finish()]);
	}

	setToneLut(lut: Float32Array) {
		if (!this.lut || this.lut.width !== lut.length) {
			this.lut?.destroy();
			this.lut = this.device.createTexture({ label: 'tone lut', size: [lut.length], dimension: '1d', format: 'r32float', usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST });
			this.bindGroup = undefined;
		}
		this.device.queue.writeTexture({ texture: this.lut }, lut, {}, [lut.length]);
	}

	/** Black and white's table over the colour cube (`Packed.monoLut`); empty in colour, which keeps a 1×1×1 one. */
	setMono(table: Float32Array) {
		const side = Math.round(Math.cbrt(table.length));
		const want = side > 0 && side ** 3 === table.length ? side : 1;
		if (this.mono.width !== want) {
			this.mono.destroy();
			this.mono = this.device.createTexture({ label: 'mono table', size: [want, want, want], dimension: '3d', format: 'r32float', usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST });
			this.bindGroup = undefined;
		}
		if (want > 1) this.device.queue.writeTexture({ texture: this.mono }, table, { bytesPerRow: want * 4, rowsPerImage: want }, [want, want, want]);
	}

	/**
	 * A spectral colour stock's two tables: `exposures` from the packer (`Packed.slideExposure`, rebuilt per
	 * edit because it depends on the scene's light) and `responses` from the stock's profile, downloaded once.
	 * Either empty keeps a 1×1×1 table, which the uniform's amount of 0 then never reads. The response table is
	 * four megabytes and changes only with the stock, so it is written only when it actually changes.
	 */
	setSlide(exposures: Float32Array, responses: Float32Array) {
		this.slideExposures = this.writeSlide(this.slideExposures, 'slide exposures', exposures, true);
		this.slideResponses = this.writeSlide(this.slideResponses, 'slide responses', responses, this.boundResponses !== responses);
		this.boundResponses = responses;
	}

	private writeSlide(texture: GPUTexture, label: string, table: Float32Array, changed: boolean): GPUTexture {
		const side = Math.round(Math.cbrt(table.length / 4));
		const want = side > 0 && side ** 3 * 4 === table.length ? side : 1;
		if (texture.width !== want) {
			texture.destroy();
			texture = slideTexture(this.device, label, want);
			this.bindGroup = undefined;
			changed = true;
		}
		if (want > 1 && changed) {
			this.device.queue.writeTexture({ texture }, table, { bytesPerRow: want * 16, rowsPerImage: want }, [want, want, want]);
		}
		return texture;
	}

	/**
	 * The edit's masks: `params` and `curves` from the packer (`Packed.masks`, `Packed.maskCurves`), and a
	 * texture per mask in the frame being developed — `undefined` for one not loaded yet, which then weighs
	 * nothing.
	 */
	setMasks(params: Uint8Array, textures: (GPUTexture | undefined)[], curves?: Float32Array) {
		if (params.byteLength !== MASK_UNIFORM_BYTES) throw new Error(`mask uniforms must be ${MASK_UNIFORM_BYTES} bytes`);
		this.device.queue.writeBuffer(this.maskUbo, 0, params);
		if (curves?.length === LUT * MAX_MASKS) this.device.queue.writeTexture({ texture: this.layerCurves }, curves, { bytesPerRow: LUT * 4 }, [LUT, MAX_MASKS]);
		if (textures.length !== this.maskTextures.length || textures.some((t, i) => t !== this.maskTextures[i])) {
			this.maskTextures = textures.slice(0, MAX_MASKS);
			this.maskBind = undefined;
		}
	}

	private maskGroup(ubo: GPUBuffer, textures: (GPUTexture | undefined)[]): GPUBindGroup {
		return this.device.createBindGroup({
			layout: this.maskLayout,
			entries: [
				{ binding: 0, resource: { buffer: ubo } },
				{ binding: 1, resource: this.maskSampler },
				...Array.from({ length: MAX_MASKS }, (_, i) => ({ binding: 2 + i, resource: (textures[i] ?? this.noMask).createView() })),
				{ binding: 2 + MAX_MASKS, resource: this.layerCurves.createView() }
			]
		});
	}

	private groupFor(ubo: GPUBuffer, input: GPUTexture, density: GPUTexture): GPUBindGroup {
		return this.device.createBindGroup({
			layout: this.layout,
			entries: [
				{ binding: 0, resource: { buffer: ubo } },
				{ binding: 1, resource: input.createView() },
				{ binding: 2, resource: this.lut!.createView() },
				{ binding: 3, resource: this.mono.createView() },
				{ binding: 4, resource: density.createView() },
				{ binding: 5, resource: this.slideExposures.createView() },
				{ binding: 6, resource: this.slideResponses.createView() }
			]
		});
	}

	private draw(target: GPUTexture, masks: GPUBindGroup) {
		if (!this.source || !this.lut) throw new Error('develop pass needs a source and a tone LUT');
		if (this.needsDensityMap) this.buildDensityMap(masks);
		const density = this.needsDensityMap && this.density ? this.density.map : this.noMask;
		if (density !== this.boundDensity) this.bindGroup = undefined;
		this.boundDensity = density;
		this.bindGroup ??= this.groupFor(this.ubo, this.input ?? this.source, density);
		this.drawWith(this.bindGroup, target, masks);
	}

	private drawWith(group: GPUBindGroup, target: GPUTexture, masks: GPUBindGroup) {
		const encoder = this.device.createCommandEncoder();
		const pass = encoder.beginRenderPass({ colorAttachments: [{ view: target.createView(), loadOp: 'clear', storeOp: 'store', clearValue: [0, 0, 0, 1] }] });
		pass.setPipeline(this.pipeline);
		pass.setBindGroup(0, group);
		pass.setBindGroup(1, masks);
		pass.draw(3);
		pass.end();
		this.device.queue.submit([encoder.finish()]);
	}

	render() {
		if (!this.target) throw new Error('develop pass needs a source');
		this.maskBind ??= this.maskGroup(this.maskUbo, this.maskTextures);
		this.draw(this.target, this.maskBind);
	}

	/** A texture of the developed picture's size and format, for `renderVariant`. */
	variantTarget(label: string): GPUTexture {
		const t = this.target!;
		return this.device.createTexture({ label, size: [t.width, t.height], format: 'rgba16float', usage: GPUTextureUsage.RENDER_ATTACHMENT | GPUTextureUsage.TEXTURE_BINDING });
	}

	/**
	 * Develop into `target` with other mask uniforms and textures — the brush preview's renders without and
	 * with the layer being painted — leaving the editor's own as they are.
	 */
	renderVariant(target: GPUTexture, maskParams: Uint8Array, textures: (GPUTexture | undefined)[]) {
		const ubo = this.device.createBuffer({ size: MASK_UNIFORM_BYTES, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST });
		this.device.queue.writeBuffer(ubo, 0, maskParams);
		this.draw(target, this.maskGroup(ubo, textures));
	}

	/**
	 * The camera RGB around a point of the source (0…1), averaged over a square `radius` pixels out from it,
	 * leaving out pixels the ingest flagged as clipped (alpha above 0): the white balance eyedropper's patch.
	 * Null when every pixel there is clipped.
	 */
	async sampleSource(at: [number, number], radius: number): Promise<[number, number, number] | null> {
		const t = this.source!;
		const cx = Math.round(at[0] * (t.width - 1));
		const cy = Math.round(at[1] * (t.height - 1));
		const x0 = Math.max(0, Math.min(t.width - 1, cx - radius));
		const y0 = Math.max(0, Math.min(t.height - 1, cy - radius));
		const w = Math.min(t.width, cx + radius + 1) - x0;
		const h = Math.min(t.height, cy + radius + 1) - y0;
		if (w <= 0 || h <= 0) return null;
		const bytesPerRow = 256 * Math.ceil((w * 8) / 256);
		const buf = this.device.createBuffer({ size: bytesPerRow * h, usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ });
		const encoder = this.device.createCommandEncoder();
		encoder.copyTextureToBuffer({ texture: t, origin: [x0, y0] }, { buffer: buf, bytesPerRow }, [w, h]);
		this.device.queue.submit([encoder.finish()]);
		await buf.mapAsync(GPUMapMode.READ);
		const sum = [0, 0, 0];
		let n = 0;
		for (let row = 0; row < h; row++) {
			const bits = new Uint16Array(buf.getMappedRange(row * bytesPerRow, bytesPerRow), 0, w * 4);
			const px = Array.from(bits, half);
			for (let i = 0; i < w * 4; i += 4) {
				if (px[i + 3] > 0) continue;
				sum[0] += px[i];
				sum[1] += px[i + 1];
				sum[2] += px[i + 2];
				n++;
			}
		}
		buf.unmap();
		buf.destroy();
		return n ? [sum[0] / n, sum[1] / n, sum[2] / n] : null;
	}

	/** Read developedTex back as RGBA16F bytes (tests and parity only — the editor never reads pixels back). */
	async readback(): Promise<Uint8Array> {
		const t = this.target!;
		const bytesPerRow = Math.ceil((t.width * 8) / 256) * 256;
		const buf = this.device.createBuffer({ size: bytesPerRow * t.height, usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ });
		const encoder = this.device.createCommandEncoder();
		encoder.copyTextureToBuffer({ texture: t }, { buffer: buf, bytesPerRow }, [t.width, t.height]);
		this.device.queue.submit([encoder.finish()]);
		await buf.mapAsync(GPUMapMode.READ);
		const mapped = new Uint8Array(buf.getMappedRange());
		const out = new Uint8Array(t.width * 8 * t.height);
		for (let row = 0; row < t.height; row++) out.set(mapped.subarray(row * bytesPerRow, row * bytesPerRow + t.width * 8), row * t.width * 8);
		buf.unmap();
		buf.destroy();
		return out;
	}
}

/** An IEEE half float's bits → its value. */
function half(h: number): number {
	const exp = (h >> 10) & 0x1f;
	const frac = h & 0x3ff;
	const sign = h & 0x8000 ? -1 : 1;
	if (exp === 0) return sign * frac * 2 ** -24;
	if (exp === 31) return frac ? NaN : sign * Infinity;
	return sign * (1 + frac / 1024) * 2 ** (exp - 15);
}

export async function requestDevice(): Promise<GPUDevice> {
	if (!('gpu' in navigator)) throw new Error('This browser has no WebGPU. Sidecar needs Safari 26 or a current Chromium.');
	const adapter = await navigator.gpu.requestAdapter({ powerPreference: 'high-performance' });
	if (!adapter) throw new Error('WebGPU found no GPU adapter.');
	return adapter.requestDevice({ label: 'sidecar' });
}
