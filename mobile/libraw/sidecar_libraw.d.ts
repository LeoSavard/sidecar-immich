// SPDX-License-Identifier: Apache-2.0
// Types for the Emscripten module build.sh writes beside this file's copy (mobile/src/local/libraw/).
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
