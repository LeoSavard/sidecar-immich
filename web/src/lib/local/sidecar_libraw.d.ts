// SPDX-License-Identifier: Apache-2.0
// Types for the Emscripten module mobile/libraw/build.sh writes into the mobile module (mobile/static/libraw/), which
// serves it at /m/libraw/; raw.worker.ts loads it from there at run time.
export type LibRaw = {
	HEAPU8: Uint8Array;
	HEAPU16: Uint16Array;
	UTF8ToString(ptr: number): string;
	_malloc(size: number): number;
	_free(ptr: number): void;
	_scw_open(data: number, size: number): number;
	_scw_strerror(code: number): number;
	_scw_info(): number;
	_scw_mosaic(): number;
	_scw_mosaic_data(): number;
	_scw_close(): void;
};
export default function createLibRaw(): Promise<LibRaw>;
