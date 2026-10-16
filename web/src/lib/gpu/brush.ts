// SPDX-License-Identifier: Apache-2.0
// Brush strokes on masks in the browser (DESIGN.md §15), with the worker's shaders (crates/engine/src/brush.rs):
// client_brush.wgsl draws a stroke's coverage, client_brush_composite.wgsl paints or erases it onto the mask,
// client_lens_mask.wgsl moves the finished mask through the lens correction. Masks are `r8unorm` at the
// proxy's size, in the photo's frame before lens correction — where strokes are stored.
import brushWgsl from './wgsl/client_brush.wgsl?raw';
import compositeWgsl from './wgsl/client_brush_composite.wgsl?raw';
import lensMaskWgsl from './wgsl/client_lens_mask.wgsl?raw';
import previewWgsl from './wgsl/client_mask_preview.wgsl?raw';
import type { Stroke } from '$lib/types';

const MASK: GPUTextureFormat = 'r8unorm';
const PAINT = 0;
const ERASE = 1;
const COPY = 2;

/** A stroke in a target's pixels: segments as `[ax, ay, bx, by]`. */
type StrokePx = { erase: boolean; radius: number; feather: number; flow: number; segments: number[][] };

/** `stroke` (fractions of the photo) in a target `size` pixels big; a lone point is a dab. */
export function strokePx(stroke: Stroke, size: [number, number]): StrokePx {
	const pts = stroke.points.map(([x, y]) => [x * size[0], y * size[1]]);
	const segments = pts.length === 1 ? [[...pts[0], ...pts[0]]] : pts.slice(1).map((p, i) => [...pts[i], ...p]);
	return { erase: stroke.erase, radius: Math.max(stroke.radius, 0) * Math.max(size[0], size[1]), feather: Math.min(Math.max(stroke.feather, 0), 1), flow: Math.min(Math.max(stroke.flow, 0), 1), segments };
}

async function compiled(device: GPUDevice, label: string, code: string) {
	const module = device.createShaderModule({ label, code });
	const errors = (await module.getCompilationInfo()).messages.filter((m) => m.type === 'error');
	if (errors.length) throw new Error(`${label}: ${errors.map((e) => `${e.lineNum}:${e.linePos} ${e.message}`).join('; ')}`);
	return module;
}

export class MaskPainter {
	private sampler: GPUSampler;
	private empty: GPUTexture;
	private brushUbo: GPUBuffer;
	private lensUbo: GPUBuffer;
	private compositeUbo: GPUBuffer;
	private previewUbo: GPUBuffer;
	/** Segments for one draw, grown as needed: every pointer move reuses it. */
	private segments?: GPUBuffer;
	/** Scratch at the mask size: a stroke's coverage, and the mask as it was before the stroke. */
	private coverage?: GPUTexture;
	private before?: GPUTexture;
	/** The stroke being drawn live: its target and the area its segments have touched so far. */
	private live?: { target: GPUTexture; stroke: StrokePx; box: [number, number, number, number] };

	private constructor(
		private device: GPUDevice,
		private brush: GPURenderPipeline,
		private composite: GPURenderPipeline,
		private lens: GPURenderPipeline,
		private preview: GPURenderPipeline
	) {
		this.sampler = device.createSampler({ magFilter: 'linear', minFilter: 'linear' });
		this.empty = this.mask([1, 1], 'empty mask');
		this.brushUbo = device.createBuffer({ size: 32, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST });
		this.lensUbo = device.createBuffer({ size: 1072, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST });
		this.compositeUbo = device.createBuffer({ size: 32, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST });
		this.previewUbo = device.createBuffer({ size: 16, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST });
	}

	static async create(device: GPUDevice): Promise<MaskPainter> {
		const [b, c, l, v] = await Promise.all([
			compiled(device, 'brush', brushWgsl),
			compiled(device, 'brush composite', compositeWgsl),
			compiled(device, 'lens mask', lensMaskWgsl),
			compiled(device, 'mask preview', previewWgsl)
		]);
		const target = (blend?: GPUBlendState): GPUColorTargetState => ({ format: MASK, blend });
		const max: GPUBlendComponent = { operation: 'max', srcFactor: 'one', dstFactor: 'one' };
		const [brush, composite, lens, preview] = await Promise.all([
			device.createRenderPipelineAsync({
				label: 'brush',
				layout: 'auto',
				vertex: { module: b, entryPoint: 'vs', buffers: [{ arrayStride: 16, stepMode: 'instance', attributes: [{ format: 'float32x4', offset: 0, shaderLocation: 0 }] }] },
				fragment: { module: b, entryPoint: 'fs', targets: [target({ color: max, alpha: max })] }
			}),
			device.createRenderPipelineAsync({ label: 'brush composite', layout: 'auto', vertex: { module: c, entryPoint: 'vs' }, fragment: { module: c, entryPoint: 'fs', targets: [target()] } }),
			device.createRenderPipelineAsync({ label: 'lens mask', layout: 'auto', vertex: { module: l, entryPoint: 'vs' }, fragment: { module: l, entryPoint: 'fs', targets: [target()] } }),
			device.createRenderPipelineAsync({ label: 'mask preview', layout: 'auto', vertex: { module: v, entryPoint: 'vs' }, fragment: { module: v, entryPoint: 'fs', targets: [{ format: 'rgba16float' }] } })
		]);
		return new MaskPainter(device, brush, composite, lens, preview);
	}

	/** A mask texture the painter can draw into and develop can read. */
	mask(size: [number, number], label: string): GPUTexture {
		return this.device.createTexture({ label, size, format: MASK, usage: GPUTextureUsage.RENDER_ATTACHMENT | GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_SRC | GPUTextureUsage.COPY_DST });
	}

	private scratch(target: GPUTexture) {
		if (this.coverage?.width !== target.width || this.coverage?.height !== target.height) {
			this.coverage?.destroy();
			this.before?.destroy();
			this.coverage = this.mask([target.width, target.height], 'stroke coverage');
			this.before = this.mask([target.width, target.height], 'mask before stroke');
		}
	}

	private pass(encoder: GPUCommandEncoder, target: GPUTexture, clear: boolean, box?: [number, number, number, number]) {
		const pass = encoder.beginRenderPass({ colorAttachments: [{ view: target.createView(), loadOp: clear ? 'clear' : 'load', storeOp: 'store', clearValue: [0, 0, 0, 0] }] });
		if (box) {
			const x = Math.max(0, Math.floor(box[0]));
			const y = Math.max(0, Math.floor(box[1]));
			const w = Math.min(target.width, Math.ceil(box[2])) - x;
			const h = Math.min(target.height, Math.ceil(box[3])) - y;
			if (w <= 0 || h <= 0) {
				pass.end();
				return null;
			}
			pass.setScissorRect(x, y, w, h);
		}
		return pass;
	}

	/** One stroke's segments into the coverage (blended with max). */
	private drawSegments(encoder: GPUCommandEncoder, stroke: StrokePx, segments: number[][], clear: boolean) {
		const coverage = this.coverage!;
		const params = new Float32Array([coverage.width, coverage.height, stroke.radius, stroke.feather, stroke.flow, 0, 0, 0]);
		this.device.queue.writeBuffer(this.brushUbo, 0, params);
		const data = new Float32Array(segments.flat());
		if (!this.segments || this.segments.size < data.byteLength) {
			this.segments?.destroy();
			this.segments = this.device.createBuffer({ size: Math.max(4096, data.byteLength * 2), usage: GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST });
		}
		const buffer = this.segments;
		this.device.queue.writeBuffer(buffer, 0, data);
		const pass = this.pass(encoder, coverage, clear)!;
		pass.setPipeline(this.brush);
		pass.setBindGroup(0, this.device.createBindGroup({ layout: this.brush.getBindGroupLayout(0), entries: [{ binding: 0, resource: { buffer: this.brushUbo } }] }));
		pass.setVertexBuffer(0, buffer);
		pass.draw(6, segments.length);
		pass.end();
	}

	/** `base` and `coverage` composited into `target` over `box` (all of it when absent); copying, `base` goes
	 *  through `range` (`rangeUniform`). */
	private compose(encoder: GPUCommandEncoder, target: GPUTexture, base: GPUTexture, coverage: GPUTexture, mode: number, box?: [number, number, number, number], range: number[] = [0, 0, 0, 0]) {
		const params = new ArrayBuffer(32);
		new Float32Array(params, 0, 2).set([target.width, target.height]);
		new Uint32Array(params, 8, 1)[0] = mode;
		new Float32Array(params, 16, 4).set(range);
		// One composite per submission at most (see `paint` and `extend`), so the uniforms can be shared.
		const ubo = this.compositeUbo;
		this.device.queue.writeBuffer(ubo, 0, params);
		const pass = this.pass(encoder, target, false, box);
		if (!pass) return;
		pass.setPipeline(this.composite);
		pass.setBindGroup(
			0,
			this.device.createBindGroup({
				layout: this.composite.getBindGroupLayout(0),
				entries: [
					{ binding: 0, resource: { buffer: ubo } },
					{ binding: 1, resource: base.createView() },
					{ binding: 2, resource: coverage.createView() },
					{ binding: 3, resource: this.sampler }
				]
			})
		);
		pass.draw(3);
		pass.end();
	}

	/** Paint `target` from scratch: `base` (a model's mask, any size; none starts empty) through `range`
	 *  (`rangeUniform`), then every stroke. */
	paint(target: GPUTexture, base: GPUTexture | undefined, strokes: Stroke[], range?: number[]) {
		this.live = undefined;
		this.scratch(target);
		const size: [number, number] = [target.width, target.height];
		let encoder = this.device.createCommandEncoder();
		this.compose(encoder, target, base ?? this.empty, this.empty, COPY, undefined, range);
		this.device.queue.submit([encoder.finish()]);
		for (const s of strokes) {
			const px = strokePx(s, size);
			if (!px.segments.length) continue;
			// One submission per stroke: the brush uniforms are written through the queue.
			encoder = this.device.createCommandEncoder();
			encoder.copyTextureToTexture({ texture: target }, { texture: this.before! }, size);
			this.drawSegments(encoder, px, px.segments, true);
			this.compose(encoder, target, this.before!, this.coverage!, px.erase ? ERASE : PAINT);
			this.device.queue.submit([encoder.finish()]);
		}
	}

	/** Start a stroke drawn as it is made, onto `target` as it is now. */
	begin(target: GPUTexture, stroke: Stroke) {
		this.scratch(target);
		const encoder = this.device.createCommandEncoder();
		encoder.copyTextureToTexture({ texture: target }, { texture: this.before! }, [target.width, target.height]);
		this.pass(encoder, this.coverage!, true)!.end();
		this.device.queue.submit([encoder.finish()]);
		this.live = { target, stroke: { ...strokePx({ ...stroke, points: [] }, [target.width, target.height]) }, box: [Infinity, Infinity, -Infinity, -Infinity] };
	}

	/** Add points (fractions of the photo) to the live stroke: from `from`, or a dab at the first point alone. */
	extend(points: [number, number][], from?: [number, number]) {
		const live = this.live;
		if (!live || !points.length) return;
		const { width: w, height: h } = live.target;
		const pts = [...(from ? [from] : []), ...points].map(([x, y]) => [x * w, y * h]);
		const segments = pts.length === 1 ? [[...pts[0], ...pts[0]]] : pts.slice(1).map((p, i) => [...pts[i], ...p]);
		const r = live.stroke.radius + 2;
		for (const [x, y] of pts) live.box = [Math.min(live.box[0], x - r), Math.min(live.box[1], y - r), Math.max(live.box[2], x + r), Math.max(live.box[3], y + r)];
		const encoder = this.device.createCommandEncoder();
		this.drawSegments(encoder, live.stroke, segments, false);
		this.compose(encoder, live.target, this.before!, this.coverage!, live.stroke.erase ? ERASE : PAINT, live.box);
		this.device.queue.submit([encoder.finish()]);
	}

	/** The live stroke's extent so far, target pixels `[x0, y0, x1, y1]`; `null` without one. */
	get liveBox(): [number, number, number, number] | null {
		return this.live && this.live.box[0] < this.live.box[2] ? this.live.box : null;
	}

	/** Undo the live stroke: put the target back as it was when it began. */
	cancel() {
		if (!this.live) return;
		const encoder = this.device.createCommandEncoder();
		encoder.copyTextureToTexture({ texture: this.before! }, { texture: this.live.target }, [this.live.target.width, this.live.target.height]);
		this.device.queue.submit([encoder.finish()]);
		this.live = undefined;
	}

	end() {
		this.live = undefined;
	}

	/**
	 * `src` through the lens correction into `dst` (same size), `uniform` packed for that size without
	 * vignetting — all of it, or only `box` (pixels of `dst`), leaving the rest as it was.
	 */
	correct(src: GPUTexture, dst: GPUTexture, uniform: Uint8Array, box?: [number, number, number, number]) {
		this.device.queue.writeBuffer(this.lensUbo, 0, uniform);
		const encoder = this.device.createCommandEncoder();
		const pass = this.pass(encoder, dst, !box, box);
		if (!pass) return;
		pass.setPipeline(this.lens);
		pass.setBindGroup(
			0,
			this.device.createBindGroup({
				layout: this.lens.getBindGroupLayout(0),
				entries: [
					{ binding: 0, resource: { buffer: this.lensUbo } },
					{ binding: 1, resource: src.createView() },
					{ binding: 2, resource: this.sampler }
				]
			})
		);
		pass.draw(3);
		pass.end();
		this.device.queue.submit([encoder.finish()]);
	}

	/**
	 * The preview while a stroke is painted: `without` and `withLayer` (the photo developed without the layer
	 * and with it at full weight) blended into `target` by `mask`, over `box` (pixels; all of it when absent).
	 * All four the developed picture's size.
	 */
	blend(without: GPUTexture, withLayer: GPUTexture, mask: GPUTexture, invert: boolean, target: GPUTexture, box?: [number, number, number, number]) {
		this.device.queue.writeBuffer(this.previewUbo, 0, new Uint32Array([invert ? 1 : 0, 0, 0, 0]));
		const encoder = this.device.createCommandEncoder();
		const pass = this.pass(encoder, target, false, box);
		if (!pass) return;
		pass.setPipeline(this.preview);
		pass.setBindGroup(
			0,
			this.device.createBindGroup({
				layout: this.preview.getBindGroupLayout(0),
				entries: [
					{ binding: 0, resource: { buffer: this.previewUbo } },
					{ binding: 1, resource: without.createView() },
					{ binding: 2, resource: withLayer.createView() },
					{ binding: 3, resource: mask.createView() }
				]
			})
		);
		pass.draw(3);
		pass.end();
		this.device.queue.submit([encoder.finish()]);
	}
}

/** Ramer–Douglas–Peucker: the fewest points within `tolerance` of the path (same units as the points). */
export function simplify(points: [number, number][], tolerance: number): [number, number][] {
	if (points.length < 3) return points;
	const keep = new Uint8Array(points.length);
	keep[0] = keep[points.length - 1] = 1;
	const stack: [number, number][] = [[0, points.length - 1]];
	while (stack.length) {
		const [a, b] = stack.pop()!;
		const [ax, ay] = points[a];
		const [bx, by] = points[b];
		const len = Math.hypot(bx - ax, by - ay);
		let worst = -1;
		let at = -1;
		for (let i = a + 1; i < b; i++) {
			const [px, py] = points[i];
			const d = len < 1e-12 ? Math.hypot(px - ax, py - ay) : Math.abs((bx - ax) * (ay - py) - (ax - px) * (by - ay)) / len;
			if (d > worst) [worst, at] = [d, i];
		}
		if (worst > tolerance) {
			keep[at] = 1;
			stack.push([a, at], [at, b]);
		}
	}
	return points.filter((_, i) => keep[i]);
}
