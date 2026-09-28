// SPDX-License-Identifier: Apache-2.0
// Lens pass in the browser: distortion, lateral CA and vignetting over the proxy, into a texture the develop
// pass reads instead of the proxy. Same composed WGSL as the worker (crates/engine/src/lens.rs); uniforms come
// from the Rust packing through WebAssembly (ColorPacker.packLens). Runs when a lens setting changes, not
// per frame.
import lensWgsl from './wgsl/client_lens.wgsl?raw';

const LENS_UNIFORM_BYTES = 1072;

export class LensPass {
	private constructor(
		private device: GPUDevice,
		private pipeline: GPUComputePipeline,
		private ubo: GPUBuffer
	) {}

	static async create(device: GPUDevice): Promise<LensPass> {
		const module = device.createShaderModule({ label: 'lens', code: lensWgsl });
		const info = await module.getCompilationInfo();
		const errors = info.messages.filter((m) => m.type === 'error');
		if (errors.length) throw new Error(`lens.wgsl: ${errors.map((e) => `${e.lineNum}:${e.linePos} ${e.message}`).join('; ')}`);
		const layout = device.createBindGroupLayout({
			label: 'lens',
			entries: [
				{ binding: 0, visibility: GPUShaderStage.COMPUTE, buffer: { type: 'uniform' } },
				{ binding: 1, visibility: GPUShaderStage.COMPUTE, texture: { sampleType: 'unfilterable-float', viewDimension: '2d' } },
				{ binding: 2, visibility: GPUShaderStage.COMPUTE, storageTexture: { access: 'write-only', format: 'rgba16float', viewDimension: '2d' } }
			]
		});
		const pipeline = await device.createComputePipelineAsync({
			label: 'lens',
			layout: device.createPipelineLayout({ bindGroupLayouts: [layout] }),
			compute: { module, entryPoint: 'main' }
		});
		const ubo = device.createBuffer({ label: 'lens params', size: LENS_UNIFORM_BYTES, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST });
		return new LensPass(device, pipeline, ubo);
	}

	/** Correct `input` into `output` (same size, `rgba16float` with storage binding) using packed uniforms. */
	run(input: GPUTexture, output: GPUTexture, uniform: Uint8Array) {
		if (uniform.byteLength !== LENS_UNIFORM_BYTES) throw new Error(`lens uniforms must be ${LENS_UNIFORM_BYTES} bytes`);
		this.device.queue.writeBuffer(this.ubo, 0, uniform);
		const bind = this.device.createBindGroup({
			layout: this.pipeline.getBindGroupLayout(0),
			entries: [
				{ binding: 0, resource: { buffer: this.ubo } },
				{ binding: 1, resource: input.createView() },
				{ binding: 2, resource: output.createView() }
			]
		});
		const encoder = this.device.createCommandEncoder();
		const pass = encoder.beginComputePass();
		pass.setPipeline(this.pipeline);
		pass.setBindGroup(0, bind);
		pass.dispatchWorkgroups(Math.ceil(output.width / 16), Math.ceil(output.height / 16));
		pass.end();
		this.device.queue.submit([encoder.finish()]);
	}

	/** A texture the pass can write, the size of `like`. */
	target(like: GPUTexture, label: string): GPUTexture {
		return this.device.createTexture({
			label,
			size: [like.width, like.height],
			format: 'rgba16float',
			usage: GPUTextureUsage.STORAGE_BINDING | GPUTextureUsage.TEXTURE_BINDING
		});
	}
}

/**
 * Where a point of the uncorrected image lands in the corrected one — for markers placed in sensor geometry,
 * such as the AF point. Inverts the packed distortion curve (green channel) by bisection on the radius; both
 * points normalized to the image (0…1). Returns the point unchanged when `uniform` is empty.
 */
export function correctedPoint(uniform: Uint8Array | undefined, width: number, height: number, p: { x: number; y: number }) {
	if (!uniform || uniform.byteLength !== LENS_UNIFORM_BYTES) return p;
	const f = new Float32Array(uniform.buffer, uniform.byteOffset, LENS_UNIFORM_BYTES / 4);
	const [cx, cy, , , , , hd, zoom, rScale] = f;
	const knot = (r: number) => {
		const x = Math.min(Math.max(r * rScale, 0), 63);
		const i = Math.min(Math.floor(x), 62);
		const t = x - i;
		return f[12 + i * 4] * (1 - t) + f[12 + (i + 1) * 4] * t;
	};
	const vx = (p.x * width - cx) / hd;
	const vy = (p.y * height - cy) / hd;
	const rs = Math.hypot(vx, vy);
	if (rs < 1e-9) return p;
	// Corrected radius ru reads from zoom · ru · D(zoom · ru); find the one that reads from rs.
	const reach = (ru: number) => zoom * ru * knot(zoom * ru);
	let lo = 0;
	let hi = 3;
	for (let i = 0; i < 48; i++) {
		const mid = (lo + hi) / 2;
		if (reach(mid) < rs) lo = mid;
		else hi = mid;
	}
	const s = lo / rs;
	return { x: (cx + vx * s * hd) / width, y: (cy + vy * s * hd) / height };
}
