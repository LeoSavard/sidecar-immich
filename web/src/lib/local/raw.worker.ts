// SPDX-License-Identifier: Apache-2.0
// LibRaw in a Web Worker, so unpacking a RAW (half a second to a few seconds on a phone) never stalls the page.
// The module is mobile/libraw's build of LibRaw and the server's shim; see ./raw.ts for what comes back.
//
// It is loaded at run time from the mobile module, which serves it where an image with a -mobile target is
// deployed (docs/MOBILE.md), rather than bundled: the main app then builds without Emscripten, and a
// deployment without the module simply has no RAW decoding on the device (`rawOnDevice` in ./raw.ts).
/// <reference lib="webworker" />
import type createLibRaw from './sidecar_libraw';
import type { RawInfo, RawResult } from './raw';
import { LIBRAW_URL } from './raw-url';

let lib: ReturnType<typeof createLibRaw> | undefined;

async function load(): ReturnType<typeof createLibRaw> {
	const module: { default: typeof createLibRaw } = await import(/* @vite-ignore */ LIBRAW_URL);
	return module.default();
}

self.onmessage = async (e: MessageEvent<ArrayBuffer>) => {
	let m: Awaited<ReturnType<typeof createLibRaw>>;
	try {
		m = await (lib ??= load());
	} catch (err) {
		lib = undefined;
		postMessage({ error: `the RAW decoder isn't available here (${err instanceof Error ? err.message : err})` } satisfies RawResult);
		return;
	}
	const bytes = new Uint8Array(e.data);
	const at = m._malloc(bytes.length);
	try {
		m.HEAPU8.set(bytes, at);
		const code = m._scw_open(at, bytes.length);
		if (code !== 0) {
			postMessage({ error: m.UTF8ToString(m._scw_strerror(code)) } satisfies RawResult);
			return;
		}
		const info: RawInfo = JSON.parse(m.UTF8ToString(m._scw_info()));
		const described = m.UTF8ToString(m._scw_mosaic());
		if (!described) {
			postMessage({ info, mosaic: null } satisfies RawResult);
			return;
		}
		const mosaic = JSON.parse(described);
		// Copied out of the module's memory, which the next decode reuses, and handed over without a second copy.
		const data = new Uint16Array(m.HEAPU16.buffer, m._scw_mosaic_data(), mosaic.width * mosaic.height).slice();
		postMessage({ info, mosaic: { ...mosaic, data, flip: info.flip } } satisfies RawResult, [data.buffer]);
	} catch (err) {
		postMessage({ error: err instanceof Error ? err.message : String(err) } satisfies RawResult);
	} finally {
		m._scw_close();
		m._free(at);
	}
};
