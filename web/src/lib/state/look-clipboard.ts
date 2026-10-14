// SPDX-License-Identifier: Apache-2.0
// A device-local clipboard for "the look": the colour sections of one photo's edit, pasted onto another.
// A small global blob, so plain localStorage rather than drafts.ts's per-asset IndexedDB store.
import { PICKS } from '$lib/state/edit-view';
import type { EditParams } from '$lib/types';

/**
 * The History tool's "use its …" sections that make up a look. Lens correction, crop and masks belong to
 * the photo they were made for. Taking them through `PICKS` means a control added to one of these sections
 * travels with a copied look without this file changing.
 */
const LOOK_SECTIONS = ['wb', 'light', 'curve', 'color', 'detail'];

export type LookClipboard = { edit: EditParams; from: string; copiedAt: number };

const STORAGE_KEY = 'sidecar:look-clipboard';

/** `edit` must be a plain object (`$state.snapshot`), not a Svelte proxy. `from` labels the paste button. */
export function copyLook(edit: EditParams, from: string): void {
	localStorage.setItem(STORAGE_KEY, JSON.stringify({ edit, from, copiedAt: Date.now() } satisfies LookClipboard));
}

export function readLookClipboard(): LookClipboard | null {
	try {
		const raw = localStorage.getItem(STORAGE_KEY);
		const entry = raw ? (JSON.parse(raw) as LookClipboard) : null;
		// A clipboard written by an earlier build of this file has no `edit`.
		return entry?.edit ? entry : null;
	} catch {
		return null;
	}
}

/** Applies the clipboard's look to `edit` in place, section by section; everything else stays. */
export function pasteLook(edit: EditParams): boolean {
	const entry = readLookClipboard();
	if (!entry) return false;
	for (const p of PICKS) if (LOOK_SECTIONS.includes(p.id)) p.take(edit, entry.edit);
	return true;
}
