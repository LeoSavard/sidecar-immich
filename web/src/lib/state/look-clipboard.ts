// SPDX-License-Identifier: Apache-2.0
// A device-local clipboard for "the look": chosen sections of one photo's edit, pasted onto another, or onto a
// selection of photos (`$lib/state/batch`). A small global blob, so plain localStorage rather than drafts.ts's
// per-asset IndexedDB store.
import { PICKS } from '$lib/state/edit-view';
import type { EditParams } from '$lib/types';

/**
 * The sections a look carries unless chosen otherwise: the History tool's "use its …" sections that make up how
 * a photo looks. Lens correction, crop and masks belong to the photo they were made for. Taking them through
 * `PICKS` means a control added to one of these sections travels with a copied look without this file changing.
 */
export const LOOK_SECTIONS = ['wb', 'light', 'film', 'curve', 'color', 'detail'];

/** Every section a look or a preset can carry, as the editor names them. */
export const SECTIONS = PICKS.map(({ id, label }) => ({ id, label }));

export type LookClipboard = { edit: EditParams; from: string; copiedAt: number; sections: string[] };

const STORAGE_KEY = 'sidecar:look-clipboard';
const CHOICE_KEY = 'sidecar:look-sections';

/**
 * `edit` must be a plain object (`$state.snapshot`), not a Svelte proxy. `from` labels the paste button.
 * `sections` are what a paste takes; the choice is remembered for the next copy.
 */
export function copyLook(edit: EditParams, from: string, sections: string[] = LOOK_SECTIONS): void {
	try {
		localStorage.setItem(STORAGE_KEY, JSON.stringify({ edit, from, copiedAt: Date.now(), sections } satisfies LookClipboard));
		localStorage.setItem(CHOICE_KEY, JSON.stringify(sections));
	} catch {
		// Storage refused (a private window): there is nothing to paste from, which the paste button shows.
	}
}

export function readLookClipboard(): LookClipboard | null {
	try {
		const raw = localStorage.getItem(STORAGE_KEY);
		const entry = raw ? (JSON.parse(raw) as LookClipboard) : null;
		// A clipboard written by an earlier build of this file has no `edit`, and one a little later no `sections`.
		return entry?.edit ? { ...entry, sections: entry.sections ?? LOOK_SECTIONS } : null;
	} catch {
		return null;
	}
}

/** The sections last chosen for a copy, or the look's. */
export function lastSections(): string[] {
	try {
		const s = JSON.parse(localStorage.getItem(CHOICE_KEY) ?? 'null');
		return Array.isArray(s) && s.length ? s : LOOK_SECTIONS;
	} catch {
		return LOOK_SECTIONS;
	}
}

/** Takes `sections` of `from` into `into`, in place; everything else stays. */
export function applySections(into: EditParams, from: EditParams, sections: string[]): void {
	for (const p of PICKS) if (sections.includes(p.id)) p.take(into, from);
}

/** Applies the clipboard's look to `edit` in place, section by section; everything else stays. */
export function pasteLook(edit: EditParams): boolean {
	const entry = readLookClipboard();
	if (!entry) return false;
	applySections(edit, entry.edit, entry.sections);
	return true;
}

/** The sections, by label, for a button's title: "White balance, Light and Curve". */
export function describeSections(sections: string[]): string {
	const labels = SECTIONS.filter((s) => sections.includes(s.id)).map((s) => s.label);
	return labels.length > 1 ? `${labels.slice(0, -1).join(', ')} and ${labels.at(-1)}` : (labels[0] ?? 'nothing');
}
