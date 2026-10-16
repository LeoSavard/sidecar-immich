/**
 * Spectral colour stocks (DESIGN.md §7, "Colour film, spectrally"): which ones this server can render, and
 * each one's profile.
 *
 * A stock's chain is mostly public — the data sheet's spectral sensitivities, characteristic curves and dyes,
 * all in `sidecar_color::slide`, which the packer runs here in wasm. What is not public is the two ISO
 * standards it is measured through, which Sidecar does not carry and the browser never sees: the server
 * reduces them to a *profile* (the slide's colours over its layers' log H, and three offsets) and serves that.
 * So a deployment whose operator has no copies of the standards offers no stocks, and the editor shows none.
 *
 * A profile is about four megabytes and depends only on the stock, so it is fetched once and kept.
 */
import type { SlideProfile } from '$lib/color/pack';

/** `SCSL`, little-endian, and the format `sidecar_color::slide::SlideProfile::to_bytes` writes. */
const MAGIC = 0x4c534353;
const VERSION = 1;

const profiles = new Map<string, SlideProfile | 'loading' | 'missing'>();
let offered: Promise<string[]> | undefined;

/** The stocks this server renders, by id; empty when it has no standards to measure them through. */
export function stocks(): Promise<string[]> {
	offered ??= fetch('/api/film/stocks')
		.then((r) => (r.ok ? r.json() : { stocks: [] }))
		.then((j) => (Array.isArray(j?.stocks) ? (j.stocks as string[]) : []))
		.catch(() => []);
	return offered;
}

/** A profile already in hand, for the packer, which runs while a slider moves and cannot wait. */
export function cachedProfile(id: string): SlideProfile | undefined {
	const p = profiles.get(id);
	return typeof p === 'object' ? p : undefined;
}

/**
 * Fetch `id`'s profile unless it is in hand or on its way, and call `ready` once it lands — the editor
 * redraws then, with the stock rendering. A stock the server cannot measure is remembered as missing, so a
 * photo whose edit names one does not ask again on every frame.
 */
export function loadProfile(id: string, ready: () => void) {
	if (profiles.has(id)) return;
	profiles.set(id, 'loading');
	fetch(`/api/film/stocks/${id}`)
		.then(async (r) => {
			if (!r.ok) throw new Error(`${r.status}`);
			const parsed = parse(await r.arrayBuffer());
			if (!parsed) throw new Error('not a profile');
			profiles.set(id, parsed);
			ready();
		})
		.catch(() => profiles.set(id, 'missing'));
}

function parse(bytes: ArrayBuffer): SlideProfile | undefined {
	if (bytes.byteLength < 32 || bytes.byteLength % 4 !== 0) return undefined;
	const head = new DataView(bytes);
	if (head.getUint32(0, true) !== MAGIC || head.getUint32(4, true) !== VERSION) return undefined;
	const side = head.getUint32(8, true);
	const entries = side ** 3 * 4;
	if (side === 0 || bytes.byteLength !== 32 + entries * 4) return undefined;
	return {
		offsets: [head.getFloat32(16, true), head.getFloat32(20, true), head.getFloat32(24, true)],
		scale: head.getFloat32(28, true),
		side,
		response: new Float32Array(bytes, 32, entries)
	};
}
