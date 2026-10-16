// SPDX-License-Identifier: Apache-2.0
// The Rust colour maths (crates/color) running as WebAssembly. Same code the worker uses for export.
import wasmUrl from './sidecar_color.wasm?url';
import type { CameraColor, DisplaySpace, EditParams, Frame, Geometry, Lens, LensProfile, SourceKind } from '$lib/types';

type Exports = {
	memory: WebAssembly.Memory;
	sc_alloc(len: number): number;
	sc_free(ptr: number, len: number): void;
	sc_pack(ptr: number, len: number): number;
	sc_pack_lens(ptr: number, len: number): number;
	sc_tone_curve_lut(ptr: number, len: number): number;
	sc_geometry(ptr: number, len: number): number;
	sc_geometry_fit(ptr: number, len: number): number;
	sc_default_edit(): number;
	sc_last_error(): number;
	sc_develop_uniform_bytes(): number;
};

/**
 * `masks` are the develop pass's mask uniforms (`DevelopPass.setMasks`), texture `i` being `edit.masks[i]`'s;
 * `maskCurves` the layers' curves, a 4,096-entry row each.
 */
export type Packed = { uniform: Uint8Array; lut: Float32Array; temperatureK: number; tint: number; masks: Uint8Array; maskCurves: Float32Array };

/** `DevelopParams` (shaders/develop/params.wgsl), mirrored in crates/color. Checked against the wasm on load, so a
 *  struct that grew on one side fails here, by name, rather than as a garbled render. */
export const DEVELOP_UNIFORM_BYTES = 288;
const MASK_UNIFORM_BYTES = 1568;
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

	/** The geometry's map and outline over a picture of `size` pixels (`sidecar_color::geometry::frame`). */
	frame(geometry: Geometry, size: [number, number]): Frame {
		return JSON.parse(new TextDecoder().decode(this.call(this.x.sc_geometry, { geometry, size })));
	}

	/** `geometry` with its crop fitted after a change from `before` (`sidecar_color::geometry::fit`). */
	fit(geometry: Geometry, before: Geometry, size: [number, number]): Geometry {
		return JSON.parse(new TextDecoder().decode(this.call(this.x.sc_geometry_fit, { geometry, before, size })));
	}

	/**
	 * The tone curve's own LUT (`sidecar_color::spline::tone_curve_lut`) — the same one `pack()` bakes into
	 * its result, callable on its own so the curve editor can draw the "ghost" line Highlights/Shadows alone
	 * would produce (`curve: []`) without a full `pack()` call, which needs camera colour this doesn't have.
	 */
	toneCurveLut(curve: [number, number][], highlights: number, shadows: number): Float32Array {
		return new Float32Array(this.call(this.x.sc_tone_curve_lut, { curve, highlights, shadows }).buffer);
	}

	/** `source` picks the develop pass's tone path: sensor light, or a picture a camera or app already rendered. */
	pack(edit: EditParams, camera: CameraColor, source: SourceKind, display: DisplaySpace): Packed {
		const input = new TextEncoder().encode(JSON.stringify({ edit, camera, source, display }));
		const ptr = this.x.sc_alloc(input.length);
		new Uint8Array(this.x.memory.buffer, ptr, input.length).set(input);
		const out = this.framed(this.x.sc_pack(ptr, input.length));
		this.x.sc_free(ptr, input.length);
		if (out.length === 0) throw new Error(new TextDecoder().decode(this.framed(this.x.sc_last_error())));
		const lutBytes = 4096 * 4;
		const U = DEVELOP_UNIFORM_BYTES;
		const view = new DataView(out.buffer, out.byteOffset);
		return {
			uniform: out.slice(0, U),
			lut: new Float32Array(out.slice(U, U + lutBytes).buffer),
			temperatureK: view.getFloat64(U + lutBytes, true),
			tint: view.getFloat64(U + 8 + lutBytes, true),
			masks: out.slice(U + 16 + lutBytes, U + 16 + lutBytes + MASK_UNIFORM_BYTES),
			maskCurves: new Float32Array(out.slice(U + 16 + lutBytes + MASK_UNIFORM_BYTES, U + 16 + lutBytes + MASK_UNIFORM_BYTES + MAX_MASKS * LUT * 4).buffer)
		};
	}
}
