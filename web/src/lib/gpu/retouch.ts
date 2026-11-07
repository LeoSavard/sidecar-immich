// SPDX-License-Identifier: Apache-2.0
// Retouch in the browser: `sidecar_engine::retouch::RetouchPass` pass for pass — each spot `client/retouch.wgsl`
// over its own box, the box copied back, in order — on the proxy before the lens pass.
import retouchWgsl from './wgsl/client_retouch.wgsl?raw';
import { kernel } from './depth';
import type { Spot } from '$lib/types';

const KIND = { heal: 0, clone: 1, redEye: 2 } as const;

export class RetouchPass {
	private constructor(
		private device: GPUDevice,
		private pipeline: GPUComputePipeline,
		private sampler: GPUSampler
	) {}

	static async create(device: GPUDevice): Promise<RetouchPass> {
		const pipeline = await kernel(device, 'retouch', retouchWgsl, ['uniform', 'filtered', 'sampler', 'storage']);
		return new RetouchPass(device, pipeline, device.createSampler({ magFilter: 'linear', minFilter: 'linear' }));
	}

	/** A texture `src`'s size and format that `apply` can write into and the passes after it can read. */
	target(src: GPUTexture, label: string): GPUTexture {
		return this.device.createTexture({
			label,
			size: [src.width, src.height],
			format: 'rgba16float',
			usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST | GPUTextureUsage.COPY_SRC | GPUTextureUsage.STORAGE_BINDING
		});
	}

	/**
	 * `src` with `spots` applied in order, into `out` (from `target`; `scratch` likewise): positions in `src`'s
	 * frame as fractions, radius a share of its long edge — `sidecar_engine::retouch::PixelSpot::new` with the
	 * identity. `wb` is the camera's as-shot multipliers, for red eye.
	 */
	apply(src: GPUTexture, out: GPUTexture, scratch: GPUTexture, spots: Spot[], wb: [number, number, number]) {
		const [w, h] = [src.width, src.height];
		const encoder = this.device.createCommandEncoder();
		encoder.copyTextureToTexture({ texture: src }, { texture: out }, [w, h]);
		const buffers: GPUBuffer[] = [];
		for (const s of spots) {
			const radius = Math.fround(Math.max(0, s.radius) * Math.max(w, h));
			const at = [Math.fround(s.at[0] * w), Math.fround(s.at[1] * h)];
			const from = [Math.fround(s.from[0] * w), Math.fround(s.from[1] * h)];
			const reach = radius * 1.12 + 2;
			const clamp = (v: number, hi: number) => Math.min(hi, Math.max(0, v));
			const [x0, y0] = [clamp(Math.floor(at[0] - reach), w), clamp(Math.floor(at[1] - reach), h)];
			const [x1, y1] = [clamp(Math.ceil(at[0] + reach), w), clamp(Math.ceil(at[1] + reach), h)];
			if (x1 <= x0 || y1 <= y0) continue;
			const data = new ArrayBuffer(64);
			const f = new Float32Array(data);
			const u = new Uint32Array(data);
			f.set([wb[0], wb[1], wb[2], 0], 0);
			f.set([at[0], at[1], from[0], from[1]], 4);
			f[8] = Math.max(radius, 0.5);
			f[9] = Math.min(Math.max(s.feather, 0), 100) / 100;
			f[10] = Math.min(Math.max(s.opacity, 0), 100) / 100;
			u[11] = KIND[s.kind];
			u.set([x0, y0, x1 - x0, y1 - y0], 12);
			const ubo = this.device.createBuffer({ size: 64, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST });
			this.device.queue.writeBuffer(ubo, 0, data);
			buffers.push(ubo);
			const bind = this.device.createBindGroup({
				layout: this.pipeline.getBindGroupLayout(0),
				entries: [
					{ binding: 0, resource: { buffer: ubo } },
					{ binding: 1, resource: out.createView() },
					{ binding: 2, resource: this.sampler },
					{ binding: 3, resource: scratch.createView() }
				]
			});
			const pass = encoder.beginComputePass();
			pass.setPipeline(this.pipeline);
			pass.setBindGroup(0, bind);
			pass.dispatchWorkgroups(Math.ceil((x1 - x0) / 16), Math.ceil((y1 - y0) / 16));
			pass.end();
			encoder.copyTextureToTexture({ texture: scratch, origin: [x0, y0] }, { texture: out, origin: [x0, y0] }, [x1 - x0, y1 - y0]);
		}
		this.device.queue.submit([encoder.finish()]);
		this.device.queue.onSubmittedWorkDone().then(() => buffers.forEach((b) => b.destroy()));
	}
}
