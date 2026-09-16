// SPDX-License-Identifier: Apache-2.0
// View pass: developedTex → canvas with pan/zoom, clip overlays and dither. Mirrors shaders/client/view.wgsl.
import viewWgsl from './wgsl/client_view.wgsl?raw';

export const CLIP_HIGHS = 1;
export const CLIP_LOWS = 2;

const srgbDecode = (v: number) => (v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);

export type Viewport = { zoom: number; panX: number; panY: number };

export class ViewPass {
	private pipeline: GPURenderPipeline;
	private ubo: GPUBuffer;
	private sampler: GPUSampler;
	private bindGroup?: GPUBindGroup;
	private boundTexture?: GPUTexture;
	private params = new ArrayBuffer(64);

	private constructor(
		private device: GPUDevice,
		private context: GPUCanvasContext,
		private format: GPUTextureFormat,
		pipeline: GPURenderPipeline
	) {
		this.pipeline = pipeline;
		this.ubo = device.createBuffer({ label: 'view params', size: 64, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST });
		this.sampler = device.createSampler({ magFilter: 'linear', minFilter: 'linear' });
	}

	static async create(device: GPUDevice, canvas: HTMLCanvasElement, displayP3: boolean): Promise<ViewPass> {
		const context = canvas.getContext('webgpu');
		if (!context) throw new Error('Could not create a WebGPU canvas.');
		const format = navigator.gpu.getPreferredCanvasFormat();
		context.configure({ device, format, alphaMode: 'opaque', colorSpace: displayP3 ? 'display-p3' : 'srgb' });
		const module = device.createShaderModule({ label: 'view', code: viewWgsl });
		const pipeline = await device.createRenderPipelineAsync({
			label: 'view',
			layout: 'auto',
			vertex: { module, entryPoint: 'vs' },
			fragment: { module, entryPoint: 'fs', targets: [{ format }] }
		});
		return new ViewPass(device, context, format, pipeline);
	}

	/** Fit the image in the canvas, then apply zoom (1 = fit) and pan (canvas pixels). */
	render(developed: GPUTexture, canvas: HTMLCanvasElement, view: Viewport, flags: number) {
		const cw = canvas.width;
		const ch = canvas.height;
		const fit = Math.min(cw / developed.width, ch / developed.height);
		const dw = developed.width * fit * view.zoom;
		const dh = developed.height * fit * view.zoom;
		const ox = (cw - dw) / 2 + view.panX;
		const oy = (ch - dh) / 2 + view.panY;
		const f = new Float32Array(this.params);
		f.set([1 / dw, 0, 0, 0, 0, 1 / dh, 0, 0, -ox / dw, -oy / dh, 1, 0]);
		const dv = new DataView(this.params);
		dv.setUint32(48, flags, true);
		dv.setFloat32(52, srgbDecode(254.5 / 255), true);
		dv.setFloat32(56, srgbDecode(0.5 / 255), true);
		dv.setFloat32(60, 0x16 / 255, true);
		this.device.queue.writeBuffer(this.ubo, 0, this.params);
		if (this.boundTexture !== developed) {
			this.bindGroup = this.device.createBindGroup({
				layout: this.pipeline.getBindGroupLayout(0),
				entries: [
					{ binding: 0, resource: { buffer: this.ubo } },
					{ binding: 1, resource: developed.createView() },
					{ binding: 2, resource: this.sampler }
				]
			});
			this.boundTexture = developed;
		}
		const encoder = this.device.createCommandEncoder();
		const pass = encoder.beginRenderPass({
			colorAttachments: [{ view: this.context.getCurrentTexture().createView(), loadOp: 'clear', storeOp: 'store', clearValue: [0.086, 0.086, 0.086, 1] }]
		});
		pass.setPipeline(this.pipeline);
		pass.setBindGroup(0, this.bindGroup!);
		pass.draw(3);
		pass.end();
		this.device.queue.submit([encoder.finish()]);
	}
}
