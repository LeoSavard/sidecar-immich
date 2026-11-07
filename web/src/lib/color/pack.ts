// SPDX-License-Identifier: Apache-2.0
// The Rust colour maths (crates/color) running as WebAssembly. Same code the worker uses for export.
import wasmUrl from './sidecar_color.wasm?url';
import type { ColorRange } from '$lib/generated/edit/ColorRange';
import type { CameraColor, DisplaySpace, EditParams, Frame, Geometry, Gradient, Lens, LensProfile, MaskEdge, SourceKind } from '$lib/types';
import type { LookLut } from '$lib/generated/edit/LookLut';
import type { LutTable } from '$lib/luts';

/** Mirrors `sidecar_color::params::EdgePlan`: the mask edge passes for one mask size. */
export type EdgePlan = { scale: number; small: [number, number]; radius: number; threshold: number; ramp: number };

type Exports = {
	memory: WebAssembly.Memory;
	sc_alloc(len: number): number;
	sc_free(ptr: number, len: number): void;
	sc_pack(ptr: number, len: number): number;
	sc_pack_lens(ptr: number, len: number): number;
	sc_tone_curve_lut(ptr: number, len: number): number;
	sc_geometry(ptr: number, len: number): number;
	sc_adopt_rendering(ptr: number, len: number): number;
	sc_geometry_fit(ptr: number, len: number): number;
	sc_upright(ptr: number, len: number): number;
	sc_default_edit(): number;
	sc_last_error(): number;
	sc_develop_uniform_bytes(): number;
	sc_white_from_neutral(ptr: number, len: number): number;
	sc_source_color(ptr: number, len: number): number;
	sc_camera_color(ptr: number, len: number): number;
	sc_pack_depth(ptr: number, len: number): number;
	sc_pack_look(ptr: number, len: number): number;
	sc_pack_proof(ptr: number, len: number): number;
	sc_mask_edge(ptr: number, len: number): number;
	sc_gradient(ptr: number, len: number): number;
	sc_color_select(req: number, reqLen: number, uniform: number, uniformLen: number, pixels: number, pixelsLen: number, mask: number, maskLen: number): number;
};

/**
 * `masks` are the develop pass's mask uniforms (`DevelopPass.setMasks`), texture `i` being `edit.masks[i]`'s;
 * `maskCurves` the layers' curves, a 4,096-entry row each.
 */
export type Packed = {
	uniform: Uint8Array;
	lut: Float32Array;
	temperatureK: number;
	tint: number;
	masks: Uint8Array;
	maskCurves: Float32Array;
	monoLut: Float32Array;
	/** A spectral colour stock's layer responses over the colour cube; empty unless one is being rendered. */
	slideExposure: Float32Array;
	/** The camera profile pass (`DevelopPass.setProfile`), when the edit renders with the file's profile. */
	profile: ProfilePass | null;
	/** The defringe pass's uniform (`DevelopPass.setDefringe`), when the edit takes fringes out. */
	defringe: Uint8Array | null;
};

/** `sidecar_color::params::ProfilePass`: the pass's uniform and its HueSatMap and LookTable, four floats an entry. */
export type ProfilePass = { uniform: Uint8Array; hueSat: Float32Array; look: Float32Array };

/** A spectral colour stock's profile, as `GET /api/film/stocks/{id}` sends it (`slide::SlideProfile`). */
export type SlideProfile = { offsets: [number, number, number]; scale: number; side: number; response: Float32Array };

/** `DevelopParams` (crates/color/src/params.rs). Checked against the wasm on load, so a
 *  struct that grew on one side fails here, by name, rather than as a garbled render. */
export const DEVELOP_UNIFORM_BYTES = 544;
const MASK_UNIFORM_BYTES = 1568;
const EMPTY = new Float32Array(0);
const LUT = 4096;
const MAX_MASKS = 8;

export class ColorPacker {
	private constructor(private x: Exports) {}

	static async load(): Promise<ColorPacker> {
		const { instance } = await WebAssembly.instantiateStreaming(fetch(wasmUrl), {});
		const x = instance.exports as unknown as Exports;
		const bytes = x.sc_develop_uniform_bytes();
		if (bytes !== DEVELOP_UNIFORM_BYTES) throw new Error(`The develop uniform is ${bytes} bytes in the colour maths but ${DEVELOP_UNIFORM_BYTES} here: rebuild the wasm (npm run generate).`);
		return new ColorPacker(x);
	}

	private framed(ptr: number): Uint8Array {
		const view = new DataView(this.x.memory.buffer);
		const len = view.getUint32(ptr, true);
		return new Uint8Array(this.x.memory.buffer, ptr + 4, len).slice();
	}

	defaultEdit(): EditParams {
		return JSON.parse(new TextDecoder().decode(this.framed(this.x.sc_default_edit())));
	}

	/**
	 * Lens-pass uniforms for the proxy (`frame`, already oriented by `flip`), or `undefined` when the strengths
	 * leave nothing to correct. `sensorAspect` is width ÷ height before orientation.
	 */
	packLens(profile: LensProfile, lens: Lens, sensorAspect: number, frame: [number, number], flip: number): Uint8Array | undefined {
		const input = new TextEncoder().encode(JSON.stringify({ profile, lens, sensorAspect, frame, flip }));
		const ptr = this.x.sc_alloc(input.length);
		new Uint8Array(this.x.memory.buffer, ptr, input.length).set(input);
		const out = this.framed(this.x.sc_pack_lens(ptr, input.length));
		this.x.sc_free(ptr, input.length);
		return out.length ? out : undefined;
	}

	private call(fn: (ptr: number, len: number) => number, request: unknown): Uint8Array {
		const input = new TextEncoder().encode(JSON.stringify(request));
		const ptr = this.x.sc_alloc(input.length);
		new Uint8Array(this.x.memory.buffer, ptr, input.length).set(input);
		const out = this.framed(fn(ptr, input.length));
		this.x.sc_free(ptr, input.length);
		if (out.length === 0) throw new Error(new TextDecoder().decode(this.framed(this.x.sc_last_error())));
		return out;
	}

	/**
	 * The edit as today's rendering would make it, and whether that differs from how it renders now
	 * (`sidecar_color::params::EditParams::adopt_current_rendering`). An edit made before a reference
	 * existed keeps its own picture for ever otherwise; this is the deliberate way out of that, and the
	 * picture changes when it is taken.
	 */
	adoptRendering(edit: EditParams, developed: boolean): { edit: EditParams; changed: boolean } {
		return JSON.parse(new TextDecoder().decode(this.call(this.x.sc_adopt_rendering, { edit, developed })));
	}

	/** The geometry's map and outline over a picture of `size` pixels (`sidecar_color::geometry::frame`). */
	frame(geometry: Geometry, size: [number, number]): Frame {
		return JSON.parse(new TextDecoder().decode(this.call(this.x.sc_geometry, { geometry, size })));
	}

	/** `geometry` with its crop fitted after a change from `before` (`sidecar_color::geometry::fit`). */
	fit(geometry: Geometry, before: Geometry, size: [number, number]): Geometry {
		return JSON.parse(new TextDecoder().decode(this.call(this.x.sc_geometry_fit, { geometry, before, size })));
	}

	/**
	 * `geometry` stood up by the photo's own lines (`sidecar_color::upright::solve`): `lum` is the picture's
	 * luminance at `lumSize`, `focal` the lens in picture diagonals when known. Null when the photo hasn't the
	 * straight lines `mode` needs.
	 */
	upright(geometry: Geometry, size: [number, number], lum: Float32Array, lumSize: [number, number], mode: 'level' | 'vertical' | 'full', focal: number | null): Geometry | null {
		try {
			return JSON.parse(new TextDecoder().decode(this.call(this.x.sc_upright, { geometry, size, lum: Array.from(lum, (v) => Math.round(v * 1e4) / 1e4), lumSize, mode, focal })));
		} catch {
			return null;
		}
	}

	/**
	 * The tone curve's own LUT (`sidecar_color::spline::tone_curve_lut`) — the same one `pack()` bakes into
	 * its result, callable on its own so the curve editor can draw the "ghost" line Highlights/Shadows alone
	 * would produce (`curve: []`) without a full `pack()` call, which needs camera colour this doesn't have.
	 */
	toneCurveLut(curve: [number, number][], highlights: number, shadows: number): Float32Array {
		return new Float32Array(this.call(this.x.sc_tone_curve_lut, { curve, highlights, shadows }).buffer);
	}

	/** A gradient layer's uniform for a mask `size` pixels big (`Gradient::uniform`, 48 bytes). */
	gradientUniform(gradient: Gradient, size: [number, number]): Uint8Array {
		return this.call(this.x.sc_gradient, { gradient, size });
	}

	/** A mask edge's passes for a mask `size` pixels big (`MaskEdge::plan`); null when it is too narrow to show. */
	edgePlan(edge: MaskEdge, size: [number, number]): EdgePlan | null {
		return JSON.parse(new TextDecoder().decode(this.call(this.x.sc_mask_edge, { edge, size })));
	}

	/** The depth passes' uniform (64 bytes) for the proxy, or `undefined` when the edit neither blurs nor hazes. */
	packDepth(edit: EditParams, airlight: [number, number, number], frame: [number, number]): Uint8Array | undefined {
		const input = new TextEncoder().encode(JSON.stringify({ edit, airlight, frame }));
		const ptr = this.x.sc_alloc(input.length);
		new Uint8Array(this.x.memory.buffer, ptr, input.length).set(input);
		const out = this.framed(this.x.sc_pack_depth(ptr, input.length));
		this.x.sc_free(ptr, input.length);
		return out.length ? out.slice() : undefined;
	}

	/** The LUT pass's uniform (128 bytes) for develop's output in `display`, or `undefined` when the amount leaves nothing to do. */
	packLook(lut: LookLut, display: DisplaySpace, header: LutTable['header']): Uint8Array | undefined {
		const input = new TextEncoder().encode(JSON.stringify({ lut, display, header }));
		const ptr = this.x.sc_alloc(input.length);
		new Uint8Array(this.x.memory.buffer, ptr, input.length).set(input);
		const out = this.framed(this.x.sc_pack_look(ptr, input.length));
		this.x.sc_free(ptr, input.length);
		return out.length ? out.slice() : undefined;
	}

	/** A soft proof's uniform for the LUT pass (`lut::proof_uniform`), with the gamut warning painted when `warn`. */
	packProof(display: DisplaySpace, header: LutTable['header'], warn: boolean): Uint8Array {
		return this.call(this.x.sc_pack_proof, { display, header, warn }).slice();
	}

	/**
	 * A colour-range layer's selection (`color_range::select`), one byte a pixel at the proxy's size: `proxy` is its
	 * camera RGB, `uniform` the develop uniform the edit packs to, `within` the model mask it is limited to.
	 */
	colorSelect(range: ColorRange, uniform: Uint8Array, proxy: { width: number; height: number; rgba: Float32Array }, within: { width: number; height: number; bytes: Uint8Array } | null): Uint8Array {
		const req = new TextEncoder().encode(JSON.stringify({ range, width: proxy.width, height: proxy.height, maskWidth: within?.width ?? 0, maskHeight: within?.height ?? 0 }));
		const px = new Uint8Array(proxy.rgba.buffer, proxy.rgba.byteOffset, proxy.rgba.byteLength);
		const parts = [req, uniform, px, within?.bytes ?? new Uint8Array(0)];
		const ptrs = parts.map((p) => {
			const ptr = this.x.sc_alloc(Math.max(p.length, 1));
			new Uint8Array(this.x.memory.buffer, ptr, p.length).set(p);
			return ptr;
		});
		const out = this.framed(this.x.sc_color_select(ptrs[0], req.length, ptrs[1], uniform.length, ptrs[2], px.length, ptrs[3], parts[3].length));
		parts.forEach((p, k) => this.x.sc_free(ptrs[k], Math.max(p.length, 1)));
		if (out.length === 0) throw new Error(new TextDecoder().decode(this.framed(this.x.sc_last_error())));
		return out;
	}

	/** The temperature and tint under which `neutral`, a camera RGB, is grey: the white balance eyedropper. */
	whiteFromNeutral(camera: CameraColor, neutral: [number, number, number]): { temperatureK: number; tint: number } {
		return JSON.parse(new TextDecoder().decode(this.call(this.x.sc_white_from_neutral, { camera, neutral })));
	}

	/** The colour model of a developed photo in `space`, for one the browser decoded itself
	 *  (`sidecar_color::camera::SourceSpace::camera_color`, the same one the worker gives a JPEG). */
	sourceColor(space: 'srgb' | 'display-p3' | 'adobe-rgb'): CameraColor {
		return JSON.parse(new TextDecoder().decode(this.call(this.x.sc_source_color, space)));
	}

	/** The colour model of a RAW the browser decoded, from the colour data LibRaw read (`LibrawColor`): the same
	 *  mapping the server applies to the same file (`sidecar_color::camera::LibrawColor::camera_color`). */
	cameraColor(libraw: unknown): CameraColor {
		return JSON.parse(new TextDecoder().decode(this.call(this.x.sc_camera_color, libraw)));
	}

	/** `source` picks the develop pass's tone path: sensor light, or a picture a camera or app already rendered. */
	pack(edit: EditParams, camera: CameraColor, source: SourceKind, display: DisplaySpace, slide?: SlideProfile): Packed {
		// The profile's header is all `pack` reads; its response table goes straight to the GPU.
		const slideProfile = slide ? { offsets: slide.offsets, scale: slide.scale, side: slide.side } : null;
		const input = new TextEncoder().encode(JSON.stringify({ edit, camera, source, display, slideProfile }));
		const ptr = this.x.sc_alloc(input.length);
		new Uint8Array(this.x.memory.buffer, ptr, input.length).set(input);
		const out = this.framed(this.x.sc_pack(ptr, input.length));
		this.x.sc_free(ptr, input.length);
		if (out.length === 0) throw new Error(new TextDecoder().decode(this.framed(this.x.sc_last_error())));
		const lutBytes = 4096 * 4;
		const U = DEVELOP_UNIFORM_BYTES;
		const view = new DataView(out.buffer, out.byteOffset);
		// Two length-prefixed tables over the colour cube close the stream: black and white's exposures, then a
		// spectral stock's layer responses. At most one is ever non-empty.
		let at = U + 16 + lutBytes + MASK_UNIFORM_BYTES + MAX_MASKS * LUT * 4;
		const table = () => {
			const floats = view.getUint32(at, true);
			const start = at + 4;
			at = start + floats * 4;
			return floats ? new Float32Array(out.slice(start, at).buffer) : EMPTY;
		};
		const monoLut = table();
		const slideExposure = table();
		const profileBytes = view.getUint32(at, true);
		let profile: ProfilePass | null = null;
		if (profileBytes) {
			const uniform = out.slice(at + 4, at + 4 + profileBytes);
			at += 4 + profileBytes;
			profile = { uniform, hueSat: table(), look: table() };
		}
		const defringeBytes = view.getUint32(at, true);
		const defringe = defringeBytes ? out.slice(at + 4, at + 4 + defringeBytes) : null;
		return {
			uniform: out.slice(0, U),
			lut: new Float32Array(out.slice(U, U + lutBytes).buffer),
			temperatureK: view.getFloat64(U + lutBytes, true),
			tint: view.getFloat64(U + 8 + lutBytes, true),
			masks: out.slice(U + 16 + lutBytes, U + 16 + lutBytes + MASK_UNIFORM_BYTES),
			maskCurves: new Float32Array(out.slice(U + 16 + lutBytes + MASK_UNIFORM_BYTES, U + 16 + lutBytes + MASK_UNIFORM_BYTES + MAX_MASKS * LUT * 4).buffer),
			monoLut,
			slideExposure,
			profile,
			defringe
		};
	}
}
