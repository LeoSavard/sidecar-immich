// SPDX-License-Identifier: Apache-2.0
/**
 * Soft proofing (DESIGN.md §15, "Soft proofing"): the output profiles stored on this server, uploading another, and
 * a proof's table for the options chosen — the display, the intent, black point compensation, paper and ink. The
 * server composes each with LittleCMS; a table for given options never changes, so it is fetched once and kept.
 */
import type { DisplaySpace } from '$lib/types';
import { parse, type LutTable } from '$lib/luts';

/** Mirrors `sidecar_api::proof::ProofInfo`. */
export type ProofInfo = { id: string; name: string; description?: string | null };

/** Mirrors `sidecar_api::proof::ProofIntent`. */
export type ProofIntent = 'perceptual' | 'relative' | 'saturation';

export type ProofOptions = { id: string; intent: ProofIntent; bpc: boolean; paper: boolean; warn: boolean };

const tables = new Map<string, LutTable | 'loading' | 'missing'>();

export async function list(): Promise<ProofInfo[]> {
	const r = await fetch('/api/proofs');
	return r.ok ? r.json() : [];
}

export async function upload(file: File): Promise<ProofInfo> {
	const r = await fetch(`/api/proofs?name=${encodeURIComponent(file.name)}`, { method: 'POST', body: file });
	if (!r.ok) throw new Error((await r.text()) || `${r.status}`);
	return r.json();
}

const query = (o: ProofOptions, display: DisplaySpace) => `display=${display}&intent=${o.intent}&bpc=${o.bpc}&paper=${o.paper}`;

export function cachedTable(o: ProofOptions, display: DisplaySpace): LutTable | undefined {
	const t = tables.get(`${o.id}?${query(o, display)}`);
	return typeof t === 'object' ? t : undefined;
}

/** Fetch the proof's table unless it is in hand or on its way, and call `ready` once it lands, or fails. */
export function loadTable(o: ProofOptions, display: DisplaySpace, ready: () => void) {
	const key = `${o.id}?${query(o, display)}`;
	if (tables.has(key)) return;
	tables.set(key, 'loading');
	fetch(`/api/proofs/${o.id}/table?${query(o, display)}`)
		.then(async (r) => {
			if (!r.ok) throw new Error(`${r.status}`);
			const parsed = parse(await r.arrayBuffer());
			if (!parsed) throw new Error('not a table');
			tables.set(key, parsed);
		})
		.catch(() => tables.set(key, 'missing'))
		.finally(ready);
}
