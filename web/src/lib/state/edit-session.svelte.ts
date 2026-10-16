// SPDX-License-Identifier: Apache-2.0
// One photo's edit as the editor holds it: loading it, saving it as a version in the sidecar's history
// (DESIGN.md §4, "Edit history"), the per-device draft, in-session undo, and the version tools. Nothing here
// draws: the page passes hooks for what follows a change on screen.
import { api, HttpError } from '$lib/api';
import { clearDraft, loadDraft, saveDraft } from '$lib/state/drafts';
import { PICKS } from '$lib/state/edit-view';
import { UndoStack } from '$lib/state/undo';
import type { EditParams, HistoryState, LensProfile } from '$lib/types';

/** What `GET`/`PUT …/edit` and a checkout answer. */
export type EditState = { edit: EditParams; etag: string; head: string; readOnly: boolean; writable?: boolean; exports: unknown[]; forked?: boolean; savedFor?: string };

export type SessionHooks = {
	/** The edit changed: develop it again. */
	redraw: () => void;
	/** A change is final (a gesture ended, a version opened): where the server renders, ask it now. */
	settled: () => void;
	/** The version compared against changed. */
	compared: () => void;
	error: (message: string) => void;
};

/** A pause after a gesture saves a checkpoint, so the history keeps up without anyone pressing Save. */
const CHECKPOINT_MS = 4000;

export class EditSession {
	edit = $state<EditParams | null>(null);
	readOnly = $state(false);
	/** False when the photo's folder can't take a sidecar: said up front rather than at the first save. */
	writable = $state(true);
	dirty = $state(false);
	saving = $state(false);
	restoredDraft = $state(false);
	/** Another device saved since this one loaded: both versions were kept, as branches. */
	forked = $state(false);
	/** The sidecar was last saved for a different file (its `sourceSha1`); cleared by the next save. */
	savedFor = $state<string | null>(null);
	canUndo = $state(false);
	canRedo = $state(false);
	versions = $state<HistoryState | null>(null);
	versionsBusy = $state(false);
	/** What pressing and holding the photo shows instead of the edit: a chosen version, or the original. */
	compare = $state<{ id: string; label: string; edit: EditParams } | null>(null);

	private etag = '';
	/** The history node the edit on screen came from: what the next save names as its base. */
	private head = '';
	/** This tab, so the server folds its checkpoints together and never another device's. */
	private readonly session = globalThis.crypto?.randomUUID?.() ?? Math.random().toString(36).slice(2);
	private saveQueue: Promise<void> = Promise.resolve();
	private draftTimer: ReturnType<typeof setTimeout> | undefined;
	private checkpointTimer: ReturnType<typeof setTimeout> | undefined;
	/** In-session undo: every gesture. The saved history is coarser — saves and checkpoints. */
	private undoStack = new UndoStack<EditParams | null>(null);
	/**
	 * Lens correction switched on by default for a photo never edited, without counting as a change: opening a
	 * photo must not write a sidecar. It is saved with the first real change, or before an export.
	 */
	private lensDefaulted = false;

	constructor(
		private readonly key: string,
		private readonly hooks: SessionHooks
	) {}

	/** Take the edit the server holds, or this device's unsaved draft of it, and the file's own lens correction. */
	async begin(state: EditState, lensProfile: LensProfile | null | undefined) {
		this.edit = state.edit;
		this.etag = state.etag;
		this.head = state.head;
		this.readOnly = state.readOnly;
		this.savedFor = state.savedFor ?? null;
		this.writable = state.writable !== false;
		const draft = await loadDraft(this.key);
		if (draft && draft.baseEtag === state.etag && JSON.stringify(draft.edit) !== JSON.stringify(state.edit)) {
			this.edit = draft.edit;
			this.dirty = true;
			this.restoredDraft = true;
		}
		// A photo never edited starts with the correction its file (or lensfun) provides (DESIGN.md §15).
		if (lensProfile && !this.edit!.lens && this.head === 'original' && !this.restoredDraft && !this.readOnly) {
			this.edit!.lens = { profile: lensProfile.id, distortion: 100, lateralCa: 100, vignetting: 100 };
			this.lensDefaulted = true;
		}
		this.undoStack = new UndoStack($state.snapshot(this.edit));
	}

	/** Continuous change during a gesture: redraw, mark unsaved, keep a device draft. */
	changed() {
		this.hooks.redraw();
		this.dirty = true;
		clearTimeout(this.checkpointTimer);
		clearTimeout(this.draftTimer);
		this.draftTimer = setTimeout(() => this.edit && saveDraft(this.key, { edit: $state.snapshot(this.edit), baseEtag: this.etag, savedAt: Date.now() }), 500);
	}

	/** A gesture ended: one undo step, and the moment to ask the server for a render where it renders. */
	committed() {
		if (this.edit) this.undoStack.commit($state.snapshot(this.edit));
		this.syncUndo();
		this.hooks.settled();
		this.scheduleCheckpoint();
	}

	undo() {
		this.restore(this.undoStack.undo());
	}

	redo() {
		this.restore(this.undoStack.redo());
	}

	private restore(state: EditParams | null) {
		if (!state || !this.edit) return;
		this.edit = state;
		this.changed();
		this.syncUndo();
		this.hooks.settled();
		this.scheduleCheckpoint();
	}

	private syncUndo() {
		this.canUndo = this.undoStack.canUndo;
		this.canRedo = this.undoStack.canRedo;
	}

	private scheduleCheckpoint() {
		clearTimeout(this.checkpointTimer);
		if (this.writable && !this.readOnly) this.checkpointTimer = setTimeout(() => this.commit(false, true), CHECKPOINT_MS);
	}

	/**
	 * Save the edit as a version in the photo's history. One PUT at a time, each sending the latest state: a
	 * call made while one is out waits for it and then saves whatever is still unsaved, instead of being
	 * dropped. `checkpoint` is the save a pause triggers; the server folds this tab's checkpoints together.
	 * `keepalive` lets it finish after the page is gone.
	 */
	commit(keepalive = false, checkpoint = false): Promise<void> {
		if (!checkpoint) clearTimeout(this.checkpointTimer);
		this.saveQueue = this.saveQueue.then(() => this.save(keepalive, checkpoint));
		return this.saveQueue;
	}

	private async save(keepalive: boolean, checkpoint: boolean) {
		if (!this.edit || !this.dirty || this.readOnly) return;
		this.saving = true;
		try {
			const sent = JSON.stringify($state.snapshot(this.edit));
			const put = (base: string) =>
				api(`/api/assets/${this.key}/edit`, {
					method: 'PUT',
					keepalive,
					headers: { 'content-type': 'application/json' },
					body: JSON.stringify({ edit: JSON.parse(sent), base, checkpoint, session: this.session })
				});
			let r: Response;
			try {
				r = await put(this.head);
			} catch (e) {
				// The version this page was built on isn't in the history any more: typically a page opened
				// before the server was updated, holding an id the new server never gave out. Save on top of
				// the current version instead of failing until a reload.
				if (!(e instanceof HttpError) || e.status !== 404) throw e;
				const current: EditState = await (await api(`/api/assets/${this.key}/edit`)).json();
				if (current.head === this.head) throw e;
				this.head = current.head;
				r = await put(this.head);
			}
			const state: EditState = await r.json();
			this.etag = state.etag;
			this.head = state.head;
			this.forked ||= !!state.forked;
			this.savedFor = state.savedFor ?? null;
			this.lensDefaulted = false;
			// Only what was sent is saved. A slider moved while the request was out used to be marked saved
			// here, and its draft cleared, so the change was lost without a word.
			if (JSON.stringify($state.snapshot(this.edit)) === sent) {
				this.dirty = false;
				this.restoredDraft = false;
				await clearDraft(this.key);
			} else {
				await saveDraft(this.key, { edit: $state.snapshot(this.edit), baseEtag: this.etag, savedAt: Date.now() });
			}
			if (this.versions) this.loadVersions();
		} catch (e) {
			this.hooks.error(e instanceof Error ? e.message : String(e));
		} finally {
			this.saving = false;
		}
	}

	/** Save if needed and return the ETag an export should pin, or null when saving failed. */
	async prepareExport(): Promise<string | null> {
		if (this.readOnly) return this.etag;
		// What's on screen is what gets exported: a correction switched on by default is saved first.
		if (this.lensDefaulted && this.edit?.lens) this.dirty = true;
		// Always through the queue: a save already out has to land before its ETag is the one to pin.
		await this.commit();
		return this.dirty ? null : this.etag;
	}

	async loadVersions() {
		try {
			this.versions = await (await api(`/api/assets/${this.key}/history`)).json();
			// A compared version may have been folded into a later checkpoint; keep comparing only what exists.
			if (this.compare && !this.versions!.nodes.some((n) => n.id === this.compare!.id)) this.compare = null;
		} catch (e) {
			this.hooks.error(e instanceof Error ? e.message : String(e));
		}
	}

	/** Make a version current. Unsaved changes are saved first, so opening something else never loses them. */
	async openVersion(id: string) {
		this.versionsBusy = true;
		try {
			await this.commit();
			if (this.dirty) return;
			const r = await api(`/api/assets/${this.key}/history/checkout`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ node: id }) });
			const state: EditState = await r.json();
			this.edit = state.edit;
			this.etag = state.etag;
			this.head = state.head;
			this.savedFor = state.savedFor ?? null;
			this.dirty = false;
			this.restoredDraft = false;
			await clearDraft(this.key);
			this.undoStack.reset($state.snapshot(this.edit));
			this.syncUndo();
			this.hooks.redraw();
			this.hooks.settled();
			await this.loadVersions();
		} catch (e) {
			this.hooks.error(e instanceof Error ? e.message : String(e));
		} finally {
			this.versionsBusy = false;
		}
	}

	/** Take one section — white balance, light, curve, mixer, detail — from another version into this edit. */
	pickFrom(id: string, section: string) {
		const from = this.versions?.nodes.find((n) => n.id === id);
		const pick = PICKS.find((p) => p.id === section);
		if (!this.edit || !from || !pick) return;
		pick.take(this.edit, from.edit);
		this.changed();
		this.committed();
	}

	async labelVersion(id: string, label: string | null) {
		try {
			await api(`/api/assets/${this.key}/history/${id}/label`, { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ label }) });
			await this.loadVersions();
		} catch (e) {
			this.hooks.error(e instanceof Error ? e.message : String(e));
		}
	}

	setCompare(id: string | null) {
		const n = id ? this.versions?.nodes.find((v) => v.id === id) : undefined;
		this.compare = n ? { id: n.id, label: n.label ?? (n.kind === 'original' ? 'Original' : 'Compared version'), edit: n.edit } : null;
		this.hooks.compared();
	}
}
