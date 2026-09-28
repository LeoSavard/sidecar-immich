// SPDX-License-Identifier: Apache-2.0
// Develop pass in the browser. Bind group layout, formats and the draw mirror crates/engine/src/develop.rs,
// and the WGSL is the same composed file, so the parity harness can compare the two renders directly.
import developWgsl from './wgsl/client_develop.wgsl?raw';

const DEVELOP_UNIFORM_BYTES = 272;

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

	private constructor(device: GPUDevice, pipeline: GPURenderPipeline, layout: GPUBindGroupLayout) {
		this.device = device;
		this.pipeline = pipeline;
		this.layout = layout;
		this.ubo = device.createBuffer({ label: 'develop params', size: DEVELOP_UNIFORM_BYTES, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST });
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
				{ binding: 2, visibility: GPUShaderStage.FRAGMENT, texture: { sampleType: 'unfilterable-float', viewDimension: '1d' } }
			]
		});
		const pipeline = await device.createRenderPipelineAsync({
			label: 'develop',
			layout: device.createPipelineLayout({ bindGroupLayouts: [layout] }),
			vertex: { module, entryPoint: 'vs' },
			fragment: { module, entryPoint: 'fs', targets: [{ format: 'rgba16float' }] },
			primitive: { topology: 'triangle-list' }
		});
		return new DevelopPass(device, pipeline, layout);
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
		this.source = this.device.createTexture({ label: 'source', size: [width, height], format: 'rgba16float', usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST });
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
	}

	setToneLut(lut: Float32Array) {
		if (!this.lut || this.lut.width !== lut.length) {
			this.lut?.destroy();
			this.lut = this.device.createTexture({ label: 'tone lut', size: [lut.length], dimension: '1d', format: 'r32float', usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST });
			this.bindGroup = undefined;
		}
		this.device.queue.writeTexture({ texture: this.lut }, lut, {}, [lut.length]);
	}

	render() {
		if (!this.source || !this.lut || !this.target) throw new Error('develop pass needs a source and a tone LUT');
		this.bindGroup ??= this.device.createBindGroup({
			layout: this.layout,
			entries: [
				{ binding: 0, resource: { buffer: this.ubo } },
				{ binding: 1, resource: (this.input ?? this.source).createView() },
				{ binding: 2, resource: this.lut.createView() }
			]
		});
		const encoder = this.device.createCommandEncoder();
		const pass = encoder.beginRenderPass({ colorAttachments: [{ view: this.target.createView(), loadOp: 'clear', storeOp: 'store', clearValue: [0, 0, 0, 1] }] });
		pass.setPipeline(this.pipeline);
		pass.setBindGroup(0, this.bindGroup);
		pass.draw(3);
		pass.end();
		this.device.queue.submit([encoder.finish()]);
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

export async function requestDevice(): Promise<GPUDevice> {
	if (!('gpu' in navigator)) throw new Error('This browser has no WebGPU. Sidecar needs Safari 26 or a current Chromium.');
	const adapter = await navigator.gpu.requestAdapter({ powerPreference: 'high-performance' });
	if (!adapter) throw new Error('WebGPU found no GPU adapter.');
	return adapter.requestDevice({ label: 'sidecar' });
}
