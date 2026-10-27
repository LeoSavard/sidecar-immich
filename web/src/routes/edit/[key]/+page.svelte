<!-- SPDX-License-Identifier: Apache-2.0 -->
<script lang="ts">
	import { page } from '$app/state';
	import { tick, untrack } from 'svelte';
	import { DevelopPass, requestDevice } from '$lib/gpu/develop';
	import { CLIP_HIGHS, CLIP_LOWS, MASK_INVERTED, PEAKING, SHOW_MASK, ViewPass, type ViewGeometry, type Viewport } from '$lib/gpu/view';
	import { BRUSH, DEPTH, MaskLibrary, brushStroke, rangeUniform, type BrushSettings, type MaskSet } from '$lib/masks';
	import { DepthPass } from '$lib/gpu/depth';
	import { MaskPainter, simplify } from '$lib/gpu/brush';
	import { HistogramPass, type HistogramData } from '$lib/gpu/histogram';
	import { LensPass, correctedPoint, sourcePoint } from '$lib/gpu/lens';
	import { ColorPacker, type SlideProfile } from '$lib/color/pack';
	import { cachedProfile, loadProfile } from '$lib/film';
	import { api } from '$lib/api';
	import type { Reading } from '$lib/measure';
	import { saveSettings, settings, SPACE_LABELS } from '$lib/settings';
	import { EditSession, type EditState } from '$lib/state/edit-session.svelte';
	import { copyLook, pasteLook, readLookClipboard, type LookClipboard } from '$lib/state/look-clipboard';
	import Histogram from '$lib/ui/Histogram.svelte';
	import Loupe from '$lib/ui/Loupe.svelte';
	import Readout from '$lib/ui/Readout.svelte';
	import ExportSheet from '$lib/ui/ExportSheet.svelte';
	import HistoryPanel from '$lib/ui/HistoryPanel.svelte';
	import MasksPanel from '$lib/ui/MasksPanel.svelte';
	import CropOverlay from '$lib/ui/CropOverlay.svelte';
	import LensSection from '$lib/ui/sections/LensSection.svelte';
	import CropSection from '$lib/ui/sections/CropSection.svelte';
	import WhiteBalanceSection from '$lib/ui/sections/WhiteBalanceSection.svelte';
	import LightSection from '$lib/ui/sections/LightSection.svelte';
	import FilmSection from '$lib/ui/sections/FilmSection.svelte';
	import DepthSection from '$lib/ui/sections/DepthSection.svelte';
	import CurveSection from '$lib/ui/sections/CurveSection.svelte';
	import ColorSection from '$lib/ui/sections/ColorSection.svelte';
	import DetailSection from '$lib/ui/sections/DetailSection.svelte';
	import { NO_GEOMETRY, isNoGeometry, project, twist, untwist } from '$lib/state/edit-view';
	import type { CameraColor, Crop, DisplaySpace, EditParams, Frame, Geometry, MaskAdjustment, Meta } from '$lib/types';
	import { originalFile } from '$lib/local/originals';
	import { developOnDevice } from '$lib/local/develop';

	const SPACES: DisplaySpace[] = ['srgb', 'display-p3'];

	type Section = 'lens' | 'crop' | 'wb' | 'light' | 'film' | 'curve' | 'color' | 'depth' | 'masks' | 'detail' | 'history';

	const key = page.params.key!;
	let canvas: HTMLCanvasElement;
	let meta = $state<Meta | null>(null);
	let defaults: EditParams;
	let status = $state('Opening…');
	/** The white balance eyedropper is waiting for a tap on the photo. */
	let pickingWhite = $state(false);
	/** Tap-to-focus (the lens blur) is waiting for a tap on the photo. */
	let pickingFocus = $state(false);
	let error = $state('');
	/**
	 * The white balance the camera recorded, resolved once per photo. It is the Temperature slider's detent, so
	 * it must not follow the edit: it did (from each repack), and the detent then sat under the thumb, catching
	 * it every few pixels of a drag.
	 */
	let asShotWb = $state({ temperatureK: 5000, tint: 0 });
	/**
	 * The proxy was made on this device from its own copy of the original (docs/MOBILE.md), not downloaded: the
	 * editor is the same, and says so in case something it shows relies on the server's decode.
	 */
	let onDevice = $state(false);
	let onDeviceNoticeDismissed = $state(false);
	/**
	 * The installed app (the manifest's `display: standalone`), which is the only place the "developed on this
	 * device" note is shown: in a browser tab it was noise on every photo (the October interface review, item 11).
	 */
	let installedApp = $state(false);
	/** Device pixels per output pixel, as a percentage, or 0 at fit — the readout over the photo (item 2). */
	let zoomPercent = $state(0);
	/** The loupe over the photo, reached from the toggles beside Focus rather than three taps deep (item 8). */
	let loupeOver = $state(false);
	/**
	 * The colour model the proxy is in when it isn't the server's: a developed photo the browser decoded into
	 * Display P3. Everything that packs or samples the proxy reads `cameraColor()`.
	 */
	let deviceColor: CameraColor | null = null;
	const cameraColor = () => deviceColor ?? meta!.info.color;
	let lookClipboard = $state<LookClipboard | null>(readLookClipboard());
	let lut = $state<Float32Array | null>(null);
	/** The profile of the spectral colour stock this edit names, once the server has sent it. */
	let slideProfile: SlideProfile | undefined;
	const NO_TABLE = new Float32Array(0);
	/** The layers' curves as they render, a 4,096-entry row each: what the selected layer's curve editor draws. */
	let maskCurves = $state<Float32Array | null>(null);
	let histogram = $state<HistogramData | null>(null);
	/**
	 * What each side of the flip last measured (`$lib/measure`). Kept per side rather than for what is on
	 * screen, so flipping to the compared version and back leaves both renderings' numbers to read together.
	 */
	let measured = $state<{ edit: Reading | null; before: Reading | null }>({ edit: null, before: null });
	/** Where the probe reads, in the picture's own frame (0…1) — so it holds through a crop, a turn and the flip. */
	let probePoint = $state<[number, number] | null>(null);
	/** The probe is waiting for a tap on the photo. */
	let probing = $state(false);
	/** Readbacks in flight, per side: a slower one that resolves late must not overwrite a newer reading. */
	const probeSeq = { edit: 0, before: 0 };
	let clipHigh = $state(false);
	let clipLow = $state(false);
	let peaking = $state(false);
	let before = $state(false);
	let section = $state<Section>('light');
	let showPhoneHistogram = $state(true);
	/** The keyboard shortcuts, listed over the editor (`?`). */
	let showShortcuts = $state(false);
	const SHORTCUTS: [string, string][] = [
		['\\', 'Before and after: see it as shot (or as the version chosen in History), and back'],
		['Press and hold the photo', 'The same, while held'],
		['⌘Z / Ctrl+Z', 'Undo'],
		['⇧⌘Z / Ctrl+Shift+Z', 'Redo'],
		['E', 'Export'],
		['Z', 'Open or close Detail, with the loupe'],
		['R', 'Crop and straighten'],
		['J', 'Show clipped highlights and shadows'],
		['H', 'Hide or show the editing tools'],
		['Double click or double tap', 'Zoom to 100 % and back; in the loupe, 100 → 150 → 200 %'],
		['Esc', 'Stop picking (white balance, focus, measure)'],
		['?', 'This list']
	];
	/** What the worker can do (`GET /api/capabilities`); null until it says. */
	let capabilities = $state<{ models: boolean; gpu: boolean } | null>(null);
	/** The editing tools put away so the photo has the screen: only the section tabs stay. Kept per device. */
	let toolsHidden = $state(typeof localStorage !== 'undefined' && localStorage.getItem('sidecar.toolsHidden') === '1');
	let wide = $state(false);
	let exporting = $state(false);
	let loupeCenter = $state<[number, number]>([0.5, 0.5]);
	let detailOpen = $state(false);
	/** The crop section is open on the wide layout. */
	let cropOpen = $state(false);
	let fallbackImg = $state<HTMLImageElement | null>(null);
	/** A proxy is on the GPU, so the canvas shows the photo. */
	let proxyLoaded = $state(false);
	/** Moves after each frame drawn in the crop tool, so its overlay follows the canvas's layout. */
	let layoutTick = $state(0);
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

	/** The edit, its saving and its versions (`$lib/state/edit-session`); the page draws what follows a change. */
	const session = new EditSession(key, {
		redraw: () => schedule(true),
		settled: () => {
			if (fallback) renderPreview();
		},
		compared: () => {
			if (!before) return;
			schedule(true);
			if (fallback) renderPreview();
		},
		error: (message) => (error = message)
	});
	const edit = $derived(session.edit);
	const readOnly = $derived(session.readOnly);
	const compare = $derived(session.compare);
	const changed = () => {
		session.changed();
		offerBeforeHint();
	};

	// ---------- the before/after hint: how to compare, said once and quietly (owner's call: no button for it)
	const BEFORE_HINT_KEY = 'sidecar.beforeHintSeen';
	let beforeHint = $state(false);
	let beforeHintDone = (() => {
		try {
			return localStorage.getItem(BEFORE_HINT_KEY) === '1';
		} catch {
			return true;
		}
	})();
	/** After the first change on this device, for a few seconds; never again once seen or once compared. */
	function offerBeforeHint() {
		if (beforeHintDone) return;
		beforeHintDone = true;
		beforeHint = true;
		setTimeout(() => (beforeHint = false), 5000);
		try {
			localStorage.setItem(BEFORE_HINT_KEY, '1');
		} catch {
			// Shown again next time; that is all.
		}
	}
	const committed = () => session.committed();
	/** The crop tool is in use: the canvas shows the whole straightened picture, with the crop's frame over it. */
	const cropping = $derived(!readOnly && !toolsHidden && (wide ? cropOpen : section === 'crop'));
	const compareEdit = () => compare?.edit ?? defaults;

	let develop: DevelopPass;
	let view: ViewPass;
	let hist: HistogramPass;
	let lensPass: LensPass;
	/** The lens pass's outputs at proxy size — the photo and its focus mask — and what they were made from. */
	let corrected: GPUTexture | undefined;
	/** What the lens pass made for develop: `corrected`, or `undefined` for the proxy itself. */
	let lensOut: GPUTexture | undefined;
	let depthPass: DepthPass;
	/** What the depth pass last ran on and with, so it only runs again when one of them changes. */
	let depthMadeFrom: { key: string; base: GPUTexture; depth: GPUTexture } | undefined;
	/** The refined depth map and what it was made from: the photo and the model's map, neither of them a slider. */
	let refinedFrom: { base: GPUTexture; depth: GPUTexture; out: GPUTexture } | undefined;
	let correctedMask: GPUTexture | undefined;
	let lensMadeFrom = '';
	/** The masks the models made for this photo, fetched when a mask is first needed (`$lib/masks`). */
	let maskLibrary: MaskLibrary | undefined;
	let maskSet = $state<MaskSet | null>(null);
	/** The layer being edited, and whether its mask is shown over the photo. */
	let maskSelected = $state<number | null>(null);
	let maskShown = $state(false);
	let brush = $state<BrushSettings>({ mode: 'off', size: 50, feather: 50, flow: 100 });
	/** Painting happens on the photo while a layer is selected and the brush is on. */
	const painting = $derived(!fallback && !readOnly && !toolsHidden && !cropping && brush.mode !== 'off' && maskSelected !== null && (wide || section === 'masks'));
	/** The camera's own rendering beside the edit (a RAW's embedded JPEG); false once it turned out to have none. */
	let showCamera = $state(false);
	let cameraMissing = $state(false);
	const cameraOffered = $derived(!developed && !cameraMissing && !fallback);
	/** The photo and the camera's rendering side by side — not while cropping or painting, which need the canvas whole. */
	const sideBySide = $derived(showCamera && cameraOffered && !cropping && !painting);
	/** Masks lens-corrected as the picture is, and the correction each was made with. */
	const correctedMasks = new Map<string, { madeFrom: string; texture: GPUTexture }>();
	let painter: MaskPainter;
	/**
	 * Layers with brush strokes, painted at the proxy's size by layer slot: what they were painted from, a
	 * version that moves with every live stroke, and the lens-corrected copy develop reads.
	 */
	const layerTextures = new Map<number, { key: string; version: number; painted: GPUTexture; corrected?: GPUTexture; correctedFrom: string }>();
	/**
	 * The stroke being painted: its finger, layer, settings, its points so far (fractions of the photo), and
	 * those not drawn yet — pointer events only collect them; each frame draws what arrived since the last.
	 */
	let stroke: { pointerId: number; layer: number; settings: ReturnType<typeof brushStroke>; points: [number, number][]; drawn: number } | null = null;
	/**
	 * While a stroke is painted, the photo developed without its layer and with it at full weight: each frame
	 * blends the two by the mask where the brush went, instead of developing the whole photo again.
	 */
	let preview: { without: GPUTexture; within: GPUTexture; invert: boolean } | null = null;
	/** The histogram missed a develop while the brush was on; it catches up once the brush is off. */
	let histogramStale = false;
	/** The brush outline on screen, CSS pixels. */
	let brushCursor = $state<{ x: number; y: number; r: number } | null>(null);
	/** Geometry frames, by geometry and picture size. */
	const frames = new Map<string, Frame>();
	/** The uniforms the corrected proxy was made with: they also move markers set in sensor geometry. */
	let lensUniform: Uint8Array | undefined;
	let packer: ColorPacker;
	let packerReady = $state(false);
	/**
	 * This edit renders by rules the pipeline has moved on from (`adopt_current_rendering`). References only
	 * change through the action below, so this is worked out when the photo opens and after it is taken,
	 * never on every keystroke.
	 */
	let behindRendering = $state(false);
	/**
	 * The references this edit had before it was brought up to date, so "keep the old rendering" can put
	 * them back. Set only when the port happened on its own, under the person's first change.
	 */
	let oldRendering = $state<ReturnType<typeof referencesOf> | null>(null);
	/**
	 * They asked to keep the old rendering on this photo. Held for as long as it is open and no longer: an
	 * edit that keeps old references is the thing this is trying to stop, so the next session asks again.
	 */
	let keepOldRendering = $state(false);
	/** What Highlights/Shadows alone would do to an identity curve — the curve editor's "ghost" line, drawn
	 * behind the real one so a slider's effect stays visible even once a manual curve has its own shape. */
	const ghostLut = $derived.by(() => (edit ? (packer?.toneCurveLut([], edit.tone.highlights ?? 0, edit.tone.shadows ?? 0) ?? null) : null));
	let display: DisplaySpace = 'srgb';
	let developDirty = true;
	let frame = 0;
	let histStride = 1;

	function schedule(dev = false) {
		developDirty ||= dev;
		if (!frame) frame = requestAnimationFrame(draw);
	}

	/**
	 * View and export in another space from now on: the canvas is configured again and the develop uniforms
	 * repacked for it. Saved as the setting, so the next photo and the Settings page agree.
	 */
	async function setOutputSpace(next: DisplaySpace) {
		if (next === display) return;
		display = next;
		displaySpace = next;
		view?.setDisplayP3(next === 'display-p3');
		schedule(true);
		if (fallback) renderPreview();
		try {
			await saveSettings({ ...(await settings()), outputSpace: next });
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
	}

	function setToolsHidden(hidden: boolean, open?: Section) {
		toolsHidden = hidden;
		localStorage.setItem('sidecar.toolsHidden', hidden ? '1' : '0');
		if (hidden) pickingWhite = pickingFocus = probing = false;
		if (!open) return;
		section = open;
		// On the desktop every section is a <details>: open the one asked for and bring it into view.
		if (wide)
			tick().then(() => {
				const d = document.getElementById(`section-${open}`) as HTMLDetailsElement | null;
				if (d) {
					// The section and the group it sits in.
					for (let at: HTMLDetailsElement | null = d; at; at = at.parentElement?.closest('details') ?? null) at.open = true;
					d.scrollIntoView({ block: 'start' });
				}
			});
	}

	/** Resolve the edit through the colour packer — wasm, so it works with or without a GPU. */
	function repack() {
		const source = before ? compareEdit() : $state.snapshot(edit!);
		// A spectral colour stock needs its profile, which only the server can measure; until it arrives the
		// edit renders as the per-channel film approximation, and the fetch marks the develop dirty again.
		const stock = source.film?.stock;
		// `schedule`, not the flag alone: the profile lands in a promise, with no frame on the way.
		if (stock) loadProfile(stock, () => schedule(true));
		slideProfile = stock ? cachedProfile(stock) : undefined;
		const packed = packer.pack(source, cameraColor(), meta!.info.sourceKind ?? 'raw', display, slideProfile);
		if (!before) {
			lut = packed.lut;
			maskCurves = packed.maskCurves;
		}
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
		if (stroke && preview) drawStroke();
		if (developDirty) {
			correctLens();
			depthEffects();
			const packed = repack();
			develop.setUniforms(packed.uniform);
			develop.setToneLut(packed.lut);
			develop.setMono(packed.monoLut);
			develop.setSlide(packed.slideExposure, slideProfile?.response ?? NO_TABLE);
			develop.setMasks(packed.masks, ((before ? compareEdit() : edit).masks ?? []).map((m, i) => layerTexture(i, m)), packed.maskCurves);
			develop.render();
			developDirty = false;
			histogramStale = true;
		}
		// Not while the brush is on: the mask is what's being judged then, and the readback costs a phone frames.
		if (histogramStale && !painting) {
			const out = outputGeometry(before ? compareEdit().geometry : edit.geometry);
			// Which rendering this is: the readback resolves later, by which time the flip may have moved on.
			const side = before ? 'before' : 'edit';
			hist.run(develop.output, histStride, (h) => { histogram = h; recordClipping(side, h); }, out ? { toPicture: out.toPicture, size: out.size } : undefined);
			// The copy is queued here, so it reads the picture as it stands now whatever renders next.
			if (probePoint) readProbe(side, probePoint);
			histogramStale = false;
		}
		const showing = maskSelected !== null && (maskShown || brush.mode !== 'off') && !before ? edit.masks?.[maskSelected] : undefined;
		const overlay = showing && layerTexture(maskSelected!, showing);
		view.useOverlay(overlay);
		const maskFlags = overlay ? SHOW_MASK | (showing.invert ? MASK_INVERTED : 0) : 0;
		const shown = displayFrame();
		const geometry: ViewGeometry | undefined = shown ? { toPicture: shown.frame.toPicture, size: shown.frame.size } : undefined;
		view.render(develop.output, canvas, viewport, (clipHigh ? CLIP_HIGHS : 0) | (clipLow ? CLIP_LOWS : 0) | (peaking ? PEAKING : 0) | maskFlags, geometry);
		markerPos = cropping ? null : imageToScreen(afPoint());
		// The zoom readout: device pixels per output pixel, which is what the double tap's 100 % means. 0 at fit,
		// where there is nothing to say and nothing to go back to.
		zoomPercent = viewport.zoom > 1.001 ? Math.round(imageRect().fit * viewport.zoom * 100) : 0;
		if (cropping) layoutTick++;
		placeCamera();
	}

	// ---------- the camera's own rendering beside the edit, at the edit's zoom and position
	let cameraPane = $state<HTMLDivElement | null>(null);
	let cameraImg = $state<HTMLImageElement | null>(null);
	let cameraTransform = $state('');

	/**
	 * Put the camera's JPEG where each of its points sits in the edit's view. The JPEG is the whole picture, as the
	 * proxy is, so a point of the picture (the edit's output frame mapped back through crop, straighten and turns) is
	 * a point of the JPEG; three canvas points give the affine map that turns the JPEG's pixels into the pane's, which
	 * has the canvas's size. Zoom, pan, a crop and a straighten all follow; keystone and lens correction are
	 * linearised at the centre, close enough to compare by.
	 */
	function placeCamera() {
		const img = cameraImg;
		if (!sideBySide || !img?.naturalWidth || !develop?.output) return;
		const r = imageRect();
		const toPicture = displayFrame()?.frame.toPicture ?? [[1, 0, 0], [0, 1, 0], [0, 0, 1]];
		const at = (x: number, y: number) => project(toPicture, [(x - r.x) / r.w, (y - r.y) / r.h]);
		const [cx, cy, d] = [canvas.width / 2, canvas.height / 2, Math.max(16, canvas.width / 4)];
		const [s0, sx, sy] = [at(cx, cy), at(cx + d, cy), at(cx, cy + d)];
		// d(picture)/d(canvas pixel), and its inverse.
		const [a, b, c, e] = [(sx[0] - s0[0]) / d, (sy[0] - s0[0]) / d, (sx[1] - s0[1]) / d, (sy[1] - s0[1]) / d];
		const det = a * e - b * c;
		if (!Number.isFinite(det) || Math.abs(det) < 1e-12) return;
		const inv = [e / det, -b / det, -c / det, a / det];
		// JPEG pixel m → picture m / (W, H) → canvas pixel p0 + inv · (m / (W, H) − s0) → pane CSS pixel p / dpr.
		const [W, H, k] = [img.naturalWidth, img.naturalHeight, 1 / devicePixelRatio];
		const m = [(inv[0] * k) / W, (inv[2] * k) / W, (inv[1] * k) / H, (inv[3] * k) / H];
		const tx = (cx - (inv[0] * s0[0] + inv[1] * s0[1])) * k;
		const ty = (cy - (inv[2] * s0[0] + inv[3] * s0[1])) * k;
		cameraTransform = `matrix(${m.join(',')},${tx},${ty})`;
	}

	// The pane takes the canvas's wheel as it takes its pointers; a wheel listener must be able to cancel.
	$effect(() => {
		const pane = cameraPane;
		if (!pane) return;
		pane.addEventListener('wheel', wheel, { passive: false });
		return () => pane.removeEventListener('wheel', wheel);
	});

	// ---------- geometry: crop, straighten, keystone, turns (sidecar_color::geometry, through wasm)

	/** The picture's size: the proxy's, or without WebGPU the photo's as the metadata gives it (oriented). */
	function pictureSize(): [number, number] | null {
		const proxy = develop?.proxy;
		if (proxy) return [proxy.width, proxy.height];
		return meta ? [meta.info.width, meta.info.height] : null;
	}

	function frameOf(g: Geometry, size: [number, number]): Frame {
		const key = JSON.stringify([g, size]);
		let f = frames.get(key);
		if (!f) {
			if (frames.size > 64) frames.clear();
			f = packer.frame(g, size);
			frames.set(key, f);
		}
		return f;
	}

	/** An edit's output frame, or `null` when it has no geometry: the output is then the picture. */
	function outputGeometry(g: Geometry | undefined): Frame | null {
		const size = pictureSize();
		return size && g && !isNoGeometry(g) ? frameOf(g, size) : null;
	}

	/** What the crop tool shows: everything the straightened picture covers, and the crop with it. */
	function cropView(g: Geometry, size: [number, number]): Geometry {
		const b = frameOf(g, size).bounds;
		const c = g.crop;
		return { ...g, crop: { left: Math.min(b.left, c.left), top: Math.min(b.top, c.top), right: Math.max(b.right, c.right), bottom: Math.max(b.bottom, c.bottom) } };
	}

	/** The geometry the canvas shows, and its frame: the edit's, or in the crop tool the whole picture. */
	function displayFrame(): { geometry: Geometry; frame: Frame } | null {
		const size = pictureSize();
		if (!size || !edit) return null;
		const g = before ? compareEdit().geometry : edit.geometry;
		if (cropping && !before) {
			const shown = cropView(g ?? NO_GEOMETRY, size);
			return { geometry: shown, frame: frameOf(shown, size) };
		}
		const frame = outputGeometry(g);
		return frame ? { geometry: g!, frame } : null;
	}

	/** The crop tool's frame on screen, in the shown output's coordinates. */
	function cropOnScreen(): [number, number, number, number] | null {
		const size = pictureSize();
		if (!size || !edit) return null;
		const g = edit.geometry ?? NO_GEOMETRY;
		const all = cropView(g, size).crop;
		const c = g.crop;
		const pre = (x: number, y: number): [number, number] => [(x - all.left) / (all.right - all.left), (y - all.top) / (all.bottom - all.top)];
		const corners = [twist(g, pre(c.left, c.top)), twist(g, pre(c.right, c.bottom))];
		return [Math.min(corners[0][0], corners[1][0]), Math.min(corners[0][1], corners[1][1]), Math.max(corners[0][0], corners[1][0]), Math.max(corners[0][1], corners[1][1])];
	}

	/** A crop drawn on screen (the shown output's coordinates) back in crop coordinates. */
	function cropFromScreen(r: [number, number, number, number]): Crop {
		const size = pictureSize()!;
		const g = edit!.geometry ?? NO_GEOMETRY;
		const all = cropView(g, size).crop;
		const back = (s: number, t: number) => {
			const [p, q] = untwist(g, [s, t]);
			return [all.left + p * (all.right - all.left), all.top + q * (all.bottom - all.top)];
		};
		const [a, b] = [back(r[0], r[1]), back(r[2], r[3])];
		return { left: Math.min(a[0], b[0]), top: Math.min(a[1], b[1]), right: Math.max(a[0], b[0]), bottom: Math.max(a[1], b[1]) };
	}

	/** Inside the convex outline the picture makes after straightening and keystone. */
	function insideQuad(quad: [number, number][], [x, y]: [number, number]): boolean {
		let sign = 0;
		for (let i = 0; i < 4; i++) {
			const [a, b] = [quad[i], quad[(i + 1) % 4]];
			const cross = (b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0]);
			if (Math.abs(cross) < 1e-9) continue;
			if (!sign) sign = Math.sign(cross);
			else if (Math.sign(cross) !== sign) return false;
		}
		return true;
	}

	function cropFits(r: [number, number, number, number]): boolean {
		const size = pictureSize();
		if (!size || !edit) return false;
		const c = cropFromScreen(r);
		const quad = frameOf(edit.geometry ?? NO_GEOMETRY, size).quad;
		return [[c.left, c.top], [c.right, c.top], [c.right, c.bottom], [c.left, c.bottom]].every((p) => insideQuad(quad, p as [number, number]));
	}

	/** Change the geometry, fitting the crop to it, and drop it altogether once it does nothing. */
	function setGeometry(patch: Partial<Geometry>) {
		const size = pictureSize();
		if (!edit || !size) return;
		const was: Geometry = $state.snapshot(edit.geometry) ?? NO_GEOMETRY;
		const next = packer.fit({ ...was, ...patch }, was, size);
		if (isNoGeometry(next) && !next.ratio) delete edit.geometry;
		else edit.geometry = next;
		changed();
	}

	function setCrop(r: [number, number, number, number]) {
		if (!edit) return;
		edit.geometry = { ...($state.snapshot(edit.geometry) ?? NO_GEOMETRY), crop: cropFromScreen(r) };
		changed();
	}

	/** A quarter turn. A locked ratio turns with the photo, so the same part of it stays cropped. */
	function turn(by: 1 | -1) {
		const g = edit?.geometry ?? NO_GEOMETRY;
		setGeometry({ rotate: (g.rotate + by + 4) % 4, ratio: g.ratio ? 1 / g.ratio : 0 });
		committed();
	}

	/** The photo's own proportions in the output's orientation. */
	function originalRatio(): number {
		const size = pictureSize() ?? [3, 2];
		return (edit?.geometry?.rotate ?? 0) % 2 ? size[1] / size[0] : size[0] / size[1];
	}

	function resetGeometry() {
		if (!edit) return;
		delete edit.geometry;
		changed();
		committed();
	}

	$effect(() => {
		// The brush going off is when a paused histogram catches up.
		if (!painting) untrack(() => schedule());
	});

	$effect(() => {
		// The crop tool shows the whole picture at fit: no zoom or pan to fight its handles. Only entering and
		// leaving it re-runs this; what the redraw reads is not this effect's business.
		if (cropping) viewport = { zoom: 1, panX: 0, panY: 0 };
		untrack(() => {
			schedule();
			if (fallback && edit) renderPreview();
		});
	});

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
			lensOut = undefined;
			view.useFocusMask(undefined);
			return;
		}
		if (corrected?.width !== proxy.width || corrected?.height !== proxy.height) {
			corrected?.destroy();
			corrected = lensPass.target(proxy, 'lens corrected');
		}
		lensPass.run(proxy, corrected, lensUniform);
		lensOut = corrected;
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
	 * Haze and lens blur by distance over what the lens pass made, for develop to read — when the edit shown has
	 * either, and the depth mask and the photo's airlight have arrived (asking for them if not). Cheap to call on
	 * every develop: the pass only runs when the lens result, the settings or the depth mask changed.
	 */
	function depthEffects() {
		const shown = before ? compareEdit() : edit!;
		const base = lensOut ?? develop.proxy;
		const wanted = (shown.haze ?? 0) !== 0 || (shown.lensBlur?.amount ?? 0) > 0;
		if (wanted && !maskSet) loadMaskSet();
		const airlight = maskSet?.airlight;
		const coarse = wanted && airlight ? maskTexture(DEPTH) : undefined;
		// The map with the photo's own edges, so a blur's edge follows hair rather than the model's 518².
		// It depends on the photo and the map alone, so it is kept until one of them changes.
		let depth = coarse;
		if (coarse && base) {
			if (refinedFrom?.base !== base || refinedFrom?.depth !== coarse) {
				refinedFrom = { base, depth: coarse, out: depthPass.refine(base, coarse, [base.width, base.height]) };
			}
			depth = refinedFrom.out;
		}
		const uniform = depth && base ? packer.packDepth($state.snapshot(shown), airlight!, [base.width, base.height]) : undefined;
		if (!uniform || !depth || !base) {
			depthMadeFrom = undefined;
			develop.setInput(lensOut);
			return;
		}
		const key = `${lensMadeFrom}|${uniform.join(',')}`;
		if (depthMadeFrom?.key === key && depthMadeFrom.base === base && depthMadeFrom.depth === depth) return;
		depthMadeFrom = { key, base, depth };
		develop.setInput(depthPass.run(base, depth, uniform));
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

	/** Lens uniforms for a mask of `size`: the shown correction without vignetting; none when it is off. */
	function maskLens(size: [number, number]): { uniform: Uint8Array; madeFrom: string } | undefined {
		const profile = meta?.info.lensProfile;
		const lens = before ? compareEdit().lens : edit?.lens;
		if (!profile || !lens) return undefined;
		const uniform = packer.packLens(profile, { ...$state.snapshot(lens), vignetting: 0 }, sensorAspect(meta!.info), size, meta!.info.orientationFlip ?? 0);
		return uniform && { uniform, madeFrom: JSON.stringify([lens, profile.id]) };
	}

	/**
	 * A layer's mask in the frame being developed: the model's mask as it is, or — once it has brush strokes,
	 * or while one is being painted — painted at the proxy's size over it and lens-corrected. `undefined`
	 * while something it needs is loading; that redraws when it arrives.
	 */
	function layerTexture(i: number, m: MaskAdjustment): GPUTexture | undefined {
		if (!m.strokes?.length && !m.range && !m.edge && stroke?.layer !== i) return m.mask === BRUSH ? undefined : maskTexture(m.mask);
		const entry = layerEntry(i, m);
		if (!entry) return undefined;
		const size: [number, number] = [entry.painted.width, entry.painted.height];
		const lens = maskLens(size);
		if (!lens) return entry.painted;
		const from = `${lens.madeFrom}:${entry.key}:${entry.version}`;
		if (entry.correctedFrom !== from) {
			entry.corrected ??= painter.mask(size, `layer ${i} corrected`);
			painter.correct(entry.painted, entry.corrected, lens.uniform);
			entry.correctedFrom = from;
		}
		return entry.corrected;
	}

	/** Layer `i` painted from its model mask and strokes, up to date unless a stroke is being drawn on it. */
	function layerEntry(i: number, m: MaskAdjustment) {
		const proxy = develop?.proxy;
		if (!proxy || !painter) return undefined;
		let base: GPUTexture | undefined;
		if (m.mask !== BRUSH) {
			base = maskLibrary?.ready(m.mask);
			if (!base) {
				maskLibrary?.load(m.mask).then(() => schedule(true), () => {});
				return undefined;
			}
		}
		let entry = layerTextures.get(i);
		if (!entry || entry.painted.width !== proxy.width || entry.painted.height !== proxy.height) {
			entry?.painted.destroy();
			entry?.corrected?.destroy();
			entry = { key: '', version: 0, painted: painter.mask([proxy.width, proxy.height], `layer ${i}`), correctedFrom: '' };
			layerTextures.set(i, entry);
		}
		const key = JSON.stringify([m.mask, m.range ?? null, m.edge ?? null, m.strokes ?? []]);
		if (entry.key !== key && stroke?.layer !== i) {
			painter.paint(entry.painted, base, $state.snapshot(m.strokes) ?? [], rangeUniform(m.range));
			// The edge last, over the range and the strokes. (While a stroke is drawn it shows without the edge.)
			const plan = m.edge ? packer.edgePlan($state.snapshot(m.edge), [proxy.width, proxy.height]) : null;
			if (plan) painter.edge(entry.painted, plan);
			entry.key = key;
		}
		return entry;
	}

	function forgetLayers() {
		for (const e of layerTextures.values()) {
			e.painted.destroy();
			e.corrected?.destroy();
		}
		layerTextures.clear();
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
		// Depth on its own is a gradient from far to near; a layer on it starts as the nearer half.
		if (name === DEPTH) m.range = { from: 50, to: 100, feather: 15 };
		edit.masks = [...(edit.masks ?? []), m];
		maskSelected = edit.masks.length - 1;
		maskShown = true;
		// A brush mask is nothing until it is painted.
		brush.mode = name === BRUSH ? 'paint' : 'off';
		changed();
		committed();
	}

	function removeMask(index: number) {
		if (!edit?.masks) return;
		const rest = edit.masks.filter((_, i) => i !== index);
		// No masks is no field at all: the edit then serializes exactly as one made before masks existed.
		if (rest.length) edit.masks = rest;
		else delete edit.masks;
		maskSelected = null;
		brush.mode = 'off';
		// Slots moved: what was painted for one now belongs to another.
		forgetLayers();
		changed();
		committed();
	}

	/** A field patched to `undefined` is removed, so the layer is stored without it. */
	function setMask(index: number, patch: Partial<MaskAdjustment>) {
		const layer = edit?.masks?.[index] as Record<string, unknown> | undefined;
		if (!layer) return;
		for (const [k, v] of Object.entries(patch)) {
			if (v === undefined) delete layer[k];
			else layer[k] = v;
		}
		changed();
	}

	// ---------- the brush

	/** A canvas pixel → the photo before lens correction (0…1), where strokes are stored. */
	function screenToSource(p: { x: number; y: number }): [number, number] | null {
		const proxy = develop?.proxy;
		if (!proxy || !develop.output) return null;
		const r = imageRect();
		const o: [number, number] = [(p.x - r.x) / r.w, (p.y - r.y) / r.h];
		const uv = project(displayFrame()?.frame.toPicture ?? [[1, 0, 0], [0, 1, 0], [0, 0, 1]], o);
		const q = sourcePoint(lensUniform, proxy.width, proxy.height, { x: uv[0], y: uv[1] });
		return [q.x, q.y];
	}

	/**
	 * The white balance eyedropper: the camera RGB around the tapped point (lens, straighten and crop undone,
	 * clipped pixels left out) becomes the neutral the temperature and tint are solved for.
	 */
	async function pickWhite(e: PointerEvent) {
		pickingWhite = false;
		const proxy = develop?.proxy;
		const p = screenToSource(px(e));
		if (!p || !proxy || !packer || !meta || p.some((v) => v < 0 || v > 1)) return;
		// What is under the tap: a square about 9 CSS pixels across at any zoom, in proxy pixels (at least 3 × 3
		// to average noise, at most 33 × 33 so a thin grey line still counts).
		const outW = displayFrame()?.frame.size[0] ?? develop.output!.width;
		const perCanvasPx = outW / imageRect().w;
		const radius = Math.min(16, Math.max(1, Math.round(4 * devicePixelRatio * perCanvasPx)));
		const rgb = await develop.sampleSource(p, radius);
		if (!rgb || rgb.some((v) => v <= 0)) {
			status = rgb ? 'That spot has no colour to balance. Pick a grey with some light in it.' : 'That spot is clipped. Pick a grey that isn\'t.';
			setTimeout(() => (status = ''), 3000);
			return;
		}
		const wb = packer.whiteFromNeutral(cameraColor(), rgb);
		const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, Math.round(v)));
		update((ed) => {
			ed.whiteBalance = { mode: 'custom', temperatureK: clamp(wb.temperatureK, 2000, 12000), tint: clamp(wb.tint, -150, 150) };
		});
		committed();
	}

	/**
	 * Tap to focus: the depth mask's value under the tap (a 5 × 5 average) becomes the lens blur's focus
	 * distance. The mask is in the same frame `screenToSource` maps to: oriented, before lens correction.
	 */
	async function pickFocus(e: PointerEvent) {
		pickingFocus = false;
		const p = screenToSource(px(e));
		if (!p || !maskLibrary || p.some((v) => v < 0 || v > 1)) return;
		const depth = await maskLibrary.values(DEPTH);
		const [cx, cy] = [Math.round(p[0] * (depth.width - 1)), Math.round(p[1] * (depth.height - 1))];
		let [sum, n] = [0, 0];
		for (let y = Math.max(0, cy - 2); y <= Math.min(depth.height - 1, cy + 2); y++)
			for (let x = Math.max(0, cx - 2); x <= Math.min(depth.width - 1, cx + 2); x++) {
				sum += depth.data[y * depth.width + x];
				n++;
			}
		const focus = Math.round((sum / n / 255) * 100);
		update((ed) => {
			ed.lensBlur = { ...(ed.lensBlur ?? { amount: 50, focus, range: 10 }), focus };
		});
		committed();
	}

	// ---------- measuring

	/** What the readout calls each side of the flip. */
	function sideLabel(side: 'edit' | 'before'): string {
		if (side === 'edit') return 'Edit';
		return compare?.label ?? (developed ? 'As saved' : 'As shot');
	}

	/** A canvas pixel → the picture's own frame (0…1), the frame the developed texture is in. */
	function screenToPicture(point: { x: number; y: number }): [number, number] | null {
		if (!develop?.output) return null;
		const r = imageRect();
		const o: [number, number] = [(point.x - r.x) / r.w, (point.y - r.y) / r.h];
		if (o.some((v) => v < 0 || v > 1)) return null;
		return project(displayFrame()?.frame.toPicture ?? [[1, 0, 0], [0, 1, 0], [0, 0, 1]], o);
	}

	/** The probe reads a square about this far out, in proxy pixels: enough to average a proxy's noise. */
	const PROBE_RADIUS = 4;

	function recordClipping(side: 'edit' | 'before', h: HistogramData) {
		const kept = measured[side];
		measured = {
			...measured,
			[side]: {
				label: sideLabel(side),
				clipping: { atWhite: h.clippedHigh, atBlack: h.clippedLow, counted: h.total },
				probe: probePoint ? (kept?.probe ?? null) : null
			}
		};
	}

	async function readProbe(side: 'edit' | 'before', at: [number, number]) {
		const mine = ++probeSeq[side];
		const probe = await develop.sampleDeveloped(at, PROBE_RADIUS);
		if (mine !== probeSeq[side]) return;
		const kept = measured[side];
		measured = { ...measured, [side]: { label: sideLabel(side), clipping: kept?.clipping ?? null, probe } };
	}

	/** The probe: the tapped place becomes the point both renderings are read at. */
	function pickProbe(e: PointerEvent) {
		probing = false;
		const p = screenToPicture(px(e));
		if (!p) return;
		probePoint = p;
		readProbe(before ? 'before' : 'edit', p);
	}

	function clearProbe() {
		probePoint = null;
		probeSeq.edit++;
		probeSeq.before++;
		measured = {
			edit: measured.edit && { ...measured.edit, probe: null },
			before: measured.before && { ...measured.before, probe: null }
		};
	}

	/** The rendering on screen first, then the other side's once it has been seen. */
	const readings = $derived(
		[before ? measured.before : measured.edit, before ? measured.edit : measured.before].filter((r): r is Reading => !!r)
	);

	// ---------- today's rendering, for an edit made before it

	function checkRendering() {
		if (!packer || !edit || !meta) return;
		// Having asked to keep the old rendering, they are not asked again while the photo is open — the
		// references are back where they were, so the question would otherwise return the moment it was
		// answered.
		if (keepOldRendering) {
			behindRendering = false;
			return;
		}
		try {
			behindRendering = packer.adoptRendering($state.snapshot(edit), developed).changed;
		} catch {
			behindRendering = false;
		}
	}

	/** The references as they stand, for putting back: every field `adopt_current_rendering` sets. One left out
	 *  is never taken up, so the edit stays behind and the offer comes back the moment it was taken. */
	function referencesOf(e: EditParams) {
		return { processVersion: e.processVersion, baselineEv: e.exposure.baselineEv, whiteEv: e.tone.whiteEv, whiteZeroEv: e.tone.whiteZeroEv, shoulderOrder: e.tone.shoulderOrder, highlightBleach: e.tone.highlightBleach, highlightReachEv: e.tone.highlightReachEv };
	}

	function setReferences(e: EditParams, r: ReturnType<typeof referencesOf>) {
		e.processVersion = r.processVersion;
		e.exposure = { ...e.exposure, baselineEv: r.baselineEv };
		e.tone = { ...e.tone, whiteEv: r.whiteEv, whiteZeroEv: r.whiteZeroEv, shoulderOrder: r.shoulderOrder, highlightBleach: r.highlightBleach, highlightReachEv: r.highlightReachEv };
	}

	/**
	 * Take up today's rendering, in the same change as whatever the person is doing. An ordinary edit, so
	 * the picture moves at once, Undo takes it back, and saving files it as a version with the old one
	 * still behind it.
	 */
	function adoptRendering() {
		if (!packer || !edit || !meta) return;
		const next = packer.adoptRendering($state.snapshot(edit), developed).edit;
		update((e) => setReferences(e, referencesOf(next)));
		committed();
		behindRendering = false;
		oldRendering = null;
		status = 'Rendering brought up to date. Undo puts it back.';
		setTimeout(() => (status = ''), 4000);
	}

	/**
	 * Editing an old edit brings it up to date first, unless the person has said to keep the old rendering.
	 * Letting it stay behind is what lets a rendering nobody can reach any more spread into versions saved
	 * today — the edit inherits the references it was opened with, so one saved this morning can still be
	 * rendering by rules from three versions ago. The port is offered back for as long as the photo is open.
	 */
	function portBeforeEditing() {
		if (!behindRendering || readOnly || keepOldRendering || !packer || !edit || !meta) return;
		const had = referencesOf($state.snapshot(edit));
		const next = packer.adoptRendering($state.snapshot(edit), developed).edit;
		setReferences(edit, referencesOf(next));
		behindRendering = false;
		oldRendering = had;
	}

	/** Put the references back and stop offering: this photo keeps the rendering it had, for this session. */
	function keepOld() {
		if (!edit || !oldRendering) return;
		update((e) => setReferences(e, oldRendering!));
		committed();
		keepOldRendering = true;
		behindRendering = false;
		oldRendering = null;
	}

	/** The brush's radius in proxy pixels, and on screen in CSS pixels. */
	function brushRadius() {
		const proxy = develop?.proxy;
		if (!proxy || !develop.output) return { px: 0, css: 0 };
		const px = brushStroke(brush).radius * Math.max(proxy.width, proxy.height);
		const r = imageRect();
		const outW = displayFrame()?.frame.size[0] ?? develop.output.width;
		return { px, css: (px * (r.w / outW)) / devicePixelRatio };
	}

	function startStroke(e: PointerEvent) {
		const i = maskSelected;
		const m = i !== null ? edit?.masks?.[i] : undefined;
		const p = screenToSource(px(e));
		if (i === null || !m || !p || !edit?.masks || !develop.output) return;
		// Whatever is waiting to be developed first, so both renders below start from the edit as it is.
		if (developDirty) draw();
		// The layer's mask painted from its strokes so far — before the stroke is live, which holds repainting.
		const entry = layerEntry(i, m);
		if (!entry) return;
		const settings = brushStroke(brush);
		stroke = { pointerId: e.pointerId, layer: i, settings, points: [p], drawn: 0 };
		// And lens-corrected, as it renders now: the stroke draws onto both.
		layerTexture(i, m);
		// Without the layer (its amount at 0), and with it at full weight everywhere: an empty mask, inverted.
		const textures = edit.masks.map((l, j) => layerTexture(j, l));
		const packed = repack();
		const layerTone = 32 + i * 192 + 16;
		const without = packed.masks.slice();
		new DataView(without.buffer).setFloat32(layerTone + 8, 0, true);
		const within = packed.masks.slice();
		new DataView(within.buffer).setFloat32(layerTone + 12, 1, true);
		preview = { without: develop.variantTarget('without the layer'), within: develop.variantTarget('with the layer'), invert: m.invert };
		develop.renderVariant(preview.without, without, textures);
		develop.renderVariant(preview.within, within, textures.map((t, j) => (j === i ? undefined : t)));
		painter.begin(entry.painted, { ...settings, points: [] });
		schedule();
	}

	function extendStroke(e: PointerEvent) {
		const proxy = develop?.proxy;
		if (!stroke || !proxy) return;
		// Spaced at a fraction of the radius: smooth curves, without a point per pixel moved.
		const spacing = Math.max(1, 0.15 * stroke.settings.radius * Math.max(proxy.width, proxy.height));
		let last = stroke.points[stroke.points.length - 1];
		for (const ev of e.getCoalescedEvents?.() ?? [e]) {
			const p = screenToSource(px(ev));
			if (!p || Math.hypot((p[0] - last[0]) * proxy.width, (p[1] - last[1]) * proxy.height) < spacing) continue;
			stroke.points.push(p);
			last = p;
		}
		if (stroke.points.length > stroke.drawn) schedule();
	}

	/**
	 * Draw what the stroke gained since the last frame: onto the mask, through the lens correction and into
	 * the picture — each only over the area the stroke has covered.
	 */
	function drawStroke() {
		const s = stroke!;
		const entry = layerTextures.get(s.layer);
		if (!entry || s.points.length === s.drawn) return;
		const fresh = s.points.slice(s.drawn);
		painter.extend(fresh, s.drawn ? s.points[s.drawn - 1] : undefined);
		s.drawn = s.points.length;
		const box = painter.liveBox;
		if (!box) return;
		const [w, h] = [entry.painted.width, entry.painted.height];
		let mask = entry.painted;
		let area = box;
		const lens = maskLens([w, h]);
		if (lens && entry.corrected) {
			// The box in the corrected picture: where its edges' points land, with a margin for the curve between.
			const xs = [box[0], (box[0] + box[2]) / 2, box[2]];
			const ys = [box[1], (box[1] + box[3]) / 2, box[3]];
			const pts = xs.flatMap((x) => ys.map((y) => correctedPoint(lensUniform, w, h, { x: x / w, y: y / h })));
			const margin = 4 + 0.005 * Math.max(w, h);
			area = [Math.min(...pts.map((p) => p.x)) * w - margin, Math.min(...pts.map((p) => p.y)) * h - margin, Math.max(...pts.map((p) => p.x)) * w + margin, Math.max(...pts.map((p) => p.y)) * h + margin];
			painter.correct(entry.painted, entry.corrected, lens.uniform, area);
			mask = entry.corrected;
		}
		painter.blend(preview!.without, preview!.within, mask, preview!.invert, develop.output!, area);
	}

	function endPreview() {
		preview?.without.destroy();
		preview?.within.destroy();
		preview = null;
	}

	/** The stroke into the edit, simplified and rounded: a few dozen points, not one per pointer event. */
	function endStroke() {
		const proxy = develop?.proxy;
		const s = stroke;
		if (s && s.drawn < s.points.length && preview) drawStroke();
		stroke = null;
		painter.end();
		endPreview();
		const m = s && edit?.masks?.[s.layer];
		if (!s || !m || !proxy) return;
		const radiusPx = s.settings.radius * Math.max(proxy.width, proxy.height);
		const round = (v: number) => Math.round(v * 1e4) / 1e4;
		const inPx = s.points.map(([x, y]) => [x * proxy.width, y * proxy.height] as [number, number]);
		const points = simplify(inPx, Math.max(0.5, radiusPx * 0.1)).map(([x, y]) => [round(x / proxy.width), round(y / proxy.height)] as [number, number]);
		const saved = { erase: s.settings.erase, radius: round(s.settings.radius), feather: round(s.settings.feather), flow: round(s.settings.flow), points };
		setMask(s.layer, { strokes: [...($state.snapshot(m.strokes) ?? []), saved] });
		committed();
	}

	/** A second finger arrived: that was a pinch, not a stroke. */
	function cancelStroke() {
		if (!stroke) return;
		const entry = layerTextures.get(stroke.layer);
		painter.cancel();
		stroke = null;
		endPreview();
		if (entry) entry.version++;
		schedule(true);
	}

	function moveBrushCursor(e: PointerEvent) {
		brushCursor = painting ? { x: e.offsetX, y: e.offsetY, r: brushRadius().css } : null;
	}

	/** The AF point where it sits in the picture on screen — moved by the lens correction when one is on. */
	function afPoint() {
		const p = meta?.info.afPoint;
		const proxy = develop?.proxy;
		return p && proxy ? correctedPoint(lensUniform, proxy.width, proxy.height, p) : p;
	}

	/**
	 * Where what is shown sits on the canvas, in canvas pixels, at the current zoom and pan — the edit's output,
	 * or the whole picture in the crop tool; `fit` is its scale at zoom 1. Without WebGPU, the server's JPEG.
	 */
	function imageRect() {
		if (fallback) {
			const img = fallbackImg?.getBoundingClientRect();
			const box = canvas.getBoundingClientRect();
			if (!img) return { x: 0, y: 0, w: 1, h: 1, fit: 1 };
			return { x: (img.left - box.left) * devicePixelRatio, y: (img.top - box.top) * devicePixelRatio, w: img.width * devicePixelRatio, h: img.height * devicePixelRatio, fit: 1 };
		}
		const [ow, oh] = displayFrame()?.frame.size ?? [develop.output!.width, develop.output!.height];
		const fit = Math.min(canvas.width / ow, canvas.height / oh);
		const w = ow * fit * viewport.zoom;
		const h = oh * fit * viewport.zoom;
		return { x: (canvas.width - w) / 2 + viewport.panX, y: (canvas.height - h) / 2 + viewport.panY, w, h, fit };
	}

	/** `imageRect` in CSS pixels, for overlays. The markup passes `layoutTick` so it asks again each frame. */
	function overlayRect(_frame: number) {
		const r = imageRect();
		return { x: r.x / devicePixelRatio, y: r.y / devicePixelRatio, w: r.w / devicePixelRatio, h: r.h / devicePixelRatio };
	}

	/** A point of the picture (0…1, lens-corrected) → CSS pixels over the canvas, or null off-screen/absent. */
	function imageToScreen(point: { x: number; y: number } | null | undefined): { x: number; y: number } | null {
		if (!point || !develop?.output) return null;
		const shown = displayFrame();
		const [u, v] = shown ? project(shown.frame.fromPicture, [point.x, point.y]) : [point.x, point.y];
		if (u < 0 || v < 0 || u > 1 || v > 1) return null;
		const r = imageRect();
		const px = r.x + u * r.w;
		const py = r.y + v * r.h;
		if (px < 0 || py < 0 || px > canvas.width || py > canvas.height) return null;
		return { x: px / devicePixelRatio, y: py / devicePixelRatio };
	}

	/** A point of the picture → where it is in the edit's output, which is what the loupe is centred in. */
	function toOutput(p: { x: number; y: number }): [number, number] {
		const out = outputGeometry(edit?.geometry);
		return out ? project(out.fromPicture, [p.x, p.y]) : [p.x, p.y];
	}

	/** Move the loupe to the AF point and make sure it's visible. */
	function jumpToAfPoint() {
		const point = afPoint();
		if (!point) return;
		loupeCenter = toOutput(point);
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
				const shownEdit: EditParams = before ? compareEdit() : $state.snapshot(edit);
				// The crop tool shows the whole straightened picture, the crop drawn over it here.
				const size = pictureSize();
				if (cropping && !before && size) shownEdit.geometry = cropView(shownEdit.geometry ?? NO_GEOMETRY, size);
				const body = JSON.stringify({
					edit: shownEdit,
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

	// Masks are asked for when a tool needs them — the section is opened, or the edit has some — not with
	// every photo: the first ask computes them.
	$effect(() => {
		if (section === 'masks' || section === 'depth' || wide || (edit?.masks?.length ?? 0) > 0) loadMaskSet();
	});

	// The eyedropper belongs to the White balance section: leaving it on a phone puts the eyedropper away.
	$effect(() => {
		if (!wide && section !== 'wb') pickingWhite = false;
		if (!wide && section !== 'depth') pickingFocus = false;
		if (!wide) probing = false;
	});

	// The history loads when it's first looked at, not with every photo.
	$effect(() => {
		if ((section === 'history' || wide) && edit && !session.versions) session.loadVersions();
	});

	// Whether this edit renders by today's rules. The references below are the whole of what the answer
	// depends on, so reading them here is what makes this re-run — when the photo opens, when a version is
	// opened, and after an undo puts an old one back — rather than on every change to anything.
	$effect(() => {
		void edit?.processVersion;
		void edit?.exposure.baselineEv;
		void edit?.tone.whiteEv;
		void edit?.tone.whiteZeroEv;
		void edit?.tone.shoulderOrder;
		void edit?.tone.highlightBleach;
		void edit?.tone.highlightReachEv;
		if (packerReady && edit && meta) checkRendering();
	});

	async function loadProxy(longEdge: number) {
		const [proxyResp, focusResp] = await Promise.all([api(`/api/assets/${key}/proxy?long_edge=${longEdge}`), api(`/api/assets/${key}/focus?edge=${longEdge}`)]);
		const w = Number(proxyResp.headers.get('x-width'));
		const h = Number(proxyResp.headers.get('x-height'));
		develop.setSource(w, h, await proxyResp.arrayBuffer());
		proxyLoaded = true;
		await view.loadFocusMask(await focusResp.arrayBuffer());
		schedule(true);
	}

	/**
	 * On a phone that still holds the original — it was imported from here — develop the proxy here instead of
	 * downloading it (a 2,560-pixel proxy is some 35 MB over the network, and the server may not decode every
	 * format the phone does). False, and the server's proxy is used as before, wherever it can't: no copy on the
	 * device, a format the device can't decode, or `?server` in the address. `?device` asks for it on a desktop.
	 */
	async function developHere(device: GPUDevice, edge: number, phone: boolean): Promise<boolean> {
		const q = page.url.searchParams;
		if (q.has('server') || !(phone || q.has('device'))) return false;
		const original = await originalFile(key).catch(() => null);
		if (!original) return false;
		status = 'Developing on this device…';
		const local = await developOnDevice(device, original, packer, edge).catch((e) => `it failed (${e instanceof Error ? e.message : e})`);
		if (typeof local === 'string') {
			console.info(`Not developed on this device, so the server develops it: ${local}.`);
			return false;
		}
		if (local.camera) {
			deviceColor = local.camera;
			const asShot = packer.pack(defaults, cameraColor(), meta!.info.sourceKind ?? 'raw', display);
			asShotWb = { temperatureK: asShot.temperatureK, tint: asShot.tint };
		}
		develop.setSource(local.proxy.width, local.proxy.height, local.proxy.f16);
		proxyLoaded = true;
		onDevice = true;
		status = '';
		// For tests and for looking: `?inspect` leaves the proxy and its colour model on the window.
		if (q.has('inspect')) Object.assign(window, { __sidecarInspect: { proxy: local.proxy, camera: local.rawColor ?? cameraColor(), info: local.raw, space: local.space } });
		schedule(true);
		// Focus peaking's mask is the server's to compute; it arrives when it does, and nothing waits for it.
		api(`/api/assets/${key}/focus?edge=${edge}`)
			.then((r) => r.arrayBuffer())
			.then((b) => view.loadFocusMask(b))
			.catch(() => {});
		return true;
	}

	async function start() {
		api('/api/capabilities')
			.then((r) => r.json())
			.then((c) => {
				capabilities = c;
				// A section the server can't run, left open from another photo, gives way to Light.
				if (!offered(section)) section = 'light';
			})
			.catch(() => {});
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
			packerReady = true;
			defaults = packer.defaultEdit();
			const asShot = packer.pack(defaults, meta!.info.color, meta!.info.sourceKind ?? 'raw', display);
			asShotWb = { temperatureK: asShot.temperatureK, tint: asShot.tint };
			await session.begin(state, meta!.info.lensProfile);
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
			[develop, view, hist, lensPass, painter] = await Promise.all([
				DevelopPass.create(device),
				ViewPass.create(device, canvas, display === 'display-p3'),
				HistogramPass.create(device),
				LensPass.create(device),
				MaskPainter.create(device)
			]);
			depthPass = await DepthPass.create(device);
			maskLibrary = new MaskLibrary(device, key);
			if (section === 'masks' || wide || (edit?.masks?.length ?? 0) > 0) loadMaskSet();
			device.lost.then((info) => (error = `The GPU was reset (${info.reason}). Reload to continue — your edits are kept as a draft.`));
			const phone = matchMedia('(pointer: coarse)').matches;
			histStride = phone ? 2 : 1;
			const wanted = Math.round(Math.max(innerWidth, innerHeight) * devicePixelRatio * 1.25);
			const edge = Math.min(wanted, device.limits.maxTextureDimension2D, phone ? 2560 : 4096);
			if (await developHere(device, edge, phone)) return;
			status = 'Loading preview…';
			await loadProxy(768);
			status = '';
			await loadProxy(edge);
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
	}

	// ---------- the phone's panel: its height, set by holding its handle and then dragging
	const SHEET_KEY = 'sidecar.sheetVh';
	const SHEET_MIN = 22;
	const SHEET_MAX = 85;
	let sheet = $state<{ vh: number | null; armed: boolean }>({ vh: null, armed: false });
	try {
		const v = Number(localStorage.getItem(SHEET_KEY));
		if (v >= SHEET_MIN && v <= SHEET_MAX) sheet.vh = v;
	} catch {
		// No storage: the panel keeps its usual height.
	}
	let sheetHold: ReturnType<typeof setTimeout> | undefined;

	// ---------- the desktop sidebar: which sections are open, as each person last left them
	const OPEN_KEY = 'sidecar.openSections';
	const openSections = (() => {
		try {
			const v = JSON.parse(localStorage.getItem(OPEN_KEY) ?? 'null');
			if (Array.isArray(v)) return new Set<Section>(v);
		} catch {
			// Nothing remembered: the defaults.
		}
		return new Set<Section>(['wb', 'light']);
	})();
	function rememberOpen(id: Section, open: boolean) {
		// Crop and Detail open themselves from shortcuts and taps; only a person's own choice is remembered.
		if (id === 'crop') return;
		if (open) openSections.add(id);
		else openSections.delete(id);
		try {
			localStorage.setItem(OPEN_KEY, JSON.stringify([...openSections]));
		} catch {
			// Remembered for this page only.
		}
	}
	let sheetFrom = { y: 0, id: -1 };

	function sheetDown(e: PointerEvent) {
		(e.currentTarget as Element).setPointerCapture(e.pointerId);
		sheetFrom = { y: e.clientY, id: e.pointerId };
		clearTimeout(sheetHold);
		sheetHold = setTimeout(() => {
			sheet.armed = true;
			navigator.vibrate?.(10);
		}, 350);
	}

	function sheetMove(e: PointerEvent) {
		if (e.pointerId !== sheetFrom.id) return;
		// Moving before the hold has armed it is a swipe, not a resize: let it go.
		if (!sheet.armed) {
			if (Math.abs(e.clientY - sheetFrom.y) > 8) clearTimeout(sheetHold);
			return;
		}
		const vh = ((innerHeight - e.clientY) / innerHeight) * 100;
		sheet.vh = Math.round(Math.min(SHEET_MAX, Math.max(SHEET_MIN, vh)));
	}

	function sheetUp(e: PointerEvent) {
		if (e.pointerId !== sheetFrom.id) return;
		clearTimeout(sheetHold);
		sheetFrom.id = -1;
		if (!sheet.armed) return;
		sheet.armed = false;
		try {
			if (sheet.vh) localStorage.setItem(SHEET_KEY, String(sheet.vh));
		} catch {
			// The height lasts as long as the page.
		}
	}

	// ---------- gestures on the photo: pinch zoom, pan, double-tap 100 %, press-and-hold for as shot
	const pointers = new Map<number, { x: number; y: number }>();
	let pinch: { dist: number; zoom: number; mid: { x: number; y: number }; panX: number; panY: number } | null = null;
	let holdTimer: ReturnType<typeof setTimeout> | undefined;
	let downAt = { x: 0, y: 0, t: 0 };
	let lastTapAt = 0;

	/**
	 * A pointer in device pixels of the view it is over: the canvas, or the camera's pane beside it, which is the
	 * canvas's size and shows the same picture at the same place, so it has the canvas's coordinates and the same
	 * gestures (`data-view` marks both).
	 */
	const px = (e: PointerEvent | WheelEvent) => {
		const host = (e.target as Element | null)?.closest?.('[data-view]') ?? canvas;
		const r = host.getBoundingClientRect();
		return { x: (e.clientX - r.left) * devicePixelRatio, y: (e.clientY - r.top) * devicePixelRatio };
	};

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
		// Comparing is the hint learnt: it isn't needed again.
		beforeHint = false;
		beforeHintDone = true;
		before = on;
		schedule(true);
		if (fallback) renderPreview();
	}

	function canvasDown(e: PointerEvent) {
		(e.currentTarget as Element).setPointerCapture(e.pointerId);
		pointers.set(e.pointerId, px(e));
		downAt = { ...px(e), t: performance.now() };
		clearTimeout(holdTimer);
		// With the brush on, one finger paints; a second one turns it into a pinch.
		if (painting && pointers.size === 1) startStroke(e);
		else if (pointers.size === 1 && !pickingWhite && !pickingFocus && !probing) holdTimer = setTimeout(() => setBefore(true), 450);
		if (pointers.size === 2) {
			cancelStroke();
			clearTimeout(holdTimer);
			const [a, b] = [...pointers.values()];
			pinch = { dist: Math.hypot(a.x - b.x, a.y - b.y), zoom: viewport.zoom, mid: { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 }, panX: viewport.panX, panY: viewport.panY };
		}
	}

	function canvasMove(e: PointerEvent) {
		moveBrushCursor(e);
		const prev = pointers.get(e.pointerId);
		if (!prev) return;
		const cur = px(e);
		pointers.set(e.pointerId, cur);
		if (stroke?.pointerId === e.pointerId) {
			extendStroke(e);
			return;
		}
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
		if (stroke?.pointerId === e.pointerId) {
			if (e.type === 'pointercancel') cancelStroke();
			else endStroke();
			return;
		}
		if (before) {
			setBefore(false);
			return;
		}
		const now = performance.now();
		const still = Math.hypot(px(e).x - downAt.x, px(e).y - downAt.y) < 8 * devicePixelRatio && now - downAt.t < 300;
		if (still && pointers.size === 0) {
			if (pickingWhite && !fallback && develop?.output) {
				pickWhite(e);
				return;
			}
			if (pickingFocus && !fallback && develop?.output) {
				pickFocus(e);
				return;
			}
			if (probing && !fallback && develop?.output) {
				pickProbe(e);
				return;
			}
			if (fallback || !develop?.output || cropping || painting) return;
			const r = imageRect();
			// What is shown is the edit's output: the loupe is centred in it.
			const u = (px(e).x - r.x) / r.w;
			const v = (px(e).y - r.y) / r.h;
			if (u >= 0 && u <= 1 && v >= 0 && v <= 1 && (section === 'detail' || detailOpen || loupeOver)) loupeCenter = [u, v];
			if (now - lastTapAt < 320) {
				zoomAt(px(e), viewport.zoom > 1.01 ? 1 : Math.max(2, 1 / r.fit));
				lastTapAt = 0;
			} else lastTapAt = now;
		}
	}

	function wheel(e: WheelEvent) {
		e.preventDefault();
		zoomAt(px(e), viewport.zoom * Math.exp(-e.deltaY * 0.002));
	}

	function keydown(e: KeyboardEvent) {
		if ((e.target as HTMLElement)?.tagName === 'INPUT') return;
		if (e.key === '?') {
			showShortcuts = !showShortcuts;
			return;
		}
		if (e.key === 'Escape' && showShortcuts) {
			showShortcuts = false;
			return;
		}
		if (e.key === 'Escape' && (pickingWhite || pickingFocus || probing)) {
			pickingWhite = pickingFocus = probing = false;
			return;
		}
		if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'z') {
			e.preventDefault();
			if (e.shiftKey) session.redo();
			else session.undo();
		} else if (e.key === '\\') setBefore(!before);
		else if (e.key.toLowerCase() === 'h' && !e.metaKey && !e.ctrlKey) setToolsHidden(!toolsHidden);
		else if (e.key.toLowerCase() === 'e' && !e.metaKey && !e.ctrlKey) exporting = true;
		else if (e.key.toLowerCase() === 'z' && !e.metaKey && !e.ctrlKey) {
			section = 'detail';
			detailOpen = !detailOpen;
		}
		else if (e.key.toLowerCase() === 'r' && !e.metaKey && !e.ctrlKey) {
			if (wide) cropOpen = !cropOpen;
			else section = section === 'crop' ? 'light' : 'crop';
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
		// Installed, so launched from /m rather than opened in a tab. iOS Safari answered `navigator.standalone`
		// before it understood the media query, and still does from a home-screen icon.
		const app = matchMedia('(display-mode: standalone)');
		const onApp = () => (installedApp = app.matches || (navigator as { standalone?: boolean }).standalone === true);
		onApp();
		app.addEventListener('change', onApp);
		const ro = new ResizeObserver(() => {
			canvas.width = Math.round(canvas.clientWidth * devicePixelRatio);
			canvas.height = Math.round(canvas.clientHeight * devicePixelRatio);
			schedule();
		});
		ro.observe(canvas);
		const onHide = () => document.visibilityState === 'hidden' && session.commit(true);
		document.addEventListener('visibilitychange', onHide);
		canvas.addEventListener('wheel', wheel, { passive: false });
		start();
		return () => {
			ro.disconnect();
			mq.removeEventListener('change', onWide);
			app.removeEventListener('change', onApp);
			document.removeEventListener('visibilitychange', onHide);
			canvas.removeEventListener('wheel', wheel);
		};
	});

	// ---------- edit helpers
	/** What sections call to change the edit (`$lib/ui/sections`): apply it, then redraw and mark it unsaved. */
	function update(mutate: (e: EditParams) => void) {
		if (!edit) return;
		portBeforeEditing();
		mutate(edit);
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
	const allSections: { id: Section; label: string; short: string }[] = [
		{ id: 'wb', label: 'White balance', short: 'WB' },
		{ id: 'film', label: 'Film', short: 'Film' },
		// Not "Colour": that is the group it sits in, and a section named after its group reads as the same thing.
		{ id: 'color', label: 'Mixer', short: 'Mixer' },
		{ id: 'light', label: 'Light', short: 'Light' },
		{ id: 'curve', label: 'Curve', short: 'Curve' },
		{ id: 'crop', label: 'Crop and straighten', short: 'Crop' },
		{ id: 'depth', label: 'Depth', short: 'Depth' },
		{ id: 'masks', label: 'Masks', short: 'Masks' },
		{ id: 'detail', label: 'Detail', short: 'Detail' },
		{ id: 'lens', label: 'Lens', short: 'Lens' },
		{ id: 'history', label: 'History', short: 'History' }
	];
	const sectionById = (id: Section) => allSections.find((x) => x.id === id)!;
	/** The tools as the panel groups them: a group with one section is that section. */
	const allGroups: { id: string; label: string; short: string; children: Section[] }[] = [
		{ id: 'colour', label: 'Colour', short: 'Colour', children: ['wb', 'film', 'color'] },
		{ id: 'light', label: 'Light', short: 'Light', children: ['light', 'curve'] },
		{ id: 'crop', label: 'Crop and straighten', short: 'Crop', children: ['crop'] },
		{ id: 'processing', label: 'Processing', short: 'Processing', children: ['depth', 'masks', 'detail', 'lens'] },
		{ id: 'history', label: 'History', short: 'History', children: ['history'] }
	];
	/**
	 * A tool this server can't run is left out rather than shown with a sentence saying so (owner's call,
	 * 2026-10-26): Depth without the models, Detail's loupe without a GPU on the worker. Until the worker has said
	 * (`GET /api/capabilities`), everything is shown.
	 */
	const offered = (id: Section) => !capabilities || ((id !== 'depth' || capabilities.models) && (id !== 'detail' || capabilities.gpu));
	const sections = $derived(allSections.filter((x) => offered(x.id)));
	const groups = $derived(allGroups.map((g) => ({ ...g, children: g.children.filter(offered) })).filter((g) => g.children.length > 0));
	const groupOf = (id: Section) => groups.find((g) => g.children.includes(id)) ?? groups[0];
</script>

<svelte:head><title>{meta?.fileName ?? 'Sidecar'}</title></svelte:head>
<svelte:window onkeydown={keydown} />

{#snippet lensSection()}
	{#if edit && meta}<LensSection {edit} onchange={update} oncommit={committed} info={meta.info} {developed} />{/if}
{/snippet}

{#snippet cropSection()}
	{#if edit && meta}<CropSection {edit} {readOnly} originalRatio={originalRatio()} ongeometry={setGeometry} onturn={turn} onreset={resetGeometry} oncommit={committed} />{/if}
{/snippet}

{#snippet wbSection()}
	{#if edit}<WhiteBalanceSection {edit} onchange={update} oncommit={committed} {developed} asShot={asShotWb} picking={fallback || readOnly ? undefined : pickingWhite} onpick={(on) => (pickingWhite = on)} />{/if}
{/snippet}

{#snippet lightSection()}
	{#if edit}<LightSection {edit} onchange={update} oncommit={committed} {developed} />{/if}
{/snippet}

{#snippet filmSection()}
	{#if edit}<FilmSection {edit} onchange={update} oncommit={committed} {developed} />{/if}
{/snippet}

{#snippet curveSection()}
	{#if edit}<CurveSection {edit} onchange={update} oncommit={committed} {lut} {ghostLut} />{/if}
{/snippet}

{#snippet detailSection()}
	{#if edit}<DetailSection {edit} onchange={update} oncommit={committed} assetKey={key} {loupeCenter} colorSpace={displaySpace} />{/if}
{/snippet}

{#snippet masksSection()}
	{#if edit}
		<MasksPanel
			masks={edit.masks ?? []}
			set={maskSet}
			selected={maskSelected}
			shown={maskShown}
			{brush}
			canPaint={!fallback}
			curveLut={maskCurves && maskSelected !== null ? maskCurves.subarray(maskSelected * 4096, (maskSelected + 1) * 4096) : null}
			disabled={readOnly}
			onadd={addMask}
			onremove={removeMask}
			onselect={(i) => { maskSelected = maskSelected === i ? null : i; if (maskSelected === null) brush.mode = 'off'; schedule(); }}
			onchange={setMask}
			oncommit={committed}
			onshow={(on) => { maskShown = on; schedule(); }}
			onbrush={(patch) => { Object.assign(brush, patch); schedule(); }}
			/>
	{/if}
{/snippet}

{#snippet historySection()}
	<!-- The colour sections as a look: copied here, pasted onto another photo (`$lib/state/look-clipboard`). -->
	<div class="mb-2 flex flex-wrap items-center gap-1.5 text-xs">
		<span class="text-fg-2">Look</span>
		<button type="button" class="rounded-full bg-raised px-2.5 py-1 text-fg disabled:opacity-40" disabled={!edit} title="Copy this photo's white balance, light, curve, colour and detail" onclick={() => { if (edit && meta) { copyLook($state.snapshot(edit), meta.fileName); lookClipboard = readLookClipboard(); } }}>Copy look</button>
		<button type="button" class="rounded-full bg-raised px-2.5 py-1 text-fg disabled:opacity-40" disabled={!edit || !lookClipboard || readOnly} onclick={() => { if (edit && pasteLook(edit)) { changed(); committed(); } }}>Paste look</button>
		{#if lookClipboard}<span class="min-w-0 truncate text-fg-2">from {lookClipboard.from}</span>{/if}
	</div>
	{#if session.versions}
		<HistoryPanel history={session.versions} compareId={compare?.id ?? null} busy={session.versionsBusy || session.saving} onopen={(id) => session.openVersion(id)} oncompare={(id) => session.setCompare(id)} onpick={(id, section) => session.pickFrom(id, section)} onlabel={(id, label) => session.labelVersion(id, label)} />

	{:else}
		<p class="text-xs text-fg-2">Loading history…</p>
	{/if}
{/snippet}

{#snippet depthSection()}
	{#if edit}<DepthSection {edit} onchange={update} oncommit={committed} available={fallback ? true : maskSet ? maskSet.available && !!maskSet.airlight : null} picking={fallback || readOnly ? undefined : pickingFocus} onpick={(on) => (pickingFocus = on)} />{/if}
{/snippet}

{#snippet sectionDetails(s: { id: Section; label: string }, top: boolean)}
	<details
		id="section-{s.id}"
		class="{top ? 'border-t border-line' : 'border-t border-line/50'} py-2"
		open={openSections.has(s.id) || (s.id === 'crop' && cropOpen)}
		ontoggle={(e) => {
			const open = (e.currentTarget as HTMLDetailsElement).open;
			if (s.id === 'detail') detailOpen = open;
			if (s.id === 'crop') cropOpen = open;
			rememberOpen(s.id, open);
		}}
	>
		<summary class="cursor-pointer py-1 text-xs font-medium">{s.label}</summary>
		<div class="pt-2">
			{#if s.id === 'lens'}{@render lensSection()}{:else if s.id === 'crop'}{@render cropSection()}{:else if s.id === 'wb'}{@render wbSection()}{:else if s.id === 'light'}{@render lightSection()}{:else if s.id === 'film'}{@render filmSection()}{:else if s.id === 'curve'}{@render curveSection()}{:else if s.id === 'color'}{@render colorSection()}{:else if s.id === 'depth'}{@render depthSection()}{:else if s.id === 'masks'}{@render masksSection()}{:else if s.id === 'history'}{@render historySection()}{:else if detailOpen}{@render detailSection()}{/if}
		</div>
	</details>
{/snippet}

{#snippet colorSection()}
	{#if edit}<ColorSection {edit} onchange={update} oncommit={committed} />{/if}
{/snippet}

<div class="flex h-dvh flex-col overflow-hidden bg-ground text-fg select-none [-webkit-touch-callout:none] lg:flex-row">
	<div class="relative min-h-0 flex-1">
		<!-- The canvas stays mounted either way so its size observer and gesture handlers keep their bindings;
		     without WebGPU nothing ever draws into it and the server's JPEG sits on top. -->
		<canvas
			bind:this={canvas}
			data-view
			title={wide ? 'Hold, or press \\, to see it as shot · double click for 100 %' : undefined}
			class="block touch-none {sideBySide ? (wide ? 'h-full w-1/2' : 'h-1/2 w-full') : 'h-full w-full'} {fallback ? 'invisible' : ''} {pickingWhite || pickingFocus || probing ? 'cursor-crosshair' : ''}"
			onpointerdown={canvasDown}
			onpointermove={canvasMove}
			onpointerup={canvasUp}
			onpointercancel={canvasUp}
			onpointerleave={() => (brushCursor = null)}
			oncontextmenu={(e) => e.preventDefault()}
		></canvas>
		{#if sideBySide}
			<!-- After the canvas, so the canvas keeps its corner and everything drawn over it keeps its place. -->
			<!-- The same gestures as the canvas, in the canvas's coordinates: either side zooms and pans both. -->
			<div
				bind:this={cameraPane}
				data-view
				role="img"
				aria-label="The camera's own rendering, at the edit's zoom and position"
				class="absolute touch-none overflow-hidden border-line bg-ground {wide ? 'inset-y-0 right-0 w-1/2 border-l' : 'inset-x-0 bottom-0 h-1/2 border-t'} {pickingWhite || pickingFocus || probing ? 'cursor-crosshair' : ''}"
				onpointerdown={canvasDown}
				onpointermove={canvasMove}
				onpointerup={canvasUp}
				onpointercancel={canvasUp}
				oncontextmenu={(e) => e.preventDefault()}
			>
				<img
					bind:this={cameraImg}
					src="/api/assets/{key}/camera"
					alt=""
					draggable="false"
					class="pointer-events-none absolute top-0 left-0 max-w-none origin-top-left select-none {cameraTransform ? '' : 'invisible'}"
					style="transform: {cameraTransform}"
					onload={() => schedule()}
					onerror={() => {
						cameraMissing = true;
						showCamera = false;
					}}
				/>
				<span class="pointer-events-none absolute top-2 left-2 rounded bg-ground/80 px-2 py-1 text-xs text-fg-2">Camera</span>
			</div>
		{/if}
		{#if fallback}
			<div class="absolute inset-0 flex items-center justify-center p-2">
				{#if previewUrl}
					<img bind:this={fallbackImg} src={previewUrl} alt="" class="max-h-full max-w-full object-contain {previewBusy ? 'opacity-60' : ''}" onload={() => schedule()} />
				{/if}
			</div>
			{#if previewBusy}
				<p class="absolute inset-x-4 bottom-3 text-center text-xs text-fg-2">Rendering…</p>
			{/if}
		{/if}
		{#if cropping && edit && (fallback ? previewUrl : proxyLoaded)}
			{@const r = overlayRect(layoutTick)}
			{@const c = cropOnScreen()}
			{#if c}
				<CropOverlay
					rect={r}
					crop={c}
					ratio={edit.geometry?.ratio ?? 0}
					accept={cropFits}
					onchange={setCrop}
					oncommit={committed}
				/>
			{/if}
		{/if}
		{#if brushCursor && painting}
			<div class="pointer-events-none absolute rounded-full border border-white/80 shadow-[0_0_0_1px_rgba(0,0,0,0.5)]" style="left: {brushCursor.x - brushCursor.r}px; top: {brushCursor.y - brushCursor.r}px; width: {brushCursor.r * 2}px; height: {brushCursor.r * 2}px;"></div>
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
		{#if beforeHint && !status && !error}
			<p class="pointer-events-none absolute inset-x-4 top-[max(3.25rem,calc(env(safe-area-inset-top)+2.5rem))] mx-auto w-fit rounded bg-ground/80 px-3 py-1.5 text-center text-xs text-fg-2" role="status">
				{wide ? 'Press \\ or hold the photo to see it as shot' : 'Press and hold the photo to see it as shot'}
			</p>
		{/if}
		{#if status || error}
			<p class="absolute inset-x-4 top-[max(1rem,env(safe-area-inset-top))] text-center text-sm {error ? 'text-danger' : 'text-fg-2'}">{error || status}</p>
		{:else if onDevice && installedApp && !onDeviceNoticeDismissed}
			<div role="note" class="absolute inset-x-4 bottom-[max(1rem,env(safe-area-inset-bottom))] mx-auto flex max-w-md items-start gap-2 rounded bg-ground/85 px-3 py-2 text-xs text-fg-2">
				<p class="flex-1">
					Developed on this device, so some advanced features might be missing.
					<a href="?server" data-sveltekit-reload class="underline underline-offset-2">Develop it on the server</a>
				</p>
				<button type="button" class="-my-1 px-1 text-fg-2" aria-label="Dismiss" onclick={() => (onDeviceNoticeDismissed = true)}>×</button>
			</div>
		{/if}
		<!-- The way back. Until this, the only way out of the editor on a phone was the browser's own back
		     gesture, which a photo opened from a share or a link does not always have. -->
		<a
			href="/"
			class="absolute top-[max(0.75rem,env(safe-area-inset-top))] left-3 grid size-8 place-items-center rounded-full bg-ground/80 text-fg-2"
			aria-label="Back to Sidecar"
			title="Back to Sidecar"
		>
			<svg viewBox="0 0 16 16" class="size-4" aria-hidden="true"><path d="M9.5 3.5 5 8l4.5 4.5" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" /></svg>
		</a>
		<button
			type="button"
			class="absolute top-[max(0.75rem,env(safe-area-inset-top))] left-[3.25rem] grid size-8 place-items-center rounded-full {toolsHidden ? 'bg-fg text-ground' : 'bg-ground/80 text-fg-2'}"
			aria-pressed={toolsHidden}
			aria-label={toolsHidden ? 'Show the editing tools' : 'Hide the editing tools'}
			title={toolsHidden ? 'Show the editing tools (H)' : 'Hide the editing tools (H)'}
			onclick={() => setToolsHidden(!toolsHidden)}
		>
			<svg viewBox="0 0 16 16" class="size-4" aria-hidden="true"><rect x="1.75" y="2.25" width="12.5" height="11.5" rx="1.75" fill="none" stroke="currentColor" stroke-width="1.5" /><path d="M2 10h12" stroke="currentColor" stroke-width="1.5" /></svg>
		</button>
		{#if wide && toolsHidden && edit && meta}
			<nav class="absolute bottom-3 left-1/2 flex max-w-[calc(100%-1.5rem)] -translate-x-1/2 gap-1 overflow-x-auto rounded-full bg-panel/90 p-1" aria-label="Tools">
				{#each sections as s (s.id)}
					<button type="button" class="h-8 shrink-0 rounded-full px-3 text-xs text-fg-2 hover:bg-raised" aria-label={s.label} onclick={() => setToolsHidden(false, s.id)}>{s.short}</button>
				{/each}
			</nav>
		{/if}
		{#if before}<span class="absolute top-[max(1rem,env(safe-area-inset-top))] left-24 rounded bg-ground/80 px-2 py-1 text-xs">{compare?.label ?? (developed ? 'As saved' : 'As shot')}</span>{/if}
		<div class="absolute top-[max(0.75rem,env(safe-area-inset-top))] right-3 grid justify-items-end gap-1.5">
			{#if !wide && showPhoneHistogram && !fallback}
				<div class="transition-opacity {painting ? 'opacity-30' : ''}" title={painting ? 'Paused while painting' : undefined}>
					<Histogram data={histogram} {clipHigh} {clipLow} compact ontoggle={(w) => { if (w === 'high') clipHigh = !clipHigh; else clipLow = !clipLow; schedule(); }} />
				</div>
			{/if}
			{#if !wide && !fallback}
				<!-- With the photo's other toggles, above them: in the tool row it was pushed off a 390-pixel screen. -->
				<button type="button" aria-pressed={showPhoneHistogram} class="rounded-full px-2.5 py-1 text-[0.6875rem] {showPhoneHistogram ? 'bg-fg text-ground' : 'bg-ground/80 text-fg-2'}" onclick={() => (showPhoneHistogram = !showPhoneHistogram)}>Histogram</button>
				<button type="button" aria-pressed={peaking} class="rounded-full px-2.5 py-1 text-[0.6875rem] {peaking ? 'bg-fg text-ground' : 'bg-ground/80 text-fg-2'}" onclick={() => { peaking = !peaking; schedule(); }}>Focus</button>
				<!-- Beside Focus, because checking detail is something one does at any time, not only while sharpening
				     (the October interface review, item 8). Detail keeps its own loupe. -->
				<button type="button" aria-pressed={loupeOver} class="rounded-full px-2.5 py-1 text-[0.6875rem] {loupeOver ? 'bg-fg text-ground' : 'bg-ground/80 text-fg-2'}" onclick={() => (loupeOver = !loupeOver)}>Loupe</button>
				{#if cameraOffered}
					<button type="button" aria-pressed={showCamera} class="rounded-full px-2.5 py-1 text-[0.6875rem] {showCamera ? 'bg-fg text-ground' : 'bg-ground/80 text-fg-2'}" onclick={() => (showCamera = !showCamera)}>Camera</button>
				{/if}
				{/if}
				{#if !wide}
					<button type="button" class="rounded-full bg-ground/80 px-2.5 py-1 text-[0.6875rem] text-fg-2" aria-label="Viewing and exporting in {SPACE_LABELS[displaySpace]}: switch to {displaySpace === 'srgb' ? 'Display P3' : 'sRGB'}" onclick={() => setOutputSpace(displaySpace === 'srgb' ? 'display-p3' : 'srgb')}>{displaySpace === 'srgb' ? 'sRGB' : 'P3'}</button>
				{/if}
				{#if !wide && !fallback}
				{#if meta?.info.afPoint}
					<button type="button" aria-pressed={showAfPoint} class="rounded-full px-2.5 py-1 text-[0.6875rem] {showAfPoint ? 'bg-fg text-ground' : 'bg-ground/80 text-fg-2'}" onclick={() => (showAfPoint = !showAfPoint)}>AF</button>
				{/if}
			{/if}
		</div>
		<!-- The loupe over a corner of the photo, from the toggle beside Focus. A tap on the photo moves it, as it
		     does for Detail's. Opposite the toggles, so neither covers the other. -->
		{#if loupeOver && edit && !cropping && !fallback}
			<div class="absolute bottom-[max(0.75rem,env(safe-area-inset-bottom))] left-3 w-40 lg:w-56">
				<Loupe assetKey={key} edit={$state.snapshot(edit)} center={loupeCenter} colorSpace={displaySpace} />
			</div>
		{/if}
		<!-- How far in, and the way back out. Only while zoomed: at fit there is nothing to say. On a desktop it is
		     the button that fits, since a wheel is easy to overshoot with; a phone pinches back and only reads the
		     number (the owner's call on the October interface review, item 2). -->
		{#if zoomPercent && !cropping && !fallback}
			{#if wide}
				<button
					type="button"
					class="absolute right-3 bottom-3 rounded-full bg-ground/80 px-2.5 py-1 text-[0.6875rem] text-fg-2 hover:bg-raised"
					title="Fit the photo (double click it for 100 %)"
					onclick={() => { viewport = { zoom: 1, panX: 0, panY: 0 }; schedule(); }}
				>Fit · {zoomPercent} %</button>
			{:else}
				<span class="pointer-events-none absolute right-3 bottom-[max(0.75rem,env(safe-area-inset-bottom))] rounded-full bg-ground/80 px-2.5 py-1 text-[0.6875rem] text-fg-2" role="status">{zoomPercent} %</span>
			{/if}
		{/if}
	</div>

	{#if edit && meta && !(wide && toolsHidden)}
		<!-- Phone: one tool at a time in a bottom sheet. Desktop: all sections in pipeline order. Tools hidden:
		     the phone keeps its tab row, the desktop a strip of tabs over the photo. -->
		<aside
			class="flex max-h-[48dvh] shrink-0 flex-col bg-panel pb-[max(0.75rem,env(safe-area-inset-bottom))] lg:max-h-none lg:w-80 lg:pb-3"
			style={!wide && !toolsHidden && sheet.vh ? `height: ${sheet.vh}dvh; max-height: ${sheet.vh}dvh` : undefined}
		>
			{#if !wide && !toolsHidden}
				<!-- Hold, then drag: a press on the handle arms it (it lights up), and only then does moving resize the
				     panel, so a swipe that starts here by accident changes nothing (owner's call). -->
				<div
					class="flex h-5 shrink-0 touch-none items-center justify-center"
					role="separator"
					aria-orientation="horizontal"
					aria-label="Panel height: press and hold, then drag"
					onpointerdown={sheetDown}
					onpointermove={sheetMove}
					onpointerup={sheetUp}
					onpointercancel={sheetUp}
				>
					<span class="h-1 w-10 rounded-full transition-colors {sheet.armed ? 'bg-fg' : 'bg-line'}"></span>
				</div>
			{/if}
			{#if !toolsHidden}
			<header class="flex items-center justify-between gap-3 px-6 pt-3 pb-2 lg:flex-col lg:items-stretch lg:gap-2 lg:px-4">
				<div class="min-w-0 text-xs text-fg-2">
					<p class="truncate text-sm text-fg">{meta.fileName}</p>
					<p class="truncate">{cameraLine(meta.info)}</p>
				</div>
				<div class="flex shrink-0 gap-2">
					<button type="button" class="grid size-9 place-items-center rounded bg-raised text-fg-2 disabled:opacity-40" aria-label="Undo" title="Undo (⌘Z)" disabled={!session.canUndo || readOnly} onclick={() => session.undo()}>
						<svg viewBox="0 0 16 16" class="size-4" aria-hidden="true"><path d="M6 3 2 7l4 4M2.5 7H10a4 4 0 0 1 0 8H8" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" /></svg>
					</button>
					<button type="button" class="grid size-9 place-items-center rounded bg-raised text-fg-2 disabled:opacity-40" aria-label="Redo" title="Redo (⇧⌘Z)" disabled={!session.canRedo || readOnly} onclick={() => session.redo()}>
						<svg viewBox="0 0 16 16" class="size-4 -scale-x-100" aria-hidden="true"><path d="M6 3 2 7l4 4M2.5 7H10a4 4 0 0 1 0 8H8" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" /></svg>
					</button>
					<button type="button" class="h-9 rounded bg-raised px-3 text-sm" title="Export (E)" onclick={() => (exporting = true)}>Export</button>
					<button type="button" class="h-9 rounded bg-fg px-4 text-sm font-medium text-ground disabled:bg-raised disabled:text-muted" disabled={!session.dirty || session.saving || readOnly} onclick={() => session.commit()}>
						{session.saving ? 'Saving…' : session.dirty ? 'Save' : 'Saved'}
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
				{#if behindRendering && !readOnly}
					<!-- Still untouched: the picture is as it was made, and editing it will bring it up to date.
					     Said now rather than at the moment of the first change, so it is never a surprise. -->
					<div class="rounded bg-raised p-2 text-xs">
						<p class="text-fg">This edit was made with an older rendering.</p>
						<p class="mt-1 text-fg-2">
							Changing anything brings it up to date first, which keeps everything you set and changes
							only how Sidecar renders underneath — so the picture will change. What you have now stays
							in History.
						</p>
						<button type="button" class="mt-2 h-8 rounded bg-fg px-3 text-ground disabled:opacity-40" disabled={session.saving} onclick={adoptRendering}>
							Bring it up to date now
						</button>
					</div>
				{:else if oldRendering}
					<!-- It has just happened, under their own change. The way back is here for as long as the
					     photo is open; a later session asks again, because an edit left on old references is
					     exactly what spreads a rendering nobody can reach any more. -->
					<div class="rounded bg-raised p-2 text-xs">
						<p class="text-fg">Brought up to today's rendering.</p>
						<p class="mt-1 text-fg-2">Everything you set was kept; only the rendering underneath moved.</p>
						<button type="button" class="mt-2 h-8 rounded bg-raised px-3 text-fg-2 underline underline-offset-4" onclick={keepOld}>
							Keep the old rendering
						</button>
					</div>
				{/if}
				{#if !readOnly && !session.writable}<p class="rounded bg-raised p-2 text-xs text-warning">Sidecar can't write in this photo's folder, so edits here can't be saved. Give the account Sidecar runs as write access to it.</p>{/if}
				{#if session.restoredDraft && session.dirty}<p class="rounded bg-raised p-2 text-xs">Restored unsaved changes from this device.</p>{/if}
				{#if session.savedFor}
					<p class="rounded bg-raised p-2 text-xs">
						This photo's sidecar was saved for a different file — the photo was replaced, or another one was
						renamed into its place. Its edit is shown, and saving keeps it for this photo.
						<button type="button" class="underline" onclick={() => session.openVersion('original')}>Start from the original</button>
						instead; the earlier versions stay in History.
					</p>
				{/if}
				{#if session.forked}
					<p class="rounded bg-raised p-2 text-xs">
						This photo was also saved somewhere else since you opened it. Both versions are kept —
						<button type="button" class="underline" onclick={() => { section = 'history'; session.forked = false; }}>see History</button>.
					</p>
				{/if}
			</div>
			{/if}

			{#if wide}
			<div class="px-4 pb-2">
				<!-- Without WebGPU there's no histogram or overlay (the view pass draws them), but it's still the
				     desktop layout: the phone's tab bar below isn't for it. -->
				{#if !fallback}
				<div class="transition-opacity {painting ? 'opacity-30' : ''}" title={painting ? 'Paused while painting' : undefined}>
					<Histogram data={histogram} {clipHigh} {clipLow} ontoggle={(w) => { if (w === 'high') clipHigh = !clipHigh; else clipLow = !clipLow; schedule(); }} />
				</div>
				<details class="mt-1">
					<summary class="cursor-pointer text-[0.6875rem] text-fg-2">Measure</summary>
					<div class="mt-1">
						<Readout
							{readings}
							{probing}
							onprobe={(on) => (probing = on)}
							probed={!!probePoint}
							onclear={clearProbe}
							title="{meta?.fileName ?? 'photo'} · {SPACE_LABELS[displaySpace]}"
						/>
					</div>
				</details>
				{/if}
				<div class="mt-1 flex items-center justify-between gap-2 text-[0.6875rem]">
					<div class="flex items-center gap-2">
						{#if !fallback}
						<button type="button" aria-pressed={peaking} class="rounded-full px-2 py-0.5 {peaking ? 'bg-fg text-ground' : 'bg-raised text-fg-2'}" onclick={() => { peaking = !peaking; schedule(); }}>Focus peaking</button>
							<button type="button" aria-pressed={loupeOver} class="rounded-full px-2 py-0.5 {loupeOver ? 'bg-fg text-ground' : 'bg-raised text-fg-2'}" title="The detail at full resolution, over the photo (Z opens Detail's own)" onclick={() => (loupeOver = !loupeOver)}>Loupe</button>
							{#if cameraOffered}
								<button type="button" aria-pressed={showCamera} class="rounded-full px-2 py-0.5 {showCamera ? 'bg-fg text-ground' : 'bg-raised text-fg-2'}" title="The camera's own rendering beside the edit" onclick={() => (showCamera = !showCamera)}>Camera</button>
							{/if}
						{#if meta?.info.afPoint}
							<button type="button" aria-pressed={showAfPoint} class="rounded-full px-2 py-0.5 {showAfPoint ? 'bg-fg text-ground' : 'bg-raised text-fg-2'}" onclick={() => (showAfPoint = !showAfPoint)}>AF point</button>
						{/if}
						{/if}
					</div>
					<!-- "Output", not "Display": the space decides the export as well as the screen. -->
					<div class="flex items-center gap-1" role="radiogroup" aria-label="Viewing and exporting in" title="The colour space the photo is shown and exported in">
						<span class="pr-0.5 text-muted">Output</span>
						{#each SPACES as sp (sp)}
							<button type="button" role="radio" aria-checked={displaySpace === sp} title="View and export in {SPACE_LABELS[sp]}" class="rounded-full px-2 py-0.5 {displaySpace === sp ? 'bg-fg text-ground' : 'bg-raised text-fg-2'}" onclick={() => setOutputSpace(sp)}>{sp === 'srgb' ? 'sRGB' : 'P3'}</button>
						{/each}
					</div>
				</div>
			</div>

			{:else}
			<nav class="flex items-center gap-2 overflow-x-auto px-6 py-2" aria-label="Tools">
				{#each groups as g (g.id)}
					{@const on = groupOf(section).id === g.id && !toolsHidden}
					<button type="button" aria-pressed={on} class="h-8 shrink-0 rounded-full px-3 text-xs {on ? 'bg-fg text-ground' : 'bg-raised text-fg-2'}" aria-label={g.label} onclick={() => setToolsHidden(false, groupOf(section).id === g.id ? section : g.children[0])}>{g.short}</button>
				{/each}
			</nav>

			{#if !toolsHidden && groupOf(section).children.length > 1}
				<div class="flex gap-1.5 overflow-x-auto px-6 pb-1" role="tablist" aria-label={groupOf(section).label}>
					{#each groupOf(section).children as id (id)}
						<button type="button" role="tab" aria-selected={section === id} class="h-7 shrink-0 rounded-full px-2.5 text-[0.6875rem] {section === id ? 'bg-raised text-fg' : 'text-fg-2'}" aria-label={sectionById(id).label} onclick={() => (section = id)}>{sectionById(id).short}</button>
					{/each}
				</div>
			{/if}
			{#if !toolsHidden}
			<div class="min-h-0 flex-1 overflow-y-auto overscroll-contain px-6 pt-1">
				{#if section === 'lens'}{@render lensSection()}{:else if section === 'crop'}{@render cropSection()}{:else if section === 'wb'}{@render wbSection()}{:else if section === 'light'}{@render lightSection()}{:else if section === 'film'}{@render filmSection()}{:else if section === 'curve'}{@render curveSection()}{:else if section === 'color'}{@render colorSection()}{:else if section === 'depth'}{@render depthSection()}{:else if section === 'masks'}{@render masksSection()}{:else if section === 'history'}{@render historySection()}{:else}{@render detailSection()}{/if}
			</div>
			{/if}

			{/if}
			{#if wide}
			<div class="min-h-0 flex-1 overflow-y-auto px-4">
				{#each groups as g (g.id)}
					{#if g.children.length === 1}
						{@render sectionDetails(sectionById(g.children[0]), true)}
					{:else}
						<!-- One level of folding (owner's call): the group is a heading, its sections fold. -->
						<section class="border-t border-line pt-3 pb-1">
							<h2 class="py-1 text-[11px] font-semibold tracking-wide text-fg-2 uppercase">{g.label}</h2>
							{#each g.children as id (id)}{@render sectionDetails(sectionById(id), false)}{/each}
						</section>
					{/if}
				{/each}
				<button type="button" class="mt-3 mb-1 text-[0.6875rem] text-muted underline underline-offset-2" onclick={() => (showShortcuts = true)}>Keyboard shortcuts (?)</button>
			</div>
			{/if}
		</aside>
	{/if}
</div>

{#if showShortcuts}
	<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
	<div class="fixed inset-0 z-50 grid place-items-center bg-ground/70 p-4" onclick={() => (showShortcuts = false)}>
		<div role="dialog" aria-modal="true" aria-label="Keyboard shortcuts" tabindex="-1" class="w-full max-w-md rounded bg-panel p-4 text-sm" onclick={(e) => e.stopPropagation()}>
			<div class="mb-3 flex items-baseline justify-between">
				<h2 class="font-medium">Keyboard shortcuts</h2>
				<button type="button" class="text-fg-2" aria-label="Close" onclick={() => (showShortcuts = false)}>×</button>
			</div>
			<dl class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5 text-xs">
				{#each SHORTCUTS as [k, what] (k)}<dt class="font-mono whitespace-nowrap text-fg">{k}</dt><dd class="text-fg-2">{what}</dd>{/each}
			</dl>
		</div>
	</div>
{/if}

{#if exporting}<ExportSheet assetKey={key} colorSpace={displaySpace} prepare={() => session.prepareExport()} onclose={() => (exporting = false)} />{/if}
