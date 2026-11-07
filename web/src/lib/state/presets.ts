// SPDX-License-Identifier: Apache-2.0
/**
 * Presets (DESIGN.md §15, "Presets, copy and paste, sync"): named sets of sections and their values, kept per
 * person on the server (`sidecar_api::presets`), applied like a pasted look, and passed around as files.
 */
import { api } from '$lib/api';
import type { EditParams } from '$lib/types';
import { applySections, SECTIONS } from '$lib/state/look-clipboard';

/** Mirrors `sidecar_api::presets::Preset`. */
export type Preset = { id: string; name: string; sections: string[]; edit: EditParams; created: number };

/** A preset as a file: what Export writes and Import reads. */
const FORMAT = 'sidecar-preset';
const VERSION = 1;

export async function list(): Promise<Preset[]> {
	return (await api('/api/presets')).json();
}

/**
 * Keep `sections` of `edit` as a preset. Only those sections' values are stored — taken onto `defaults`, the
 * edit a fresh photo has — so a preset carries nothing of the photo it was made from beyond what it applies.
 */
export async function create(name: string, sections: string[], edit: EditParams, defaults: EditParams): Promise<Preset> {
	const kept = structuredClone(defaults);
	applySections(kept, edit, sections);
	const r = await api('/api/presets', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ name, sections, edit: kept }) });
	return r.json();
}

export async function remove(id: string): Promise<void> {
	await api(`/api/presets/${id}`, { method: 'DELETE' });
}

/** `preset` applied to `edit`, in place. */
export function apply(edit: EditParams, preset: Preset): void {
	applySections(edit, preset.edit, preset.sections);
}

/** Download `preset` as a `.sidecar-preset.json` file. */
export function exportFile(preset: Preset): void {
	const body = JSON.stringify({ format: FORMAT, version: VERSION, name: preset.name, sections: preset.sections, edit: preset.edit }, null, '\t');
	const a = document.createElement('a');
	a.href = URL.createObjectURL(new Blob([body], { type: 'application/json' }));
	a.download = `${preset.name.replace(/[^\w .-]+/g, '_')}.sidecar-preset.json`;
	a.click();
	setTimeout(() => URL.revokeObjectURL(a.href), 1000);
}

/** Read a preset file and keep it as one of this person's presets. */
export async function importFile(file: File, defaults: EditParams): Promise<Preset> {
	let parsed: { format?: string; version?: number; name?: string; sections?: unknown; edit?: EditParams };
	try {
		parsed = JSON.parse(await file.text());
	} catch {
		throw new Error(`${file.name} isn't a preset file.`);
	}
	if (parsed.format !== FORMAT || typeof parsed.version !== 'number' || !parsed.edit || !Array.isArray(parsed.sections)) throw new Error(`${file.name} isn't a Sidecar preset.`);
	if (parsed.version > VERSION) throw new Error(`${file.name} was written by a newer Sidecar.`);
	const known = new Set(SECTIONS.map((s) => s.id));
	const sections = (parsed.sections as unknown[]).filter((s): s is string => typeof s === 'string' && known.has(s));
	if (!sections.length) throw new Error(`${file.name} carries no section this Sidecar knows.`);
	// Through `create`, so the stored edit is the file's sections over this server's defaults.
	return create(parsed.name || file.name.replace(/\.sidecar-preset\.json$|\.json$/, ''), sections, { ...structuredClone(defaults), ...parsed.edit }, defaults);
}
