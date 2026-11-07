// SPDX-License-Identifier: Apache-2.0
// Texture and Clarity's map in the browser: `DevelopPass::local_map` (crates/engine/src/develop.rs) pass for pass —
// two self-guided filters (`sidecar_engine::guided`, the same `client/guided_*.wgsl`) of the averaged-down
// picture's log luminance, their coefficients packed side by side (`client/local_pack.wgsl`) for develop to read.
import statsWgsl from './wgsl/client_guided_stats.wgsl?raw';
import boxWgsl from './wgsl/client_guided_box.wgsl?raw';
import coefWgsl from './wgsl/client_guided_coef.wgsl?raw';
import packWgsl from './wgsl/client_local_pack.wgsl?raw';
import { kernel } from './depth';

/** `sidecar_color::params`: each band's radius as a share of the long edge, and ε in log₂ units squared. */
const TEXTURE_RADIUS = 0.003;
const TEXTURE_EPS = 0.04;
const CLARITY_RADIUS = 0.025;
const CLARITY_EPS = 0.25;
/** `sidecar_engine::guided::GUIDE`: filter the guide's own value. */
const GUIDE = 4;
const WIDE = { storage: 'rgba32float' as GPUTextureFormat };

export class LocalMap {
	private constructor(
		private device: GPUDevice,
		private stats: GPUComputePipeline,
		private box: GPUComputePipeline,
		private coef: GPUComputePipeline,
		private pack: GPUComputePipeline,
		private sampler: GPUSampler
	) {}

	static async create(device: GPUDevice): Promise<LocalMap> {
		const [stats, box, coef, pack] = await Promise.all([
			kernel(device, 'guided stats', statsWgsl, ['uniform', 'filtered', 'filtered', 'sampler', WIDE]),
			kernel(device, 'guided box', boxWgsl, ['uniform', 'texture', WIDE]),
			kernel(device, 'guided coef', coefWgsl, ['uniform', 'texture', WIDE]),
			kernel(device, 'local pack', packWgsl, ['uniform', 'texture', 'texture', WIDE])
		]);
		return new LocalMap(device, stats, box, coef, pack, device.createSampler({ magFilter: 'linear', minFilter: 'linear' }));
	}

	private wide(label: string, w: number, h: number): GPUTexture {
		return this.device.createTexture({ label, size: [w, h], format: 'rgba32float', usage: GPUTextureUsage.STORAGE_BINDING | GPUTextureUsage.TEXTURE_BINDING });
	}

	private run(encoder: GPUCommandEncoder, pipeline: GPUComputePipeline, resources: GPUBindingResource[], size: [number, number]) {
		const bind = this.device.createBindGroup({
			layout: pipeline.getBindGroupLayout(0),
			entries: resources.map((resource, binding) => ({ binding, resource }))
		});
		const pass = encoder.beginComputePass();
		pass.setPipeline(pipeline);
		pass.setBindGroup(0, bind);
		pass.dispatchWorkgroups(Math.ceil(size[0] / 16), Math.ceil(size[1] / 16));
		pass.end();
	}

	/** `GuidedParams` (shaders/uniforms/guided_params.wgsl): 32 bytes. */
	private params(size: [number, number], radius: number, axis: number, eps: number): GPUBuffer {
		const buf = this.device.createBuffer({ size: 32, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST });
		const data = new ArrayBuffer(32);
		const u = new Uint32Array(data);
		const f = new Float32Array(data);
		u[0] = size[0];
		u[1] = size[1];
		u[2] = radius;
		u[3] = axis;
		f[4] = Math.max(eps, 1e-12);
		u[5] = GUIDE;
		u[6] = 1;
		this.device.queue.writeBuffer(buf, 0, data);
		return buf;
	}

	/** One band's smoothed coefficients (mean a, mean b) on `small`'s grid: `GuidedFilter::coefficients`. */
	private band(encoder: GPUCommandEncoder, small: GPUTexture, radius: number, eps: number, temps: GPUBuffer[], scratch: GPUTexture[]): GPUTexture {
		const size: [number, number] = [small.width, small.height];
		const r = Math.max(1, Math.round(Math.fround(Math.max(1, radius))));
		const [p, across, down] = [0, 0, 1].map((axis) => this.params(size, r, axis, eps));
		temps.push(p, across, down);
		const a = this.wide('guided a', ...size);
		const b = this.wide('guided b', ...size);
		const v = (t: GPUTexture) => t.createView();
		this.run(encoder, this.stats, [{ buffer: p }, v(small), v(small), this.sampler, v(a)], size);
		this.run(encoder, this.box, [{ buffer: across }, v(a), v(b)], size);
		this.run(encoder, this.box, [{ buffer: down }, v(b), v(a)], size);
		this.run(encoder, this.coef, [{ buffer: p }, v(a), v(b)], size);
		this.run(encoder, this.box, [{ buffer: across }, v(b), v(a)], size);
		this.run(encoder, this.box, [{ buffer: down }, v(a), v(b)], size);
		scratch.push(a);
		return b;
	}

	/**
	 * The map for a frame `frame` pixels across from `small`, the whole frame averaged down by `k` (the
	 * adjacency map's source, `developer::map_scale`). Into `out` (rgba32float, `small`'s size).
	 */
	build(small: GPUTexture, frame: [number, number], k: number, out: GPUTexture) {
		const texels = Math.max(frame[0], frame[1]) / k;
		const temps: GPUBuffer[] = [];
		const scratch: GPUTexture[] = [];
		const encoder = this.device.createCommandEncoder();
		const fine = this.band(encoder, small, TEXTURE_RADIUS * texels, TEXTURE_EPS, temps, scratch);
		const mid = this.band(encoder, small, CLARITY_RADIUS * texels, CLARITY_EPS, temps, scratch);
		const p = this.device.createBuffer({ size: 16, usage: GPUBufferUsage.UNIFORM });
		temps.push(p);
		this.run(encoder, this.pack, [{ buffer: p }, fine.createView(), mid.createView(), out.createView()], [out.width, out.height]);
		this.device.queue.submit([encoder.finish()]);
		this.device.queue.onSubmittedWorkDone().then(() => {
			for (const t of [fine, mid, ...scratch]) t.destroy();
			for (const b of temps) b.destroy();
		});
	}
}
