// SPDX-License-Identifier: Apache-2.0
// Undo and redo within this page: edit snapshots, one per completed gesture, not per input event. Separate from
// the version history the sidecar keeps (xmp/history.rs), which outlives the page.
export class UndoStack<T> {
	private past: string[] = [];
	private future: string[] = [];
	private current: string;

	constructor(initial: T) {
		this.current = JSON.stringify(initial);
	}

	/** Record the state after a completed change. */
	commit(state: T) {
		const s = JSON.stringify(state);
		if (s === this.current) return;
		this.past.push(this.current);
		if (this.past.length > 200) this.past.shift();
		this.current = s;
		this.future = [];
	}

	reset(state: T) {
		this.current = JSON.stringify(state);
		this.past = [];
		this.future = [];
	}

	undo(): T | null {
		const prev = this.past.pop();
		if (prev === undefined) return null;
		this.future.push(this.current);
		this.current = prev;
		return JSON.parse(prev);
	}

	redo(): T | null {
		const next = this.future.pop();
		if (next === undefined) return null;
		this.past.push(this.current);
		this.current = next;
		return JSON.parse(next);
	}

	get canUndo() {
		return this.past.length > 0;
	}
	get canRedo() {
		return this.future.length > 0;
	}
}
