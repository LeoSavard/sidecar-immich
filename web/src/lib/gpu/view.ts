// SPDX-License-Identifier: Apache-2.0
// View pass: developedTex → canvas with pan/zoom, clip overlays and dither. Mirrors shaders/client/view.wgsl.
import viewWgsl from './wgsl/client_view.wgsl?raw';

export const CLIP_HIGHS = 1;
export const CLIP_LOWS = 2;
export const PEAKING = 4;

/**
 * Threshold (0…1) on the worker's focus_mask that counts as sharp. The mask is a Laplacian energy over
 * local contrast squashed through raw / (raw + 1), so 0.5 is where a moderately sharp edge lands; a
 * first-pass value, to tune against real photos rather than derive analytically.
 */
const PEAK_THRESHOLD = 0.5;

const srgbDecode = (v: number) => (v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);

export type Viewport = { zoom: number; panX: number; panY: number };

export class ViewPass {
	private pipeline: GPURenderPipeline;
	private ubo: GPUBuffer;
	private sampler: GPUSampler;
	private bindGroup?: GPUBindGroup;
	private boundTexture?: GPUTexture;
	private boundFocusMask?: GPUTexture;
	private focusMask: GPUTexture;
	private params = new ArrayBuffer(80);

	private constructor(
		private device: GPUDevice,
		private context: GPUCanvasContext,
		private format: GPUTextureFormat,
		pipeline: GPURenderPipeline
	) {
		this.pipeline = pipeline;
		this.ubo = device.createBuffer({ label: 'view params', size: 80, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST });
		this.sampler = device.createSampler({ magFilter: 'linear', minFilter: 'linear' });
		// Placeholder until loadFocusMask() lands the real one; never triggers peaking (0 < any threshold > 0).
		this.focusMask = device.createTexture({ label: 'focus mask placeholder', size: [1, 1], format: 'r8unorm', usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST });
		device.queue.writeTexture({ texture: this.focusMask }, new Uint8Array([0]), {}, [1, 1]);
	}

	/** Decode a focus_mask PNG (grayscale, from `/api/assets/{key}/focus`) and upload it as `r8unorm`. */
	async loadFocusMask(pngBytes: ArrayBuffer) {
		const bitmap = await createImageBitmap(new Blob([pngBytes], { type: 'image/png' }));
		const { width, height } = bitmap; // bitmap.close() below zeroes these, so capture them first
		const canvas = new OffscreenCanvas(width, height);
		const ctx = canvas.getContext('2d')!;
		ctx.drawImage(bitmap, 0, 0);
		const { data } = ctx.getImageData(0, 0, width, height);
		bitmap.close();
		// Grayscale PNGs decode to RGBA with R = G = B; take the one channel we need.
		const gray = new Uint8Array(width * height);
		for (let i = 0; i < gray.length; i++) gray[i] = data[i * 4];
		this.focusMask.destroy();
		this.focusMask = this.device.createTexture({ label: 'focus mask', size: [width, height], format: 'r8unorm', usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST });
		this.device.queue.writeTexture({ texture: this.focusMask }, gray, { bytesPerRow: width }, [width, height]);
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
		dv.setFloat32(64, PEAK_THRESHOLD, true);
		this.device.queue.writeBuffer(this.ubo, 0, this.params);
		if (this.boundTexture !== developed || this.boundFocusMask !== this.focusMask) {
			this.bindGroup = this.device.createBindGroup({
				layout: this.pipeline.getBindGroupLayout(0),
				entries: [
					{ binding: 0, resource: { buffer: this.ubo } },
					{ binding: 1, resource: developed.createView() },
					{ binding: 2, resource: this.sampler },
					{ binding: 3, resource: this.focusMask.createView() }
				]
			});
			this.boundTexture = developed;
			this.boundFocusMask = this.focusMask;
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
