// SPDX-License-Identifier: Apache-2.0
// The Rust colour maths (crates/color) running as WebAssembly. Same code the worker uses for export.
import wasmUrl from './sidecar_color.wasm?url';
import type { CameraColor, DisplaySpace, EditParams, SourceKind } from '$lib/types';

type Exports = {
	memory: WebAssembly.Memory;
	sc_alloc(len: number): number;
	sc_free(ptr: number, len: number): void;
	sc_pack(ptr: number, len: number): number;
	sc_default_edit(): number;
	sc_last_error(): number;
};

export type Packed = { uniform: Uint8Array; lut: Float32Array; temperatureK: number; tint: number };

export class ColorPacker {
	private constructor(private x: Exports) {}

	static async load(): Promise<ColorPacker> {
		const { instance } = await WebAssembly.instantiateStreaming(fetch(wasmUrl), {});
		return new ColorPacker(instance.exports as unknown as Exports);
	}

	private framed(ptr: number): Uint8Array {
		const view = new DataView(this.x.memory.buffer);
		const len = view.getUint32(ptr, true);
		return new Uint8Array(this.x.memory.buffer, ptr + 4, len).slice();
	}

	defaultEdit(): EditParams {
		return JSON.parse(new TextDecoder().decode(this.framed(this.x.sc_default_edit())));
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
		const view = new DataView(out.buffer, out.byteOffset);
		return {
			uniform: out.slice(0, 272),
			lut: new Float32Array(out.slice(272, 272 + lutBytes).buffer),
			temperatureK: view.getFloat64(272 + lutBytes, true),
			tint: view.getFloat64(280 + lutBytes, true)
		};
	}
}
