// SPDX-License-Identifier: Apache-2.0
// Histogram compute over developedTex. At most one readback in flight; a newer request replaces a queued one.
import histogramWgsl from './wgsl/client_histogram.wgsl?raw';

export type HistogramData = { r: Uint32Array; g: Uint32Array; b: Uint32Array; luma: Uint32Array; clippedHigh: number; clippedLow: number; total: number };

const srgbDecode = (v: number) => (v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);
const BINS = 1026;

export class HistogramPass {
	private pipeline: GPUComputePipeline;
	private bins: GPUBuffer;
	private readback: GPUBuffer;
	private params: GPUBuffer;
	private busy = false;
	private queued: GPUTexture | null = null;

	private constructor(private device: GPUDevice, pipeline: GPUComputePipeline) {
		this.pipeline = pipeline;
		this.bins = device.createBuffer({ size: BINS * 4, usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC | GPUBufferUsage.COPY_DST });
		this.readback = device.createBuffer({ size: BINS * 4, usage: GPUBufferUsage.MAP_READ | GPUBufferUsage.COPY_DST });
		this.params = device.createBuffer({ size: 32, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST });
	}

	static async create(device: GPUDevice): Promise<HistogramPass> {
		const module = device.createShaderModule({ label: 'histogram', code: histogramWgsl });
		const pipeline = await device.createComputePipelineAsync({ label: 'histogram', layout: 'auto', compute: { module, entryPoint: 'main' } });
		return new HistogramPass(device, pipeline);
	}

	/** Compute on `developed`; calls `done` when the counts arrive. Skips work while a readback is pending. */
	run(developed: GPUTexture, stride: number, done: (h: HistogramData) => void) {
		if (this.busy) {
			this.queued = developed;
			return;
		}
		this.busy = true;
		const p = new ArrayBuffer(32);
		const dv = new DataView(p);
		dv.setUint32(0, developed.width, true);
		dv.setUint32(4, developed.height, true);
		dv.setUint32(8, stride, true);
		dv.setFloat32(16, srgbDecode(254.5 / 255), true);
		dv.setFloat32(20, srgbDecode(0.5 / 255), true);
		this.device.queue.writeBuffer(this.params, 0, p);
		this.device.queue.writeBuffer(this.bins, 0, new Uint32Array(BINS));
		const bind = this.device.createBindGroup({
			layout: this.pipeline.getBindGroupLayout(0),
			entries: [
				{ binding: 0, resource: developed.createView() },
				{ binding: 1, resource: { buffer: this.bins } },
				{ binding: 2, resource: { buffer: this.params } }
			]
		});
		const enc = this.device.createCommandEncoder();
		const pass = enc.beginComputePass();
		pass.setPipeline(this.pipeline);
		pass.setBindGroup(0, bind);
		pass.dispatchWorkgroups(Math.ceil(developed.width / stride / 16), Math.ceil(developed.height / stride / 16));
		pass.end();
		enc.copyBufferToBuffer(this.bins, 0, this.readback, 0, BINS * 4);
		this.device.queue.submit([enc.finish()]);
		this.readback.mapAsync(GPUMapMode.READ).then(() => {
			const all = new Uint32Array(this.readback.getMappedRange().slice(0));
			this.readback.unmap();
			const total = Math.ceil(developed.width / stride) * Math.ceil(developed.height / stride);
			done({ r: all.subarray(0, 256), g: all.subarray(256, 512), b: all.subarray(512, 768), luma: all.subarray(768, 1024), clippedHigh: all[1024], clippedLow: all[1025], total });
			this.busy = false;
			const next = this.queued;
			this.queued = null;
			if (next) this.run(next, stride, done);
		});
	}
}
