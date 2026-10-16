// SPDX-License-Identifier: Apache-2.0
// Lens blur and haze by distance in the browser: the same composed WGSL the worker runs
// (crates/engine/src/depth.rs), over the proxy after the lens pass, into a texture develop reads instead. The
// uniform comes from the Rust packing through WebAssembly (ColorPacker.packDepth). Runs when a depth setting,
// the lens correction or the proxy changes, not per frame.
import downWgsl from './wgsl/client_depth_down.wgsl?raw';
import gatherWgsl from './wgsl/client_depth_gather.wgsl?raw';
import composeWgsl from './wgsl/client_depth_compose.wgsl?raw';
import refineWgsl from './wgsl/client_depth_refine.wgsl?raw';

const DEPTH_UNIFORM_BYTES = 64;
/** `DepthRefineParams` in shaders/client/depth_refine.wgsl: spacing, sigma, two of padding. */
const REFINE_UNIFORM_BYTES = 16;
/** Mirrored from `sidecar_color::depth`: the model's own edge, the cap on a refined map, and its sigma. */
const DEPTH_MODEL_EDGE = 518;
const REFINE_MAX_EDGE = 2048;
const REFINE_SIGMA = 0.08;

/** `sidecar_color::depth::refined_size`: the frame, its long edge capped, its proportions kept. */
export function refinedSize(frame: [number, number]): [number, number] {
	const long = Math.max(frame[0], frame[1]);
	if (long <= REFINE_MAX_EDGE || long === 0) return frame;
	const scale = REFINE_MAX_EDGE / long;
	return [Math.max(1, Math.round(frame[0] * scale)), Math.max(1, Math.round(frame[1] * scale))];
}

export type Binding = 'uniform' | 'texture' | 'filtered' | 'sampler' | 'storage';

export async function kernel(device: GPUDevice, label: string, code: string, bindings: Binding[]): Promise<GPUComputePipeline> {
	const module = device.createShaderModule({ label, code });
	const errors = (await module.getCompilationInfo()).messages.filter((m) => m.type === 'error');
	if (errors.length) throw new Error(`${label}: ${errors.map((e) => `${e.lineNum}:${e.linePos} ${e.message}`).join('; ')}`);
	const visibility = GPUShaderStage.COMPUTE;
	const layout = device.createBindGroupLayout({
		label,
		entries: bindings.map((b, binding): GPUBindGroupLayoutEntry => {
			switch (b) {
				case 'uniform':
					return { binding, visibility, buffer: { type: 'uniform' } };
				case 'texture':
					return { binding, visibility, texture: { sampleType: 'unfilterable-float' } };
				case 'filtered':
					return { binding, visibility, texture: { sampleType: 'float' } };
				case 'sampler':
					return { binding, visibility, sampler: { type: 'filtering' } };
				case 'storage':
					return { binding, visibility, storageTexture: { access: 'write-only', format: 'rgba16float' } };
			}
		})
	});
	return device.createComputePipelineAsync({ label, layout: device.createPipelineLayout({ bindGroupLayouts: [layout] }), compute: { module, entryPoint: 'main' } });
}

export class DepthPass {
	private output?: GPUTexture;
	private small?: GPUTexture;
	private blurred?: GPUTexture;
	private refined?: GPUTexture;

	private constructor(
		private device: GPUDevice,
		private down: GPUComputePipeline,
		private gather: GPUComputePipeline,
		private compose: GPUComputePipeline,
		private refinePipeline: GPUComputePipeline,
		private ubo: GPUBuffer,
		private refineUbo: GPUBuffer,
		private sampler: GPUSampler
	) {}

	static async create(device: GPUDevice): Promise<DepthPass> {
		const [down, gather, compose, refine] = await Promise.all([
			kernel(device, 'depth down', downWgsl, ['uniform', 'texture', 'filtered', 'sampler', 'storage']),
			kernel(device, 'depth gather', gatherWgsl, ['uniform', 'filtered', 'filtered', 'sampler', 'storage']),
			kernel(device, 'depth compose', composeWgsl, ['uniform', 'texture', 'filtered', 'filtered', 'sampler', 'storage']),
			kernel(device, 'depth refine', refineWgsl, ['uniform', 'filtered', 'filtered', 'sampler', 'storage'])
		]);
		const ubo = device.createBuffer({ label: 'depth params', size: DEPTH_UNIFORM_BYTES, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST });
		const refineUbo = device.createBuffer({ label: 'depth refine params', size: REFINE_UNIFORM_BYTES, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST });
		const sampler = device.createSampler({ magFilter: 'linear', minFilter: 'linear' });
		return new DepthPass(device, down, gather, compose, refine, ubo, refineUbo, sampler);
	}

	private texture(have: GPUTexture | undefined, w: number, h: number, label: string): GPUTexture {
		if (have?.width === w && have?.height === h) return have;
		have?.destroy();
		return this.device.createTexture({ label, size: [w, h], format: 'rgba16float', usage: GPUTextureUsage.STORAGE_BINDING | GPUTextureUsage.TEXTURE_BINDING });
	}

	private dispatch(encoder: GPUCommandEncoder, pipeline: GPUComputePipeline, resources: GPUBindingResource[], size: [number, number], ubo = this.ubo) {
		const bind = this.device.createBindGroup({
			layout: pipeline.getBindGroupLayout(0),
			entries: [{ binding: 0, resource: { buffer: ubo } }, ...resources.map((resource, i) => ({ binding: i + 1, resource }))]
		});
		const pass = encoder.beginComputePass();
		pass.setPipeline(pipeline);
		pass.setBindGroup(0, bind);
		pass.dispatchWorkgroups(Math.ceil(size[0] / 16), Math.ceil(size[1] / 16));
		pass.end();
	}

	/**
	 * The depth map with the photo's own edges (`client/depth_refine.wgsl`), as a texture that stays this
	 * pass's until the next refinement. `photo` and `depth` are both read across the whole frame, so the map
	 * can be — and is — far smaller than the photo: this is what upsamples it. Returns `depth` unchanged when
	 * the frame is small enough that there is nothing to repair.
	 *
	 * It depends on the photo and the map alone, not on any slider, so the caller keeps the result and only
	 * calls this again when one of them changes.
	 */
	refine(photo: GPUTexture, depth: GPUTexture, frame: [number, number]): GPUTexture {
		const [w, h] = refinedSize(frame);
		const spacing = Math.max(w, h) / DEPTH_MODEL_EDGE;
		if (spacing < 1) return depth;
		const params = new Float32Array([spacing, REFINE_SIGMA, 0, 0]);
		this.device.queue.writeBuffer(this.refineUbo, 0, params);
		this.refined = this.texture(this.refined, w, h, 'depth refined');
		const encoder = this.device.createCommandEncoder();
		this.dispatch(encoder, this.refinePipeline, [photo.createView(), depth.createView(), this.sampler, this.refined.createView()], [w, h], this.refineUbo);
		this.device.queue.submit([encoder.finish()]);
		return this.refined;
	}

	/** `input` hazed and blurred by distance into a texture of its size, which stays this pass's until the next run.
	 *  `depth` is the depth mask in the same frame, lens-corrected as `input` is. */
	run(input: GPUTexture, depth: GPUTexture, uniform: Uint8Array): GPUTexture {
		if (uniform.byteLength !== DEPTH_UNIFORM_BYTES) throw new Error(`depth uniforms must be ${DEPTH_UNIFORM_BYTES} bytes`);
		this.device.queue.writeBuffer(this.ubo, 0, uniform);
		const f = new Float32Array(uniform.buffer, uniform.byteOffset, 16);
		const blurs = f[11] > 0.5;
		const k = Math.max(1, Math.floor(f[12]));
		const [w, h] = [input.width, input.height];
		this.output = this.texture(this.output, w, h, 'depth effects');
		const encoder = this.device.createCommandEncoder();
		let blurredView = input.createView();
		if (blurs) {
			const [sw, sh] = [Math.ceil(w / k), Math.ceil(h / k)];
			this.small = this.texture(this.small, sw, sh, 'depth small');
			this.blurred = this.texture(this.blurred, sw, sh, 'depth blurred');
			this.dispatch(encoder, this.down, [input.createView(), depth.createView(), this.sampler, this.small.createView()], [sw, sh]);
			this.dispatch(encoder, this.gather, [this.small.createView(), depth.createView(), this.sampler, this.blurred.createView()], [sw, sh]);
			blurredView = this.blurred.createView();
		}
		this.dispatch(encoder, this.compose, [input.createView(), blurredView, depth.createView(), this.sampler, this.output.createView()], [w, h]);
		this.device.queue.submit([encoder.finish()]);
		return this.output;
	}
}
