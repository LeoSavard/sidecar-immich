// SPDX-License-Identifier: Apache-2.0
// The whole-frame maps develop reads, in the browser, pass for pass as on the server:
// - Texture and Clarity's (`DevelopPass::local_map`): two self-guided filters (`sidecar_engine::guided`, the same
//   `client/guided_*.wgsl`) of the averaged-down picture's log luminance, their coefficients packed side by side
//   (`client/local_pack.wgsl`).
// - Dehaze's (`sidecar_engine::haze`): the dark channel, the airlight, the transmission (`client/haze_*.wgsl`), and
//   the guided filter's coefficients for it.
import statsWgsl from './wgsl/client_guided_stats.wgsl?raw';
import boxWgsl from './wgsl/client_guided_box.wgsl?raw';
import coefWgsl from './wgsl/client_guided_coef.wgsl?raw';
import packWgsl from './wgsl/client_local_pack.wgsl?raw';
import hazeDarkWgsl from './wgsl/client_haze_dark.wgsl?raw';
import hazeMinWgsl from './wgsl/client_haze_min.wgsl?raw';
import hazeAirWgsl from './wgsl/client_haze_air.wgsl?raw';
import hazeTransWgsl from './wgsl/client_haze_trans.wgsl?raw';
import { kernel } from './depth';

/** `sidecar_color::params`: each band's radius as a share of the long edge, and ε in log₂ units squared. */
const TEXTURE_RADIUS = 0.003;
const TEXTURE_EPS = 0.04;
const CLARITY_RADIUS = 0.025;
const CLARITY_EPS = 0.25;
/** `sidecar_engine::haze`: the dark channel's patch and the refinement's radius (shares of the long edge), its ε,
 * and ω. */
const HAZE_PATCH = 0.012;
const HAZE_REFINE_RADIUS = 0.04;
const HAZE_REFINE_EPS = 0.01;
const HAZE_OMEGA = 0.95;
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
		private hazeDark: GPUComputePipeline,
		private hazeMin: GPUComputePipeline,
		private hazeAir: GPUComputePipeline,
		private hazeTrans: GPUComputePipeline,
		private sampler: GPUSampler,
		/** An airlight of 1, for the first dark channel. */
		private unit: GPUTexture
	) {}

	static async create(device: GPUDevice): Promise<LocalMap> {
		const [stats, box, coef, pack, hazeDark, hazeMin, hazeAir, hazeTrans] = await Promise.all([
			kernel(device, 'guided stats', statsWgsl, ['uniform', 'filtered', 'filtered', 'sampler', WIDE]),
			kernel(device, 'guided box', boxWgsl, ['uniform', 'texture', WIDE]),
			kernel(device, 'guided coef', coefWgsl, ['uniform', 'texture', WIDE]),
			kernel(device, 'local pack', packWgsl, ['uniform', 'texture', 'texture', WIDE]),
			kernel(device, 'haze dark', hazeDarkWgsl, ['uniform', 'texture', 'texture', WIDE]),
			kernel(device, 'haze min', hazeMinWgsl, ['uniform', 'texture', WIDE]),
			kernel(device, 'haze air', hazeAirWgsl, ['uniform', 'texture', WIDE]),
			kernel(device, 'haze trans', hazeTransWgsl, ['uniform', 'texture', 'storage'])
		]);
		const unit = device.createTexture({ label: 'unit airlight', size: [1, 1], format: 'rgba32float', usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST });
		device.queue.writeTexture({ texture: unit }, new Float32Array([1, 1, 1, 1]), { bytesPerRow: 16 }, [1, 1]);
		return new LocalMap(device, stats, box, coef, pack, hazeDark, hazeMin, hazeAir, hazeTrans, device.createSampler({ magFilter: 'linear', minFilter: 'linear' }), unit);
	}

	private wide(label: string, w: number, h: number): GPUTexture {
		return this.device.createTexture({ label, size: [w, h], format: 'rgba32float', usage: GPUTextureUsage.STORAGE_BINDING | GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_SRC });
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
	private params(size: [number, number], radius: number, axis: number, eps: number, channel = GUIDE): GPUBuffer {
		const buf = this.device.createBuffer({ size: 32, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST });
		const data = new ArrayBuffer(32);
		const u = new Uint32Array(data);
		const f = new Float32Array(data);
		u[0] = size[0];
		u[1] = size[1];
		u[2] = radius;
		u[3] = axis;
		f[4] = Math.max(eps, 1e-12);
		u[5] = channel;
		u[6] = 1;
		this.device.queue.writeBuffer(buf, 0, data);
		return buf;
	}

	/** One band's smoothed coefficients (mean a, mean b) on `small`'s grid: `GuidedFilter::coefficients`, filtering
	 * `input`'s `channel` (the guide's own value by default). */
	private band(encoder: GPUCommandEncoder, small: GPUTexture, radius: number, eps: number, temps: GPUBuffer[], scratch: GPUTexture[], input = small, channel = GUIDE): GPUTexture {
		const size: [number, number] = [small.width, small.height];
		const r = Math.max(1, Math.round(Math.fround(Math.max(1, radius))));
		const [p, across, down] = [0, 0, 1].map((axis) => this.params(size, r, axis, eps, channel));
		temps.push(p, across, down);
		const a = this.wide('guided a', ...size);
		const b = this.wide('guided b', ...size);
		const v = (t: GPUTexture) => t.createView();
		this.run(encoder, this.stats, [{ buffer: p }, v(small), v(input), this.sampler, v(a)], size);
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

	/**
	 * Dehaze's map for a frame `frame` pixels across from `small` (averaged down by `k`), under the develop uniform's
	 * white balance and camera matrix (`uniform`, as floats): the transmission's coefficients into `map` and the
	 * airlight into `air` (1×1), as `sidecar_engine::haze::Haze::map` makes them.
	 */
	haze(small: GPUTexture, frame: [number, number], k: number, uniform: Float32Array, map: GPUTexture, air: GPUTexture) {
		const texels = Math.max(frame[0], frame[1]) / k;
		const size: [number, number] = [small.width, small.height];
		const temps: GPUBuffer[] = [];
		const scratch: GPUTexture[] = [];
		// `HazeParams` (shaders/uniforms/haze_params.wgsl): cam_to_work, wb, radius, axis, ω — 80 bytes.
		const hazeParams = (axis: number) => {
			const data = new ArrayBuffer(80);
			const f = new Float32Array(data);
			const u = new Uint32Array(data);
			f.set(uniform.subarray(4, 16), 0);
			f.set(uniform.subarray(0, 3), 12);
			u[16] = Math.max(1, Math.round(HAZE_PATCH * texels));
			u[17] = axis;
			f[18] = HAZE_OMEGA;
			const buf = this.device.createBuffer({ size: 80, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST });
			this.device.queue.writeBuffer(buf, 0, data);
			temps.push(buf);
			return buf;
		};
		const [p, down] = [hazeParams(0), hazeParams(1)];
		const [a, b] = [this.wide('haze a', ...size), this.wide('haze b', ...size)];
		const coarse = this.device.createTexture({ label: 'haze coarse', size, format: 'rgba16float', usage: GPUTextureUsage.STORAGE_BINDING | GPUTextureUsage.TEXTURE_BINDING });
		scratch.push(a, b, coarse);
		const v = (t: GPUTexture) => t.createView();
		const encoder = this.device.createCommandEncoder();
		const minimum = () => {
			this.run(encoder, this.hazeMin, [{ buffer: p }, v(a), v(b)], size);
			this.run(encoder, this.hazeMin, [{ buffer: down }, v(b), v(a)], size);
		};
		this.run(encoder, this.hazeDark, [{ buffer: p }, v(small), v(this.unit), v(a)], size);
		minimum();
		this.run(encoder, this.hazeAir, [{ buffer: p }, v(a), v(air)], [16, 16]);
		this.run(encoder, this.hazeDark, [{ buffer: p }, v(small), v(air), v(a)], size);
		minimum();
		this.run(encoder, this.hazeTrans, [{ buffer: p }, v(a), v(coarse)], size);
		const coef = this.band(encoder, small, HAZE_REFINE_RADIUS * texels, HAZE_REFINE_EPS, temps, scratch, coarse, 0);
		encoder.copyTextureToTexture({ texture: coef }, { texture: map }, size);
		scratch.push(coef);
		this.device.queue.submit([encoder.finish()]);
		this.device.queue.onSubmittedWorkDone().then(() => {
			for (const t of scratch) t.destroy();
			for (const b of temps) b.destroy();
		});
	}
}
