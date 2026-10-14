// SPDX-License-Identifier: Apache-2.0
// What every editor section gets: the edit to read, and a way to change it. Sections never mutate `edit`
// themselves; the page applies the change and does what follows one (redraw, undo, draft, checkpoint).
import type { EditParams } from '$lib/types';

/** Change the edit during a gesture. The page applies `mutate` to its edit, then redraws and marks it unsaved. */
export type Update = (mutate: (edit: EditParams) => void) => void;

export type SectionProps = {
	edit: EditParams;
	onchange: Update;
	/** A gesture ended: one undo step, and a checkpoint after a pause. */
	oncommit: () => void;
};
