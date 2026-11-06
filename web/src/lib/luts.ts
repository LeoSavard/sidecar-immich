// SPDX-License-Identifier: Apache-2.0
/**
 * 3D LUTs as looks (DESIGN.md §15, "3D LUTs"): the ones stored on this server, uploading another, and each one's
 * table for the develop pass. A table is stored by its file's hash and never changes, so it is fetched once
 * and kept.
 */

/** Mirrors `sidecar_api::luts::LutInfo`. */
export type LutInfo = { id: string; name: string; title?: string | null; size: number };

/** Mirrors `sidecar_color::lut::CubeHeader`, and the table as RGBA floats, red fastest. */
export type LutTable = { header: { size: number; domainMin: [number, number, number]; domainMax: [number, number, number] }; rgba: Float32Array };

const tables = new Map<string, LutTable | 'loading' | 'missing'>();

export async function list(): Promise<LutInfo[]> {
	const r = await fetch('/api/luts');
	return r.ok ? r.json() : [];
}

/** Store a `.cube` file on the server; the server's message on failure. */
export async function upload(file: File): Promise<LutInfo> {
	const r = await fetch(`/api/luts?name=${encodeURIComponent(file.name)}`, { method: 'POST', body: await file.text() });
	if (!r.ok) throw new Error((await r.text()) || `${r.status}`);
	return r.json();
}

export async function remove(id: string): Promise<void> {
	tables.delete(id);
	await fetch(`/api/luts/${id}`, { method: 'DELETE' });
}

/** A table already in hand, for the packer, which runs while a slider moves and cannot wait. */
export function cachedTable(id: string): LutTable | undefined {
	const t = tables.get(id);
	return typeof t === 'object' ? t : undefined;
}

/** Whether the server has no such LUT: an edit made where it was, opened where it isn't. */
export function isMissing(id: string): boolean {
	return tables.get(id) === 'missing';
}

/** Fetch `id`'s table unless it is in hand or on its way, and call `ready` once it lands, or once it is known missing. */
export function loadTable(id: string, ready: () => void) {
	if (tables.has(id)) return;
	tables.set(id, 'loading');
	fetch(`/api/luts/${id}`)
		.then(async (r) => {
			if (!r.ok) throw new Error(`${r.status}`);
			const parsed = parse(await r.arrayBuffer());
			if (!parsed) throw new Error('not a table');
			tables.set(id, parsed);
		})
		.catch(() => tables.set(id, 'missing'))
		.finally(ready);
}

/** `GET /api/luts/{id}`'s body, and a proof's (`GET /api/proofs/{id}/table`): the side (u32), the domain's six floats, then RGBA floats. */
export function parse(bytes: ArrayBuffer): LutTable | undefined {
	if (bytes.byteLength < 28) return undefined;
	const v = new DataView(bytes);
	const size = v.getUint32(0, true);
	if (size < 2 || bytes.byteLength !== 28 + size ** 3 * 16) return undefined;
	const f = (i: number) => v.getFloat32(4 + i * 4, true);
	return { header: { size, domainMin: [f(0), f(1), f(2)], domainMax: [f(3), f(4), f(5)] }, rgba: new Float32Array(bytes, 28) };
}
