// SPDX-License-Identifier: Apache-2.0
// A RAW decoded on the phone (docs/MOBILE.md, stage 2): LibRaw, built for the browser, unpacks the sensor data in
// a worker; the server's own ingest shaders then develop it to a proxy on the phone's GPU (`$lib/gpu/ingest`).
import type { Mosaic } from '$lib/gpu/ingest';

/** What `scw_info` reports (mobile/libraw/wasm_api.cpp). `color` is `sidecar_color::camera::LibrawColor`. */
export type RawInfo = {
	width: number;
	height: number;
	flip: number;
	colors: number;
	filters: number;
	iso: number;
	shutter: number;
	aperture: number;
	focalLen: number;
	timestamp: number;
	make: string;
	model: string;
	lens: string;
	color: unknown;
};

/** A decode: the sensor's mosaic, or null for a RAW with none (sRAW, linear DNG, Foveon), or why it failed. */
export type RawResult = { info: RawInfo; mosaic: Mosaic | null; error?: undefined } | { error: string; info?: undefined; mosaic?: undefined };

let worker: Worker | undefined;

/** Unpack a RAW's bytes off the main thread. The bytes are copied to the worker; `bytes` stays usable here. */
export function decodeRaw(bytes: ArrayBuffer): Promise<RawResult> {
	worker ??= new Worker(new URL('./raw.worker.ts', import.meta.url), { type: 'module' });
	const w = worker;
	return new Promise((resolve) => {
		w.onmessage = (e: MessageEvent<RawResult>) => resolve(e.data);
		w.onerror = (e) => resolve({ error: e.message || 'the RAW decoder failed to start' });
		const copy = bytes.slice(0);
		w.postMessage(copy, [copy]);
	});
}
