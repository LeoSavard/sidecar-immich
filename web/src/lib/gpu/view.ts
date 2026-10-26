// SPDX-License-Identifier: Apache-2.0
// View pass: developedTex → canvas with pan/zoom, clip overlays and dither. Mirrors shaders/client/view.wgsl.
import viewWgsl from './wgsl/client_view.wgsl?raw';
import { greyTexture } from '$lib/masks';
import { srgbDecode } from '$lib/color/transfer';

export const CLIP_HIGHS = 1;
export const CLIP_LOWS = 2;
export const PEAKING = 4;
/** Show a mask: desaturate and dim what it leaves out (DESIGN.md §15), red and blue stay for clipping. */
export const SHOW_MASK = 8;
/** With SHOW_MASK: the mask is inverted, as its adjustment is. */
export const MASK_INVERTED = 16;
const VIEW_UNIFORM_BYTES = 128;

/**
 * What the canvas shows of the developed picture: the output of a geometry (`Frame.toPicture`, output 0…1 →
 * picture 0…1, row-major), `size` pixels.
 */
export type ViewGeometry = { toPicture: number[][]; size: [number, number] };

/**
 * Threshold (0…1) on the worker's focus_mask that counts as sharp. The mask is a Laplacian energy over
 * local contrast squashed through raw / (raw + 1), so 0.5 is where a moderately sharp edge lands; a
 * first-pass value, to tune against real photos rather than derive analytically.
 */
const PEAK_THRESHOLD = 0.5;


export type Viewport = { zoom: number; panX: number; panY: number };

export class ViewPass {
	private pipeline: GPURenderPipeline;
	private ubo: GPUBuffer;
	private sampler: GPUSampler;
	private bindGroup?: GPUBindGroup;
	private boundTexture?: GPUTexture;
	private boundFocusMask?: GPUTexture;
	private boundOverlay?: GPUTexture;
	/** The mask SHOW_MASK shows, in the developed texture's frame; a 1×1 placeholder when none. */
	private overlay: GPUTexture;
	private noOverlay: GPUTexture;
	private focusMask: GPUTexture;
	/** The mask as decoded, before any lens correction; `focusMask` is what the view samples. */
	private focusSource: GPUTexture;
	private params = new ArrayBuffer(VIEW_UNIFORM_BYTES);

	private constructor(
		private device: GPUDevice,
		private context: GPUCanvasContext,
		private format: GPUTextureFormat,
		pipeline: GPURenderPipeline
	) {
		this.pipeline = pipeline;
		this.ubo = device.createBuffer({ label: 'view params', size: VIEW_UNIFORM_BYTES, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST });
		this.sampler = device.createSampler({ magFilter: 'linear', minFilter: 'linear' });
		// Placeholder until loadFocusMask() lands the real one; never triggers peaking (0 < any threshold > 0).
		this.focusMask = device.createTexture({ label: 'focus mask placeholder', size: [1, 1], format: 'r8unorm', usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST });
		device.queue.writeTexture({ texture: this.focusMask }, new Uint8Array([0]), {}, [1, 1]);
		this.focusSource = this.focusMask;
		this.noOverlay = device.createTexture({ label: 'no mask', size: [1, 1], format: 'r8unorm', usage: GPUTextureUsage.TEXTURE_BINDING });
		this.overlay = this.noOverlay;
	}

	/** The focus mask in sensor geometry, for the lens pass to warp. */
	get focusMaskSource(): GPUTexture {
		return this.focusSource;
	}

	/** Sample `texture` (a lens-corrected copy of the mask) as the focus mask; `undefined` restores the original. */
	useFocusMask(texture?: GPUTexture) {
		this.focusMask = texture ?? this.focusSource;
	}

	/** Decode a focus_mask PNG (grayscale, from `/api/assets/{key}/focus`) and upload it as `r8unorm`. */
	async loadFocusMask(pngBytes: ArrayBuffer) {
		const texture = await greyTexture(this.device, pngBytes, 'focus mask');
		this.focusSource.destroy();
		this.focusSource = texture;
		this.focusMask = this.focusSource;
	}

	/** The mask to show with SHOW_MASK, lens-corrected like the picture; `undefined` for none. */
	useOverlay(texture?: GPUTexture) {
		this.overlay = texture ?? this.noOverlay;
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

	/** Draw in another colour space from the next frame on: only the canvas is configured again. */
	setDisplayP3(displayP3: boolean) {
		this.context.configure({ device: this.device, format: this.format, alphaMode: 'opaque', colorSpace: displayP3 ? 'display-p3' : 'srgb' });
	}

	/** Fit the output in the canvas, then apply zoom (1 = fit) and pan (canvas pixels). */
	render(developed: GPUTexture, canvas: HTMLCanvasElement, view: Viewport, flags: number, geometry?: ViewGeometry) {
		const [ow, oh] = geometry?.size ?? [developed.width, developed.height];
		const cw = canvas.width;
		const ch = canvas.height;
		const fit = Math.min(cw / ow, ch / oh);
		const dw = ow * fit * view.zoom;
		const dh = oh * fit * view.zoom;
		const ox = (cw - dw) / 2 + view.panX;
		const oy = (ch - dh) / 2 + view.panY;
		const f = new Float32Array(this.params);
		f.fill(0);
		f.set([1 / dw, 0, 0, 0, 0, 1 / dh, 0, 0, -ox / dw, -oy / dh, 1, 0]);
		const m = geometry?.toPicture ?? [[1, 0, 0], [0, 1, 0], [0, 0, 1]];
		// WGSL's mat3x3f is three padded columns.
		for (let j = 0; j < 3; j++) f.set([m[0][j], m[1][j], m[2][j], 0], 12 + j * 4);
		const dv = new DataView(this.params);
		dv.setUint32(96, flags, true);
		dv.setFloat32(100, srgbDecode(254.5 / 255), true);
		dv.setFloat32(104, srgbDecode(0.5 / 255), true);
		dv.setFloat32(108, 0x16 / 255, true);
		dv.setFloat32(112, PEAK_THRESHOLD, true);
		this.device.queue.writeBuffer(this.ubo, 0, this.params);
		if (this.boundTexture !== developed || this.boundFocusMask !== this.focusMask || this.boundOverlay !== this.overlay) {
			this.bindGroup = this.device.createBindGroup({
				layout: this.pipeline.getBindGroupLayout(0),
				entries: [
					{ binding: 0, resource: { buffer: this.ubo } },
					{ binding: 1, resource: developed.createView() },
					{ binding: 2, resource: this.sampler },
					{ binding: 3, resource: this.focusMask.createView() },
					{ binding: 4, resource: this.overlay.createView() }
				]
			});
			this.boundTexture = developed;
			this.boundFocusMask = this.focusMask;
			this.boundOverlay = this.overlay;
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
