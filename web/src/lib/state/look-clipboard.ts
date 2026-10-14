// SPDX-License-Identifier: Apache-2.0
// A device-local clipboard for "the look": the color recipe copied from one photo's edit and pasted onto
// another, as the foundation for a look workflow (DESIGN.md has no preset library yet — this is one slot).
// A small global blob, so plain localStorage rather than drafts.ts's per-asset IndexedDB store.
import type { EditParams } from '$lib/types';

/** What makes up "a look": color only. Geometry, masks and lens correction stay with the photo they're for. */
const LOOK_FIELDS = ['whiteBalance', 'exposure', 'tone', 'mixer', 'detail'] as const;

export type Look = Pick<EditParams, (typeof LOOK_FIELDS)[number]>;
export type LookClipboard = { look: Look; from: string; copiedAt: number };

const STORAGE_KEY = 'sidecar:look-clipboard';

/** Copies the color recipe out of `edit`. `from` is a label for the paste side, e.g. the source's file name. */
export function copyLook(edit: EditParams, from: string): void {
	const look: Look = {
		whiteBalance: structuredClone(edit.whiteBalance),
		exposure: structuredClone(edit.exposure),
		tone: structuredClone(edit.tone),
		mixer: structuredClone(edit.mixer),
		detail: structuredClone(edit.detail)
	};
	localStorage.setItem(STORAGE_KEY, JSON.stringify({ look, from, copiedAt: Date.now() } satisfies LookClipboard));
}

export function readLookClipboard(): LookClipboard | null {
	try {
		const raw = localStorage.getItem(STORAGE_KEY);
		return raw ? (JSON.parse(raw) as LookClipboard) : null;
	} catch {
		return null;
	}
}

/** Overwrites the look fields of `edit` in place with the clipboard's, leaving everything else untouched. */
export function pasteLook(edit: EditParams): boolean {
	const entry = readLookClipboard();
	if (!entry) return false;
	Object.assign(edit, structuredClone(entry.look));
	return true;
}
