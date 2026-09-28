<!-- SPDX-License-Identifier: Apache-2.0 -->
<script lang="ts">
	import { page } from '$app/state';
	import { DevelopPass, requestDevice } from '$lib/gpu/develop';
	import { CLIP_HIGHS, CLIP_LOWS, MASK_INVERTED, PEAKING, SHOW_MASK, ViewPass, type Viewport } from '$lib/gpu/view';
	import { MaskLibrary, type MaskSet } from '$lib/masks';
	import { HistogramPass, type HistogramData } from '$lib/gpu/histogram';
	import { LensPass, correctedPoint } from '$lib/gpu/lens';
	import { ColorPacker } from '$lib/color/pack';
	import { api, HttpError } from '$lib/api';
	import { settings, SPACE_LABELS } from '$lib/settings';
	import { clearDraft, loadDraft, saveDraft } from '$lib/state/drafts';
	import { UndoStack } from '$lib/state/undo';
	import Slider from '$lib/ui/Slider.svelte';
	import Histogram from '$lib/ui/Histogram.svelte';
	import CurveEditor from '$lib/ui/CurveEditor.svelte';
	import ColorMixer from '$lib/ui/ColorMixer.svelte';
	import ExportSheet from '$lib/ui/ExportSheet.svelte';
	import Loupe from '$lib/ui/Loupe.svelte';
	import HistoryPanel from '$lib/ui/HistoryPanel.svelte';
	import MasksPanel from '$lib/ui/MasksPanel.svelte';
	import { CURVE_ANCHORS, PICKS, curveOffset, view as sliders, type Anchor } from '$lib/state/edit-view';
	import { decimals, signed } from '$lib/ui/format';
	import type { Band, DisplaySpace, EditParams, HistoryState, Lens, MaskAdjustment, Meta, MixerBand } from '$lib/types';

	type EditState = { edit: EditParams; etag: string; head: string; readOnly: boolean; writable?: boolean; exports: unknown[]; forked?: boolean };
	type Section = 'lens' | 'wb' | 'light' | 'curve' | 'color' | 'masks' | 'detail' | 'history';

	const key = page.params.key!;
	let canvas: HTMLCanvasElement;
	let meta = $state<Meta | null>(null);
	let edit = $state<EditParams | null>(null);
	let defaults: EditParams;
	let status = $state('Opening…');
	let error = $state('');
	/**
	 * The white balance the camera recorded, resolved once per photo. It is the Temperature slider's detent, so
	 * it must not follow the edit: it did (from each repack), and the detent then sat under the thumb, catching
	 * it every few pixels of a drag.
	 */
	let asShotWb = $state({ temperatureK: 5000, tint: 0 });
	let lut = $state<Float32Array | null>(null);
	let histogram = $state<HistogramData | null>(null);
	let clipHigh = $state(false);
	let clipLow = $state(false);
	let peaking = $state(false);
	let before = $state(false);
	let section = $state<Section>('light');
	let showPhoneHistogram = $state(true);
	let wide = $state(false);
	let exporting = $state(false);
	let loupeCenter = $state<[number, number]>([0.5, 0.5]);
	let detailOpen = $state(false);
	/** Screen position (CSS px, relative to the canvas's positioned ancestor) of the AF point marker. */
	let markerPos = $state<{ x: number; y: number } | null>(null);
	let showAfPoint = $state(true);
	/** A JPEG, HEIC, PNG or TIFF: something else already developed it, so some controls have nothing to act on. */
	const developed = $derived(meta?.info.sourceKind === 'developed');

	/**
	 * No WebGPU here, so the server runs the develop pass instead and sends back a JPEG — same shaders, same
	 * parameters, same picture, just not live. Sliders commit on release rather than on every move.
	 */
	let fallback = $state(false);
	let previewUrl = $state<string | null>(null);
	let previewBusy = $state(false);
	let previewPending = false;
	let displaySpace = $state<DisplaySpace>('srgb');
	let viewport: Viewport = { zoom: 1, panX: 0, panY: 0 };

	let etag = '';
	let readOnly = $state(false);
	/// False when the photo's folder can't take a sidecar: said here rather than at the first save.
	let writable = $state(true);
	let dirty = $state(false);
	let saving = $state(false);
	let saveQueue: Promise<void> = Promise.resolve();
	let restoredDraft = $state(false);
	let draftTimer: ReturnType<typeof setTimeout> | undefined;
	/** In-session undo: every gesture. The saved history below is coarser — saves and checkpoints. */
	let undo: UndoStack<EditParams>;
	let canUndo = $state(false);
	let canRedo = $state(false);

	/** The history node the edit on screen came from: what the next save names as its base. */
	let head = '';
	/** This tab, so the server folds its checkpoints together and never another device's. */
	const session = globalThis.crypto?.randomUUID?.() ?? Math.random().toString(36).slice(2);
	/** A pause after a gesture saves a checkpoint, so the history keeps up without anyone pressing Save. */
	let checkpointTimer: ReturnType<typeof setTimeout> | undefined;
	const CHECKPOINT_MS = 4000;
	/** Another device saved since this one loaded: both versions were kept, as branches. */
	let forked = $state(false);
	let versions = $state<HistoryState | null>(null);
	let versionsBusy = $state(false);
	/** What pressing and holding the photo shows instead of the edit: a chosen version, or the original. */
	let compare = $state<{ id: string; label: string; edit: EditParams } | null>(null);
	const compareEdit = () => compare?.edit ?? defaults;

	let develop: DevelopPass;
	let view: ViewPass;
	let hist: HistogramPass;
	let lensPass: LensPass;
	/** The lens pass's outputs at proxy size — the photo and its focus mask — and what they were made from. */
	let corrected: GPUTexture | undefined;
	let correctedMask: GPUTexture | undefined;
	let lensMadeFrom = '';
	/** The masks the models made for this photo, fetched when a mask is first needed (`$lib/masks`). */
	let maskLibrary: MaskLibrary | undefined;
	let maskSet = $state<MaskSet | null>(null);
	/** The adjustment whose mask is shown over the photo. */
	let maskShown = $state<number | null>(null);
	/** Masks lens-corrected as the picture is, and the correction each was made with. */
	const correctedMasks = new Map<string, { madeFrom: string; texture: GPUTexture }>();
	/** The uniforms the corrected proxy was made with: they also move markers set in sensor geometry. */
	let lensUniform: Uint8Array | undefined;
	/**
	 * Lens correction switched on by default for a photo never edited, without counting as a change: opening a
	 * photo must not write a sidecar. It is saved with the first real change, or before an export.
	 */
	let lensDefaulted = false;
	let packer: ColorPacker;
	let display: DisplaySpace = 'srgb';
	let developDirty = true;
	let frame = 0;
	let histStride = 1;

	function schedule(dev = false) {
		developDirty ||= dev;
		if (!frame) frame = requestAnimationFrame(draw);
	}

	/** Resolve the edit through the colour packer — wasm, so it works with or without a GPU. */
	function repack() {
		const source = before ? compareEdit() : $state.snapshot(edit!);
		const packed = packer.pack(source, meta!.info.color, meta!.info.sourceKind ?? 'raw', display);
		if (!before) lut = packed.lut;
		return packed;
	}

	function draw() {
		frame = 0;
		if (!edit || !meta) return;
		// Without WebGPU there is nothing to draw here: the picture is a JPEG the server developed. The
		// packer still runs, so the white-balance readout and the curve stay live while sliders move.
		if (fallback) {
			if (developDirty) {
				repack();
				developDirty = false;
			}
			return;
		}
		if (!develop?.output) return;
		if (developDirty) {
			correctLens();
			const packed = repack();
			develop.setUniforms(packed.uniform);
			develop.setToneLut(packed.lut);
			develop.setMasks(packed.masks, ((before ? compareEdit() : edit).masks ?? []).map((m) => maskTexture(m.mask)));
			develop.render();
			developDirty = false;
			hist.run(develop.output, histStride, (h) => (histogram = h));
		}
		const showing = maskShown === null ? undefined : edit.masks?.[maskShown];
		const overlay = showing && maskTexture(showing.mask);
		view.useOverlay(overlay);
		const maskFlags = overlay ? SHOW_MASK | (showing.invert ? MASK_INVERTED : 0) : 0;
		view.render(develop.output, canvas, viewport, (clipHigh ? CLIP_HIGHS : 0) | (clipLow ? CLIP_LOWS : 0) | (peaking ? PEAKING : 0) | maskFlags);
		markerPos = imageToScreen(afPoint());
	}

	/** Width ÷ height before orientation: where the photo's edges are for the lens pass. */
	const sensorAspect = (info: Meta['info']) => ((info.orientationFlip ?? 0) & 4 ? info.height / info.width : info.width / info.height);

	/**
	 * Run the lens pass over the proxy and its focus mask when the correction on screen changed — a lens
	 * setting, the version being compared, a new proxy — and point develop and the view at the results. Cheap
	 * to call on every develop: it does nothing unless something it depends on moved.
	 */
	function correctLens() {
		const proxy = develop.proxy;
		const profile = meta?.info.lensProfile;
		const lens = before ? compareEdit().lens : edit?.lens;
		const mask = view.focusMaskSource;
		const madeFrom = JSON.stringify([lens, proxy?.width, proxy?.height, mask.width, mask.height, profile?.id]);
		if (madeFrom === lensMadeFrom) return;
		lensMadeFrom = madeFrom;
		const flip = meta!.info.orientationFlip ?? 0;
		lensUniform = proxy && profile && lens ? packer.packLens(profile, $state.snapshot(lens), sensorAspect(meta!.info), [proxy.width, proxy.height], flip) : undefined;
		if (!proxy || !lensUniform) {
			develop.setInput(undefined);
			view.useFocusMask(undefined);
			return;
		}
		if (corrected?.width !== proxy.width || corrected?.height !== proxy.height) {
			corrected?.destroy();
			corrected = lensPass.target(proxy, 'lens corrected');
		}
		lensPass.run(proxy, corrected, lensUniform);
		develop.setInput(corrected);
		// The mask moves with the picture, but isn't a brightness: no vignetting gain on it.
		const maskUniform = mask.width > 1 ? packer.packLens(profile!, { ...$state.snapshot(lens!), vignetting: 0 }, sensorAspect(meta!.info), [mask.width, mask.height], flip) : undefined;
		if (maskUniform) {
			if (correctedMask?.width !== mask.width || correctedMask?.height !== mask.height) {
				correctedMask?.destroy();
				correctedMask = lensPass.target(mask, 'lens corrected focus mask');
			}
			lensPass.run(mask, correctedMask, maskUniform);
			view.useFocusMask(correctedMask);
		} else view.useFocusMask(undefined);
	}

	/**
	 * A mask in the frame being developed: loaded, and lens-corrected as the picture is (without vignetting,
	 * which would change its values rather than move them). `undefined` until it has loaded — it then weighs
	 * nothing — and loading it redraws.
	 */
	function maskTexture(name: string): GPUTexture | undefined {
		if (!maskLibrary) return undefined;
		const raw = maskLibrary.ready(name);
		if (!raw) {
			maskLibrary.load(name).then(() => schedule(true), () => {});
			return undefined;
		}
		const profile = meta?.info.lensProfile;
		const lens = before ? compareEdit().lens : edit?.lens;
		const uniform = profile && lens ? packer.packLens(profile, { ...$state.snapshot(lens), vignetting: 0 }, sensorAspect(meta!.info), [raw.width, raw.height], meta!.info.orientationFlip ?? 0) : undefined;
		if (!uniform) return raw;
		const madeFrom = JSON.stringify([lens, profile!.id]);
		const hit = correctedMasks.get(name);
		if (hit?.madeFrom === madeFrom) return hit.texture;
		const texture = hit?.texture ?? lensPass.target(raw, `lens corrected ${name}`);
		lensPass.run(raw, texture, uniform);
		correctedMasks.set(name, { madeFrom, texture });
		return texture;
	}

	/** Ask the worker which masks this photo has — the first time for a photo, that computes them. */
	function loadMaskSet() {
		if (!maskLibrary || maskSet) return;
		maskLibrary.manifest().then((set) => {
			maskSet = set;
			schedule(true);
		});
	}

	function addMask(name: string) {
		if (!edit) return;
		const m: MaskAdjustment = { mask: name, invert: false, amount: 100, exposure: 0, contrast: 0, saturation: 0, temperature: 0, tint: 0 };
		edit.masks = [...(edit.masks ?? []), m];
		maskShown = edit.masks.length - 1;
		changed();
		committed();
	}

	function removeMask(index: number) {
		if (!edit?.masks) return;
		const rest = edit.masks.filter((_, i) => i !== index);
		// No masks is no field at all: the edit then serializes exactly as one made before masks existed.
		if (rest.length) edit.masks = rest;
		else delete edit.masks;
		maskShown = null;
		changed();
		committed();
	}

	function setMask(index: number, patch: Partial<MaskAdjustment>) {
		if (!edit?.masks?.[index]) return;
		Object.assign(edit.masks[index], patch);
		changed();
	}

	/** The AF point where it sits in the picture on screen — moved by the lens correction when one is on. */
	function afPoint() {
		const p = meta?.info.afPoint;
		const proxy = develop?.proxy;
		return p && proxy ? correctedPoint(lensUniform, proxy.width, proxy.height, p) : p;
	}

	/** Where the photo sits on the canvas, in canvas pixels, at the current zoom and pan; `fit` is its scale at zoom 1. */
	function imageRect() {
		const out = develop.output!;
		const fit = Math.min(canvas.width / out.width, canvas.height / out.height);
		const w = out.width * fit * viewport.zoom;
		const h = out.height * fit * viewport.zoom;
		return { x: (canvas.width - w) / 2 + viewport.panX, y: (canvas.height - h) / 2 + viewport.panY, w, h, fit };
	}

	/** Normalized (oriented) image coordinates → CSS pixels over the canvas, or null off-screen/absent. */
	function imageToScreen(point: { x: number; y: number } | null | undefined): { x: number; y: number } | null {
		if (!point || !develop?.output) return null;
		const r = imageRect();
		const px = r.x + point.x * r.w;
		const py = r.y + point.y * r.h;
		if (px < 0 || py < 0 || px > canvas.width || py > canvas.height) return null;
		return { x: px / devicePixelRatio, y: py / devicePixelRatio };
	}

	/** Move the loupe to the AF point and make sure it's visible. */
	function jumpToAfPoint() {
		const point = afPoint();
		if (!point) return;
		loupeCenter = [point.x, point.y];
		section = 'detail';
		detailOpen = true;
	}

	/**
	 * Ask the server for a freshly developed JPEG. Only ever one request in flight: a change arriving while
	 * one is running sets a flag and is served by another round after it, so dragging a slider can't queue a
	 * render per frame.
	 */
	async function renderPreview() {
		if (!edit || !meta) return;
		if (previewBusy) {
			previewPending = true;
			return;
		}
		previewBusy = true;
		try {
			do {
				previewPending = false;
				const body = JSON.stringify({
					edit: before ? compareEdit() : $state.snapshot(edit),
					colorSpace: display,
					longEdge: Math.round(Math.min(Math.max(innerWidth, innerHeight) * devicePixelRatio, 2560))
				});
				const r = await api(`/api/assets/${key}/preview`, { method: 'POST', headers: { 'content-type': 'application/json' }, body });
				const url = URL.createObjectURL(await r.blob());
				if (previewUrl) URL.revokeObjectURL(previewUrl);
				previewUrl = url;
			} while (previewPending);
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			previewBusy = false;
		}
	}

	/** Continuous change during a gesture: redraw, mark unsaved, keep a device draft. */
	function changed() {
		schedule(true);
		dirty = true;
		clearTimeout(checkpointTimer);
		clearTimeout(draftTimer);
		draftTimer = setTimeout(() => edit && saveDraft(key, { edit: $state.snapshot(edit), baseEtag: etag, savedAt: Date.now() }), 500);
	}

	/** A gesture ended: one undo step, and in fallback mode the moment to ask for a new render. */
	function committed() {
		if (edit) undo.commit($state.snapshot(edit));
		syncUndo();
		if (fallback) renderPreview();
		scheduleCheckpoint();
	}

	function scheduleCheckpoint() {
		clearTimeout(checkpointTimer);
		if (writable && !readOnly) checkpointTimer = setTimeout(() => commit(false, true), CHECKPOINT_MS);
	}

	function syncUndo() {
		canUndo = undo.canUndo;
		canRedo = undo.canRedo;
	}

	function restore(state: EditParams | null) {
		if (!state || !edit) return;
		edit = state;
		changed();
		syncUndo();
		if (fallback) renderPreview();
		scheduleCheckpoint();
	}

	/**
	 * Save the edit as a version in the photo's history. One PUT at a time, each sending the latest state: a
	 * call made while one is out waits for it and then saves whatever is still unsaved, instead of being
	 * dropped. `checkpoint` is the save a pause triggers; the server folds this tab's checkpoints together.
	 */
	function commit(keepalive = false, checkpoint = false): Promise<void> {
		if (!checkpoint) clearTimeout(checkpointTimer);
		saveQueue = saveQueue.then(() => save(keepalive, checkpoint));
		return saveQueue;
	}

	async function save(keepalive: boolean, checkpoint: boolean) {
		if (!edit || !dirty || readOnly) return;
		saving = true;
		try {
			const sent = JSON.stringify($state.snapshot(edit));
			const put = (base: string) =>
				api(`/api/assets/${key}/edit`, {
					method: 'PUT',
					keepalive,
					headers: { 'content-type': 'application/json' },
					body: JSON.stringify({ edit: JSON.parse(sent), base, checkpoint, session })
				});
			let r: Response;
			try {
				r = await put(head);
			} catch (e) {
				// The version this page was built on isn't in the history any more: typically a page opened
				// before the server was updated, holding an id the new server never gave out. Save on top of
				// the current version instead of failing until a reload.
				if (!(e instanceof HttpError) || e.status !== 404) throw e;
				const current: EditState = await (await api(`/api/assets/${key}/edit`)).json();
				if (current.head === head) throw e;
				head = current.head;
				r = await put(head);
			}
			const state: EditState = await r.json();
			etag = state.etag;
			head = state.head;
			forked ||= !!state.forked;
			lensDefaulted = false;
			// Only what was sent is saved. A slider moved while the request was out used to be marked saved
			// here, and its draft cleared, so the change was lost without a word.
			if (JSON.stringify($state.snapshot(edit)) === sent) {
				dirty = false;
				restoredDraft = false;
				await clearDraft(key);
			} else {
				await saveDraft(key, { edit: $state.snapshot(edit), baseEtag: etag, savedAt: Date.now() });
			}
			if (versions) loadVersions();
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			saving = false;
		}
	}

	/** Save if needed and return the ETag an export should pin, or null when saving failed. */
	async function prepareExport(): Promise<string | null> {
		if (readOnly) return etag;
		// What's on screen is what gets exported: a correction switched on by default is saved first.
		if (lensDefaulted && edit?.lens) dirty = true;
		// Always through the queue: a save already out has to land before its ETag is the one to pin.
		await commit();
		return dirty ? null : etag;
	}

	async function loadVersions() {
		try {
			versions = await (await api(`/api/assets/${key}/history`)).json();
			// A compared version may have been folded into a later checkpoint; keep comparing only what exists.
			if (compare && !versions!.nodes.some((n) => n.id === compare!.id)) compare = null;
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
	}

	/** Make a version current. Unsaved changes are saved first, so opening something else never loses them. */
	async function openVersion(id: string) {
		versionsBusy = true;
		try {
			await commit();
			if (dirty) return;
			const r = await api(`/api/assets/${key}/history/checkout`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ node: id }) });
			const state: EditState = await r.json();
			edit = state.edit;
			etag = state.etag;
			head = state.head;
			dirty = false;
			restoredDraft = false;
			await clearDraft(key);
			undo.reset($state.snapshot(edit));
			syncUndo();
			schedule(true);
			if (fallback) renderPreview();
			await loadVersions();
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			versionsBusy = false;
		}
	}

	/** Take one section — white balance, light, curve, mixer, detail — from another version into this edit. */
	function pickFrom(id: string, section: string) {
		const from = versions?.nodes.find((n) => n.id === id);
		const pick = PICKS.find((p) => p.id === section);
		if (!edit || !from || !pick) return;
		pick.take(edit, from.edit);
		changed();
		committed();
	}

	async function labelVersion(id: string, label: string | null) {
		try {
			await api(`/api/assets/${key}/history/${id}/label`, { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ label }) });
			await loadVersions();
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
	}

	function setCompare(id: string | null) {
		const n = id ? versions?.nodes.find((v) => v.id === id) : undefined;
		compare = n ? { id: n.id, label: n.label ?? (n.kind === 'original' ? 'Original' : 'Compared version'), edit: n.edit } : null;
		if (before) {
			schedule(true);
			if (fallback) renderPreview();
		}
	}

	// Masks are asked for when a tool needs them — the section is opened, or the edit has some — not with
	// every photo: the first ask computes them.
	$effect(() => {
		if (section === 'masks' || wide || (edit?.masks?.length ?? 0) > 0) loadMaskSet();
	});

	// The history loads when it's first looked at, not with every photo.
	$effect(() => {
		if ((section === 'history' || wide) && edit && !versions) loadVersions();
	});

	async function loadProxy(longEdge: number) {
		const [proxyResp, focusResp] = await Promise.all([api(`/api/assets/${key}/proxy?long_edge=${longEdge}`), api(`/api/assets/${key}/focus?edge=${longEdge}`)]);
		const w = Number(proxyResp.headers.get('x-width'));
		const h = Number(proxyResp.headers.get('x-height'));
		develop.setSource(w, h, await proxyResp.arrayBuffer());
		await view.loadFocusMask(await focusResp.arrayBuffer());
		schedule(true);
	}

	async function start() {
		try {
			const [metaResp, stateResp] = await Promise.all([api(`/api/assets/${key}/meta`), api(`/api/assets/${key}/edit`)]);
			meta = await metaResp.json();
			const state: EditState = await stateResp.json();
			// A browser without WebGPU still edits: the server runs the develop pass instead. Logged, not
			// swallowed silently: "no WebGPU" in the UI can mean navigator.gpu is missing, requestAdapter()
			// found no adapter, or requestDevice() itself rejected, and only the console message says which.
			const [device, p] = await Promise.all([
				requestDevice().catch((e) => {
					console.error('WebGPU unavailable, falling back to server-rendered preview:', e);
					return null;
				}),
				ColorPacker.load(),
			]);
			fallback = device === null;
			packer = p;
			defaults = packer.defaultEdit();
			const asShot = packer.pack(defaults, meta!.info.color, meta!.info.sourceKind ?? 'raw', display);
			asShotWb = { temperatureK: asShot.temperatureK, tint: asShot.tint };
			edit = state.edit;
			etag = state.etag;
			head = state.head;
			readOnly = state.readOnly;
			writable = state.writable !== false;
			const draft = await loadDraft(key);
			if (draft && draft.baseEtag === state.etag && JSON.stringify(draft.edit) !== JSON.stringify(state.edit)) {
				edit = draft.edit;
				dirty = true;
				restoredDraft = true;
			}
			// A photo never edited starts with the correction its file (or lensfun) provides (DESIGN.md §15).
			const profile = meta!.info.lensProfile;
			if (profile && !edit!.lens && head === 'original' && !restoredDraft && !readOnly) {
				edit!.lens = { profile: profile.id, distortion: 100, lateralCa: 100, vignetting: 100 };
				lensDefaulted = true;
			}
			undo = new UndoStack($state.snapshot(edit));
			const prefs = await settings();
			display = prefs.outputSpace;
			displaySpace = display;
			clipHigh = prefs.clipHighlights;
			clipLow = prefs.clipShadows;
			if (!device) {
				status = 'Rendering on the server…';
				await renderPreview();
				status = '';
				return;
			}
			[develop, view, hist, lensPass] = await Promise.all([DevelopPass.create(device), ViewPass.create(device, canvas, display === 'display-p3'), HistogramPass.create(device), LensPass.create(device)]);
			maskLibrary = new MaskLibrary(device, key);
			if (section === 'masks' || wide || (edit?.masks?.length ?? 0) > 0) loadMaskSet();
			device.lost.then((info) => (error = `The GPU was reset (${info.reason}). Reload to continue — your edits are kept as a draft.`));
			const phone = matchMedia('(pointer: coarse)').matches;
			histStride = phone ? 2 : 1;
			const wanted = Math.round(Math.max(innerWidth, innerHeight) * devicePixelRatio * 1.25);
			const edge = Math.min(wanted, device.limits.maxTextureDimension2D, phone ? 2560 : 4096);
			status = 'Loading preview…';
			await loadProxy(768);
			status = '';
			await loadProxy(edge);
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
	}

	// ---------- gestures on the photo: pinch zoom, pan, double-tap 100 %, press-and-hold for as shot
	const pointers = new Map<number, { x: number; y: number }>();
	let pinch: { dist: number; zoom: number; mid: { x: number; y: number }; panX: number; panY: number } | null = null;
	let holdTimer: ReturnType<typeof setTimeout> | undefined;
	let downAt = { x: 0, y: 0, t: 0 };
	let lastTapAt = 0;

	const px = (e: PointerEvent) => ({ x: e.offsetX * devicePixelRatio, y: e.offsetY * devicePixelRatio });

	function zoomAt(point: { x: number; y: number }, nextZoom: number, from = { zoom: viewport.zoom, panX: viewport.panX, panY: viewport.panY }) {
		const z = Math.min(16, Math.max(1, nextZoom));
		const cx = canvas.width / 2;
		const cy = canvas.height / 2;
		const k = z / from.zoom;
		viewport = { zoom: z, panX: (point.x - cx) * (1 - k) + from.panX * k, panY: (point.y - cy) * (1 - k) + from.panY * k };
		if (z === 1) viewport = { zoom: 1, panX: 0, panY: 0 };
		schedule();
	}

	function setBefore(on: boolean) {
		if (before === on) return;
		before = on;
		schedule(true);
		if (fallback) renderPreview();
	}

	function canvasDown(e: PointerEvent) {
		canvas.setPointerCapture(e.pointerId);
		pointers.set(e.pointerId, px(e));
		downAt = { ...px(e), t: performance.now() };
		clearTimeout(holdTimer);
		if (pointers.size === 1) holdTimer = setTimeout(() => setBefore(true), 450);
		if (pointers.size === 2) {
			clearTimeout(holdTimer);
			const [a, b] = [...pointers.values()];
			pinch = { dist: Math.hypot(a.x - b.x, a.y - b.y), zoom: viewport.zoom, mid: { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 }, panX: viewport.panX, panY: viewport.panY };
		}
	}

	function canvasMove(e: PointerEvent) {
		const prev = pointers.get(e.pointerId);
		if (!prev) return;
		const cur = px(e);
		pointers.set(e.pointerId, cur);
		if (Math.hypot(cur.x - downAt.x, cur.y - downAt.y) > 8 * devicePixelRatio) clearTimeout(holdTimer);
		if (pinch && pointers.size >= 2) {
			const [a, b] = [...pointers.values()];
			zoomAt(pinch.mid, pinch.zoom * (Math.hypot(a.x - b.x, a.y - b.y) / pinch.dist), { zoom: pinch.zoom, panX: pinch.panX, panY: pinch.panY });
		} else if (pointers.size === 1 && viewport.zoom > 1) {
			viewport = { ...viewport, panX: viewport.panX + cur.x - prev.x, panY: viewport.panY + cur.y - prev.y };
			schedule();
		}
	}

	function canvasUp(e: PointerEvent) {
		pointers.delete(e.pointerId);
		clearTimeout(holdTimer);
		if (pointers.size < 2) pinch = null;
		if (before) {
			setBefore(false);
			return;
		}
		const now = performance.now();
		const still = Math.hypot(px(e).x - downAt.x, px(e).y - downAt.y) < 8 * devicePixelRatio && now - downAt.t < 300;
		if (still && pointers.size === 0) {
			if (fallback || !develop?.output) return;
			const r = imageRect();
			const u = (px(e).x - r.x) / r.w;
			const v = (px(e).y - r.y) / r.h;
			if (u >= 0 && u <= 1 && v >= 0 && v <= 1 && (section === 'detail' || detailOpen)) loupeCenter = [u, v];
			if (now - lastTapAt < 320) {
				zoomAt(px(e), viewport.zoom > 1.01 ? 1 : Math.max(2, 1 / r.fit));
				lastTapAt = 0;
			} else lastTapAt = now;
		}
	}

	function wheel(e: WheelEvent) {
		e.preventDefault();
		zoomAt({ x: e.offsetX * devicePixelRatio, y: e.offsetY * devicePixelRatio }, viewport.zoom * Math.exp(-e.deltaY * 0.002));
	}

	function keydown(e: KeyboardEvent) {
		if ((e.target as HTMLElement)?.tagName === 'INPUT') return;
		if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'z') {
			e.preventDefault();
			restore(e.shiftKey ? undo.redo() : undo.undo());
		} else if (e.key === '\\') setBefore(!before);
		else if (e.key.toLowerCase() === 'e' && !e.metaKey && !e.ctrlKey) exporting = true;
		else if (e.key.toLowerCase() === 'z' && !e.metaKey && !e.ctrlKey) {
			section = 'detail';
			detailOpen = !detailOpen;
		}
		else if (e.key.toLowerCase() === 'j') {
			const on = !(clipHigh && clipLow);
			clipHigh = clipLow = on;
			schedule();
		}
	}

	$effect(() => {
		const mq = matchMedia('(min-width: 1024px)');
		wide = mq.matches;
		const onWide = () => (wide = mq.matches);
		mq.addEventListener('change', onWide);
		const ro = new ResizeObserver(() => {
			canvas.width = Math.round(canvas.clientWidth * devicePixelRatio);
			canvas.height = Math.round(canvas.clientHeight * devicePixelRatio);
			schedule();
		});
		ro.observe(canvas);
		const onHide = () => document.visibilityState === 'hidden' && commit(true);
		const noGesture = (e: Event) => e.preventDefault();
		document.addEventListener('visibilitychange', onHide);
		canvas.addEventListener('gesturestart', noGesture);
		canvas.addEventListener('gesturechange', noGesture);
		canvas.addEventListener('wheel', wheel, { passive: false });
		start();
		return () => {
			ro.disconnect();
			mq.removeEventListener('change', onWide);
			document.removeEventListener('visibilitychange', onHide);
			canvas.removeEventListener('gesturestart', noGesture);
			canvas.removeEventListener('gesturechange', noGesture);
			canvas.removeEventListener('wheel', wheel);
		};
	});

	// ---------- edit helpers
	function setWb(field: 'temperatureK' | 'tint', value: number) {
		if (!edit) return;
		if (edit.whiteBalance.mode === 'asShot') {
			edit.whiteBalance.temperatureK = Math.round(asShotWb.temperatureK);
			edit.whiteBalance.tint = Math.round(asShotWb.tint);
			edit.whiteBalance.mode = 'custom';
		}
		edit.whiteBalance[field] = value;
		changed();
	}

	function setCurveOffset(which: Anchor, v: number) {
		if (!edit) return;
		const x = CURVE_ANCHORS[which];
		const rest = edit.tone.curve.filter((q) => Math.abs(q[0] - x) >= 1e-6);
		edit.tone.curve = v === 0 ? rest : [...rest, [x, Math.min(1, Math.max(0, x + (v / 100) * 0.15))] as [number, number]].sort((a, b) => a[0] - b[0]);
		changed();
	}

	function setLens(on: boolean) {
		const profile = meta?.info.lensProfile;
		if (!edit || !profile) return;
		edit.lens = on ? { profile: profile.id, distortion: 100, lateralCa: 100, vignetting: 100 } : null;
		changed();
		committed();
	}

	function setLensStrength(field: keyof Omit<Lens, 'profile'>, v: number) {
		if (!edit?.lens) return;
		edit.lens[field] = v;
		changed();
	}

	const shutter = (s: number) => (s >= 1 ? `${s}s` : `1/${Math.round(1 / s)}`);

	/** Camera line, skipping whatever this file didn't record — plenty of JPEGs carry almost no EXIF. */
	const cameraLine = (info: Meta['info']) =>
		[
			[info.make, info.model].filter(Boolean).join(' '),
			info.iso > 0 && `ISO ${info.iso}`,
			info.shutterS > 0 && shutter(info.shutterS),
			info.aperture > 0 && `f/${info.aperture}`
		]
			.filter(Boolean)
			.join(' · ');
	const TEMP_TRACK = 'linear-gradient(90deg,#5f7fb6,#9aa0a6 45%,#c9a15f)';
	const TINT_TRACK = 'linear-gradient(90deg,#6f9f6f,#9aa0a6 50%,#b07aa6)';
	const sections: { id: Section; label: string; short: string }[] = [
		{ id: 'lens', label: 'Lens', short: 'Lens' },
		{ id: 'wb', label: 'White balance', short: 'WB' },
		{ id: 'light', label: 'Light', short: 'Light' },
		{ id: 'curve', label: 'Curve', short: 'Curve' },
		{ id: 'color', label: 'Color', short: 'Color' },
		{ id: 'masks', label: 'Masks', short: 'Masks' },
		{ id: 'detail', label: 'Detail', short: 'Detail' },
		{ id: 'history', label: 'History', short: 'History' }
	];
</script>

<svelte:head><title>{meta?.fileName ?? 'Sidecar'}</title></svelte:head>
<svelte:window onkeydown={keydown} />

{#snippet lensSection()}
	{#if edit && meta}
		{@const profile = meta.info.lensProfile}
		{@const name = meta.info.lensIdentity?.lensId ?? meta.info.lensIdentity?.lensModel ?? meta.info.lens}
		<div class="grid gap-2">
			{#if name}<p class="text-xs text-fg-2">{name}</p>{/if}
			{#if !profile}
				<p class="text-xs text-fg-2">
					{developed ? 'This picture was already developed, lens and all.' : 'No correction for this lens: the file carries none, and no lens database profile matches it yet.'}
				</p>
			{:else}
				<div class="flex items-center justify-between text-xs">
					<span class="text-fg-2">{profile.label}</span>
					<button type="button" aria-pressed={!!edit.lens} class="rounded-full px-3 py-1 {edit.lens ? 'bg-fg text-ground' : 'bg-raised text-fg-2'}" onclick={() => setLens(!edit!.lens)}>{edit.lens ? 'On' : 'Off'}</button>
				</div>
				{#if profile.required && !edit.lens}
					<p class="text-xs text-warning">This lens is designed to be corrected in software. Without it the frame shows distortion the camera never showed.</p>
				{/if}
				{#if edit.lens}
					{#if profile.distortion}
						<Slider id="lens-distortion" label="Distortion" value={edit.lens.distortion} min={0} max={200} defaultValue={100} format={(v) => `${decimals(v)} %`} onchange={(v) => setLensStrength('distortion', v)} oncommit={committed} />
					{/if}
					{#if profile.caRed || profile.caBlue}
						<Slider id="lens-ca" label="Chromatic aberration" value={edit.lens.lateralCa} min={0} max={200} defaultValue={100} format={(v) => `${decimals(v)} %`} onchange={(v) => setLensStrength('lateralCa', v)} oncommit={committed} />
					{/if}
					{#if profile.vignetting}
						<Slider id="lens-vignetting" label="Vignetting" value={edit.lens.vignetting} min={0} max={200} defaultValue={100} format={(v) => `${decimals(v)} %`} onchange={(v) => setLensStrength('vignetting', v)} oncommit={committed} />
					{/if}
				{/if}
			{/if}
		</div>
	{/if}
{/snippet}

{#snippet wbSection()}
	{#if edit}
		<div class="grid gap-2">
			<div class="flex items-center justify-between text-xs">
				<span class="text-fg-2">{edit.whiteBalance.mode === 'asShot' ? (developed ? 'As saved' : 'As shot') : 'Custom'}</span>
				<button type="button" class="rounded px-2 py-1 text-fg-2 hover:bg-raised disabled:opacity-40" disabled={edit.whiteBalance.mode === 'asShot'} onclick={() => { edit!.whiteBalance.mode = 'asShot'; changed(); committed(); }}>Reset to {developed ? 'as saved' : 'as shot'}</button>
			</div>
			<Slider id="wb-temp" label="Temperature" value={edit.whiteBalance.mode === 'asShot' ? Math.round(asShotWb.temperatureK) : edit.whiteBalance.temperatureK} min={2000} max={12000} step={10} defaultValue={Math.round(asShotWb.temperatureK)} trackGradient={TEMP_TRACK} format={(v) => `${decimals(v)} K`} onchange={(v) => setWb('temperatureK', v)} oncommit={committed} />
			<Slider id="wb-tint" label="Tint" value={edit.whiteBalance.mode === 'asShot' ? Math.round(asShotWb.tint) : edit.whiteBalance.tint} min={-150} max={150} defaultValue={0} bipolar trackGradient={TINT_TRACK} format={(v) => signed(v)} onchange={(v) => setWb('tint', v)} oncommit={committed} />
		</div>
	{/if}
{/snippet}

{#snippet lightSection()}
	{#if edit}
		<div class="grid gap-2">
			<!-- A developed file has no headroom past white to recover: ±2 EV is already most of what it holds. -->
			<Slider id="exposure" label="Exposure" value={edit.exposure.ev} min={developed ? -2 : -5} max={developed ? 2 : 5} step={0.01} defaultValue={0} bipolar format={(v) => `${signed(v, 2)} EV`} onchange={(v) => { edit!.exposure.ev = v; changed(); }} oncommit={committed} />
			<Slider id="contrast" label="Contrast" value={sliders.contrast(edit)} min={-70} max={100} defaultValue={0} bipolar format={(v) => signed(v)} onchange={(v) => { edit!.tone.contrast = 1 + v / 100; changed(); }} oncommit={committed} />
			<Slider id="highlights" label="Highlights" value={curveOffset(edit, 'highlights')} min={-100} max={100} defaultValue={0} bipolar format={(v) => signed(v)} onchange={(v) => setCurveOffset('highlights', v)} oncommit={committed} />
			<Slider id="shadows" label="Shadows" value={curveOffset(edit, 'shadows')} min={-100} max={100} defaultValue={0} bipolar format={(v) => signed(v)} onchange={(v) => setCurveOffset('shadows', v)} oncommit={committed} />
			<Slider id="whites" label="Whites" value={sliders.whites(edit)} min={-100} max={100} defaultValue={0} bipolar format={(v) => signed(v)} onchange={(v) => { edit!.tone.whiteEv = 4.5 - (v / 100) * 2; changed(); }} oncommit={committed} />
			<Slider id="blacks" label="Blacks" value={sliders.blacks(edit)} min={-100} max={100} defaultValue={0} bipolar format={(v) => signed(v)} onchange={(v) => { edit!.tone.blackEv = -8 - (v / 100) * 3; changed(); }} oncommit={committed} />
			<!-- Highlight recovery reconstructs clipped sensor channels; an already-developed file has none to
			     reconstruct, so the control would sit there doing nothing. -->
			{#if !developed}
				<Slider id="hl-blend" label="Highlight recovery" value={sliders.highlightRecovery(edit)} min={0} max={100} defaultValue={50} format={(v) => `${decimals(v)} %`} onchange={(v) => { edit!.exposure.highlightBlend = v / 100; changed(); }} oncommit={committed} />
			{/if}
		</div>
	{/if}
{/snippet}

{#snippet curveSection()}
	{#if edit}
		<div class="grid gap-2">
			<CurveEditor points={edit.tone.curve} {lut} onchange={(p) => { edit!.tone.curve = p; changed(); }} oncommit={committed} />
			<button type="button" class="justify-self-end rounded px-2 py-1 text-xs text-fg-2 hover:bg-raised" onclick={() => { edit!.tone.curve = [[0, 0], [1, 1]]; changed(); committed(); }}>Reset curve</button>
		</div>
	{/if}
{/snippet}

{#snippet detailSection()}
	{#if edit}
		<div class="grid gap-2">
			<Loupe assetKey={key} edit={$state.snapshot(edit)} center={loupeCenter} colorSpace={displaySpace} />
			<p class="text-xs text-muted">Tap the photo to move the loupe.</p>
			<Slider id="sharpen" label="Sharpening" value={edit.detail.sharpenAmount} min={0} max={100} defaultValue={0} format={(v) => decimals(v)} onchange={(v) => { edit!.detail.sharpenAmount = v; changed(); }} oncommit={committed} />
			<Slider id="sharpen-radius" label="Sharpening radius" value={edit.detail.sharpenRadius} min={0.5} max={2} step={0.1} defaultValue={1} format={(v) => `${decimals(v, 1)} px`} onchange={(v) => { edit!.detail.sharpenRadius = v; changed(); }} oncommit={committed} />
			<Slider id="denoise-luma" label="Noise reduction" value={edit.detail.denoiseLuminance} min={0} max={100} defaultValue={0} format={(v) => decimals(v)} onchange={(v) => { edit!.detail.denoiseLuminance = v; changed(); }} oncommit={committed} />
			<Slider id="denoise-chroma" label="Colour noise reduction" value={edit.detail.denoiseChroma} min={0} max={100} defaultValue={0} format={(v) => decimals(v)} onchange={(v) => { edit!.detail.denoiseChroma = v; changed(); }} oncommit={committed} />
		</div>
	{/if}
{/snippet}

{#snippet masksSection()}
	{#if edit}
		<MasksPanel masks={edit.masks ?? []} set={maskSet} shown={maskShown} disabled={readOnly} onadd={addMask} onremove={removeMask} onchange={setMask} oncommit={committed} onshow={(i) => { maskShown = i; schedule(); }} />
	{/if}
{/snippet}

{#snippet historySection()}
	{#if versions}
		<HistoryPanel history={versions} compareId={compare?.id ?? null} busy={versionsBusy || saving} onopen={openVersion} oncompare={setCompare} onpick={pickFrom} onlabel={labelVersion} />
	{:else}
		<p class="text-xs text-fg-2">Loading history…</p>
	{/if}
{/snippet}

{#snippet colorSection()}
	{#if edit}
		<ColorMixer mixer={edit.mixer} onchange={(band: Band, field: keyof MixerBand, v: number) => { edit!.mixer[band][field] = v; changed(); }} oncommit={committed} />
	{/if}
{/snippet}

<div class="flex h-dvh flex-col overflow-hidden bg-ground text-fg select-none [-webkit-touch-callout:none] lg:flex-row">
	<div class="relative min-h-0 flex-1">
		<!-- The canvas stays mounted either way so its size observer and gesture handlers keep their bindings;
		     without WebGPU nothing ever draws into it and the server's JPEG sits on top. -->
		<canvas
			bind:this={canvas}
			class="block h-full w-full touch-none {fallback ? 'invisible' : ''}"
			onpointerdown={canvasDown}
			onpointermove={canvasMove}
			onpointerup={canvasUp}
			onpointercancel={canvasUp}
			oncontextmenu={(e) => e.preventDefault()}
		></canvas>
		{#if fallback}
			<div class="absolute inset-0 flex items-center justify-center p-2">
				{#if previewUrl}
					<img src={previewUrl} alt="" class="max-h-full max-w-full object-contain {previewBusy ? 'opacity-60' : ''}" />
				{/if}
			</div>
			{#if previewBusy}
				<p class="absolute inset-x-4 bottom-3 text-center text-xs text-fg-2">Rendering…</p>
			{/if}
		{/if}
		{#if markerPos && showAfPoint}
			<button
				type="button"
				class="absolute h-7 w-7 -translate-x-1/2 -translate-y-1/2 rounded-full border-2 border-[#fadb26] shadow-[0_0_0_1px_rgba(0,0,0,0.4)]"
				style="left: {markerPos.x}px; top: {markerPos.y}px;"
				aria-label="Autofocus point — tap to check in the loupe"
				onclick={jumpToAfPoint}
			></button>
		{/if}
		{#if status || error}
			<p class="absolute inset-x-4 top-[max(1rem,env(safe-area-inset-top))] text-center text-sm {error ? 'text-danger' : 'text-fg-2'}">{error || status}</p>
		{/if}
		{#if before}<span class="absolute top-[max(1rem,env(safe-area-inset-top))] left-4 rounded bg-ground/80 px-2 py-1 text-xs">{compare?.label ?? (developed ? 'As saved' : 'As shot')}</span>{/if}
		<div class="absolute top-[max(0.75rem,env(safe-area-inset-top))] right-3 grid justify-items-end gap-1.5">
			{#if !wide && showPhoneHistogram && !fallback}
				<Histogram data={histogram} {clipHigh} {clipLow} compact ontoggle={(w) => { if (w === 'high') clipHigh = !clipHigh; else clipLow = !clipLow; schedule(); }} />
			{/if}
			{#if !wide && !fallback}
				<button type="button" aria-pressed={peaking} class="rounded-full px-2.5 py-1 text-[0.6875rem] {peaking ? 'bg-fg text-ground' : 'bg-ground/80 text-fg-2'}" onclick={() => { peaking = !peaking; schedule(); }}>Focus</button>
				{#if meta?.info.afPoint}
					<button type="button" aria-pressed={showAfPoint} class="rounded-full px-2.5 py-1 text-[0.6875rem] {showAfPoint ? 'bg-fg text-ground' : 'bg-ground/80 text-fg-2'}" onclick={() => (showAfPoint = !showAfPoint)}>AF</button>
				{/if}
			{/if}
		</div>
	</div>

	{#if edit && meta}
		<!-- Phone: one tool at a time in a bottom sheet. Desktop: all sections in pipeline order. -->
		<aside class="flex max-h-[48dvh] shrink-0 flex-col bg-panel pb-[max(0.75rem,env(safe-area-inset-bottom))] lg:max-h-none lg:w-80 lg:pb-3">
			<header class="flex items-center justify-between gap-3 px-6 pt-3 pb-2 lg:px-4">
				<div class="min-w-0 text-xs text-fg-2">
					<p class="truncate text-sm text-fg">{meta.fileName}</p>
					<p class="truncate">{cameraLine(meta.info)}</p>
				</div>
				<div class="flex shrink-0 gap-2">
					<button type="button" class="grid size-9 place-items-center rounded bg-raised text-fg-2 disabled:opacity-40" aria-label="Undo" disabled={!canUndo || readOnly} onclick={() => restore(undo.undo())}>
						<svg viewBox="0 0 16 16" class="size-4" aria-hidden="true"><path d="M6 3 2 7l4 4M2.5 7H10a4 4 0 0 1 0 8H8" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" /></svg>
					</button>
					<button type="button" class="grid size-9 place-items-center rounded bg-raised text-fg-2 disabled:opacity-40" aria-label="Redo" disabled={!canRedo || readOnly} onclick={() => restore(undo.redo())}>
						<svg viewBox="0 0 16 16" class="size-4 -scale-x-100" aria-hidden="true"><path d="M6 3 2 7l4 4M2.5 7H10a4 4 0 0 1 0 8H8" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" /></svg>
					</button>
					<button type="button" class="h-9 rounded bg-raised px-3 text-sm" onclick={() => (exporting = true)}>Export</button>
					<button type="button" class="h-9 rounded bg-fg px-4 text-sm font-medium text-ground disabled:bg-raised disabled:text-muted" disabled={!dirty || saving || readOnly} onclick={() => commit()}>
						{saving ? 'Saving…' : dirty ? 'Save' : 'Saved'}
					</button>
				</div>
			</header>
			<div class="grid gap-2 px-6 lg:px-4">
				{#if fallback}
					<p class="rounded bg-raised p-2 text-xs">
						This browser has no WebGPU, so the server is developing the photo — same result, but it
						updates when you let go of a slider rather than as you drag. Safari 26 or a current
						Chromium edits live.
					</p>
				{/if}
				{#if readOnly}<p class="rounded bg-raised p-2 text-xs">A newer Sidecar edited this photo. Update Sidecar to change it.</p>{/if}
				{#if !readOnly && !writable}<p class="rounded bg-raised p-2 text-xs text-warning">Sidecar can't write in this photo's folder, so edits here can't be saved. Give the account Sidecar runs as write access to it.</p>{/if}
				{#if restoredDraft && dirty}<p class="rounded bg-raised p-2 text-xs">Restored unsaved changes from this device.</p>{/if}
				{#if forked}
					<p class="rounded bg-raised p-2 text-xs">
						This photo was also saved somewhere else since you opened it. Both versions are kept —
						<button type="button" class="underline" onclick={() => { section = 'history'; forked = false; }}>see History</button>.
					</p>
				{/if}
			</div>

			{#if wide && !fallback}
			<div class="px-4 pb-2">
				<Histogram data={histogram} {clipHigh} {clipLow} ontoggle={(w) => { if (w === 'high') clipHigh = !clipHigh; else clipLow = !clipLow; schedule(); }} />
				<div class="mt-1 flex items-center justify-between gap-2 text-[0.6875rem]">
					<div class="flex items-center gap-2">
						<button type="button" aria-pressed={peaking} class="rounded-full px-2 py-0.5 {peaking ? 'bg-fg text-ground' : 'bg-raised text-fg-2'}" onclick={() => { peaking = !peaking; schedule(); }}>Focus peaking</button>
						{#if meta?.info.afPoint}
							<button type="button" aria-pressed={showAfPoint} class="rounded-full px-2 py-0.5 {showAfPoint ? 'bg-fg text-ground' : 'bg-raised text-fg-2'}" onclick={() => (showAfPoint = !showAfPoint)}>AF point</button>
						{/if}
					</div>
					<a href="/settings" class="text-fg-2">Viewing and exporting in {SPACE_LABELS[displaySpace]}</a>
				</div>
			</div>

			{:else}
			<nav class="flex items-center gap-2 overflow-x-auto px-6 py-2" aria-label="Tools">
				{#each sections as s (s.id)}
					<button type="button" aria-pressed={section === s.id} class="h-8 shrink-0 rounded-full px-3 text-xs {section === s.id ? 'bg-fg text-ground' : 'bg-raised text-fg-2'}" aria-label={s.label} onclick={() => (section = s.id)}>{s.short}</button>
				{/each}
				<button type="button" aria-pressed={showPhoneHistogram} aria-label="Show histogram" class="ml-auto grid size-8 shrink-0 place-items-center rounded-full {showPhoneHistogram ? 'bg-fg text-ground' : 'bg-raised text-fg-2'}" onclick={() => (showPhoneHistogram = !showPhoneHistogram)}>
					<svg viewBox="0 0 16 16" class="size-4" aria-hidden="true"><path d="M1 14 L4 8 L7 10 L10 3 L13 9 L15 6 L15 14 Z" fill="currentColor" /></svg>
				</button>
			</nav>

			<div class="min-h-0 flex-1 overflow-y-auto overscroll-contain px-6 pt-1">
				{#if section === 'lens'}{@render lensSection()}{:else if section === 'wb'}{@render wbSection()}{:else if section === 'light'}{@render lightSection()}{:else if section === 'curve'}{@render curveSection()}{:else if section === 'color'}{@render colorSection()}{:else if section === 'masks'}{@render masksSection()}{:else if section === 'history'}{@render historySection()}{:else}{@render detailSection()}{/if}
			</div>

			{/if}
			{#if wide}
			<div class="min-h-0 flex-1 overflow-y-auto px-4">
				{#each sections as s (s.id)}
					<details class="border-t border-line py-2" open={s.id === 'wb' || s.id === 'light'} ontoggle={(e) => s.id === 'detail' && (detailOpen = (e.currentTarget as HTMLDetailsElement).open)}>
						<summary class="cursor-pointer py-1 text-xs font-medium">{s.label}</summary>
						<div class="pt-2">
							{#if s.id === 'lens'}{@render lensSection()}{:else if s.id === 'wb'}{@render wbSection()}{:else if s.id === 'light'}{@render lightSection()}{:else if s.id === 'curve'}{@render curveSection()}{:else if s.id === 'color'}{@render colorSection()}{:else if s.id === 'masks'}{@render masksSection()}{:else if s.id === 'history'}{@render historySection()}{:else if detailOpen}{@render detailSection()}{/if}
						</div>
					</details>
				{/each}
			</div>
			{/if}
		</aside>
	{/if}
</div>

{#if exporting}<ExportSheet assetKey={key} colorSpace={displaySpace} prepare={prepareExport} onclose={() => (exporting = false)} />{/if}
