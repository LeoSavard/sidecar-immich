// SPDX-License-Identifier: Apache-2.0
/**
 * Sync across a selection (DESIGN.md §15, "Presets, copy and paste, sync"): one change — a pasted look, a preset —
 * applied to several photos' edits, each saved as its own version through the same API the editor saves with,
 * so every photo's history shows the change and can undo it.
 */
import { api, HttpError } from '$lib/api';
import type { EditParams } from '$lib/types';

export type BatchResult = { key: string; ok: boolean; reason?: string };

type EditState = { edit: EditParams; head: string; readOnly: boolean; writable: boolean };

/** Apply `change` to each photo's edit in turn; `progress` hears of each as it is done. */
export async function applyToPhotos(keys: string[], change: (edit: EditParams) => void, progress?: (done: BatchResult[]) => void): Promise<BatchResult[]> {
	const done: BatchResult[] = [];
	for (const key of keys) {
		let result: BatchResult;
		try {
			const state: EditState = await (await api(`/api/assets/${key}/edit`)).json();
			if (state.readOnly) result = { key, ok: false, reason: 'made by a newer Sidecar, so read-only here' };
			else if (!state.writable) result = { key, ok: false, reason: "its folder can't take an edit" };
			else {
				const edit = structuredClone(state.edit);
				change(edit);
				const r = await api(`/api/assets/${key}/edit`, { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ edit, base: state.head }) });
				result = r.ok ? { key, ok: true } : { key, ok: false, reason: `the server said ${r.status}` };
			}
		} catch (e) {
			result = { key, ok: false, reason: e instanceof HttpError ? e.message : String(e) };
		}
		done.push(result);
		progress?.([...done]);
	}
	return done;
}
