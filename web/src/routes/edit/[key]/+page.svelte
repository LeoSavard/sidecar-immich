<!-- SPDX-License-Identifier: Apache-2.0 -->
<script lang="ts">
	import { page } from '$app/state';
	import { DevelopPass, requestDevice } from '$lib/gpu/develop';
	import { CLIP_HIGHS, CLIP_LOWS, PEAKING, ViewPass, type Viewport } from '$lib/gpu/view';
	import { HistogramPass, type HistogramData } from '$lib/gpu/histogram';
	import { ColorPacker } from '$lib/color/pack';
	import { api } from '$lib/api';
	import { settings, SPACE_LABELS } from '$lib/settings';
	import { clearDraft, loadDraft, saveDraft } from '$lib/state/drafts';
	import { History } from '$lib/state/history';
	import Slider from '$lib/ui/Slider.svelte';
	import Histogram from '$lib/ui/Histogram.svelte';
	import CurveEditor from '$lib/ui/CurveEditor.svelte';
	import ColorMixer from '$lib/ui/ColorMixer.svelte';
	import ExportSheet from '$lib/ui/ExportSheet.svelte';
	import Loupe from '$lib/ui/Loupe.svelte';
	import type { Band, DisplaySpace, EditParams, Meta, MixerBand } from '$lib/types';

	type EditState = { edit: EditParams; etag: string; readOnly: boolean; exports: unknown[] };
	type Section = 'wb' | 'light' | 'curve' | 'color' | 'detail';

	const key = page.params.key!;
	let canvas: HTMLCanvasElement;
	let meta = $state<Meta | null>(null);
	let edit = $state<EditParams | null>(null);
	let defaults: EditParams;
	let status = $state('Opening…');
	let error = $state('');
	let resolvedWb = $state({ temperatureK: 5000, tint: 0 });
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
	let displaySpace = $state<DisplaySpace>('srgb');
	let viewport: Viewport = { zoom: 1, panX: 0, panY: 0 };

	let etag = '';
	let readOnly = $state(false);
	let dirty = $state(false);
	let saving = $state(false);
	let conflict = $state<EditState | null>(null);
	let restoredDraft = $state(false);
	let draftTimer: ReturnType<typeof setTimeout> | undefined;
	let history: History<EditParams>;

	let develop: DevelopPass;
	let view: ViewPass;
	let hist: HistogramPass;
	let packer: ColorPacker;
	let display: DisplaySpace = 'srgb';
	let developDirty = true;
	let frame = 0;
	let histStride = 1;

	function schedule(dev = false) {
		developDirty ||= dev;
		if (!frame) frame = requestAnimationFrame(draw);
	}

	function draw() {
		frame = 0;
		if (!develop?.output || !edit || !meta) return;
		if (developDirty) {
			const source = before ? defaults : $state.snapshot(edit);
			const packed = packer.pack(source, meta.info.color, display);
			if (!before) {
				resolvedWb = { temperatureK: packed.temperatureK, tint: packed.tint };
				lut = packed.lut;
			}
			develop.setUniforms(packed.uniform);
			develop.setToneLut(packed.lut);
			develop.render();
			developDirty = false;
			hist.run(develop.output, histStride, (h) => (histogram = h));
		}
		view.render(develop.output, canvas, viewport, (clipHigh ? CLIP_HIGHS : 0) | (clipLow ? CLIP_LOWS : 0) | (peaking ? PEAKING : 0));
	}

	/** Continuous change during a gesture: redraw, mark unsaved, keep a device draft. */
	function changed() {
		schedule(true);
		dirty = true;
		clearTimeout(draftTimer);
		draftTimer = setTimeout(() => edit && saveDraft(key, { edit: $state.snapshot(edit), baseEtag: etag, savedAt: Date.now() }), 500);
	}

	/** A gesture ended: one undo step. */
	function committed() {
		if (edit) history.commit($state.snapshot(edit));
	}

	function restore(state: EditParams | null) {
		if (!state || !edit) return;
		edit = state;
		changed();
	}

	async function commit(keepalive = false) {
		if (!edit || !dirty || readOnly || saving) return;
		saving = true;
		try {
			const r = await api(`/api/assets/${key}/edit`, {
				method: 'PUT',
				keepalive,
				headers: { 'content-type': 'application/json', 'if-match': `"${etag}"` },
				body: JSON.stringify($state.snapshot(edit))
			});
			if (r.status === 412) {
				conflict = await r.json();
				return;
			}
			etag = (await r.json()).etag;
			dirty = false;
			restoredDraft = false;
			await clearDraft(key);
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			saving = false;
		}
	}

	/** Save if needed and return the ETag an export should pin, or null when saving failed. */
	async function prepareExport(): Promise<string | null> {
		if (readOnly) return etag;
		if (dirty) await commit();
		return dirty || conflict ? null : etag;
	}

	function keepMine() {
		if (!conflict) return;
		etag = conflict.etag;
		conflict = null;
		commit();
	}

	function useTheirs() {
		if (!conflict) return;
		edit = conflict.edit;
		etag = conflict.etag;
		conflict = null;
		dirty = false;
		clearDraft(key);
		history.reset($state.snapshot(edit));
		schedule(true);
	}

	async function loadProxy(longEdge: number) {
		const r = await api(`/api/assets/${key}/proxy?long_edge=${longEdge}`);
		const w = Number(r.headers.get('x-width'));
		const h = Number(r.headers.get('x-height'));
		develop.setSource(w, h, await r.arrayBuffer());
		schedule(true);
	}

	async function start() {
		try {
			const [metaResp, stateResp] = await Promise.all([api(`/api/assets/${key}/meta`), api(`/api/assets/${key}/edit`)]);
			meta = await metaResp.json();
			const state: EditState = await stateResp.json();
			const [device, p] = await Promise.all([requestDevice(), ColorPacker.load()]);
			packer = p;
			defaults = packer.defaultEdit();
			edit = state.edit;
			etag = state.etag;
			readOnly = state.readOnly;
			const draft = await loadDraft(key);
			if (draft && draft.baseEtag === state.etag && JSON.stringify(draft.edit) !== JSON.stringify(state.edit)) {
				edit = draft.edit;
				dirty = true;
				restoredDraft = true;
			}
			history = new History($state.snapshot(edit));
			const prefs = await settings();
			display = prefs.outputSpace;
			displaySpace = display;
			clipHigh = prefs.clipHighlights;
			clipLow = prefs.clipShadows;
			[develop, view, hist] = await Promise.all([DevelopPass.create(device), ViewPass.create(device, canvas, display === 'display-p3'), HistogramPass.create(device)]);
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
			const out = develop.output!;
			const fit = Math.min(canvas.width / out.width, canvas.height / out.height);
			const dw = out.width * fit * viewport.zoom;
			const dh = out.height * fit * viewport.zoom;
			const u = (px(e).x - ((canvas.width - dw) / 2 + viewport.panX)) / dw;
			const v = (px(e).y - ((canvas.height - dh) / 2 + viewport.panY)) / dh;
			if (u >= 0 && u <= 1 && v >= 0 && v <= 1 && (section === 'detail' || detailOpen)) loupeCenter = [u, v];
			if (now - lastTapAt < 320) {
				const fit = Math.min(canvas.width / develop.output!.width, canvas.height / develop.output!.height);
				zoomAt(px(e), viewport.zoom > 1.01 ? 1 : Math.max(2, 1 / fit));
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
			restore(e.shiftKey ? history.redo() : history.undo());
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
			edit.whiteBalance.temperatureK = Math.round(resolvedWb.temperatureK);
			edit.whiteBalance.tint = Math.round(resolvedWb.tint);
			edit.whiteBalance.mode = 'custom';
		}
		edit.whiteBalance[field] = value;
		changed();
	}

	/** Shadows and Highlights move the curve points at 25 % and 75 % — the same curve the curve editor shows. */
	const CURVE_ANCHORS = { shadows: 0.25, highlights: 0.75 } as const;
	function curveOffset(which: keyof typeof CURVE_ANCHORS): number {
		const x = CURVE_ANCHORS[which];
		const p = edit?.tone.curve.find((q) => Math.abs(q[0] - x) < 1e-6);
		return p ? Math.round(((p[1] - x) / 0.15) * 100) : 0;
	}
	function setCurveOffset(which: keyof typeof CURVE_ANCHORS, v: number) {
		if (!edit) return;
		const x = CURVE_ANCHORS[which];
		const rest = edit.tone.curve.filter((q) => Math.abs(q[0] - x) >= 1e-6);
		edit.tone.curve = v === 0 ? rest : [...rest, [x, Math.min(1, Math.max(0, x + (v / 100) * 0.15))] as [number, number]].sort((a, b) => a[0] - b[0]);
		changed();
	}

	const signed = (v: number, digits = 0) => `${v > 0 ? '+' : ''}${v.toFixed(digits)}`;
	const shutter = (s: number) => (s >= 1 ? `${s}s` : `1/${Math.round(1 / s)}`);
	const TEMP_TRACK = 'linear-gradient(90deg,#5f7fb6,#9aa0a6 45%,#c9a15f)';
	const TINT_TRACK = 'linear-gradient(90deg,#6f9f6f,#9aa0a6 50%,#b07aa6)';
	const sections: { id: Section; label: string; short: string }[] = [
		{ id: 'wb', label: 'White balance', short: 'WB' },
		{ id: 'light', label: 'Light', short: 'Light' },
		{ id: 'curve', label: 'Curve', short: 'Curve' },
		{ id: 'color', label: 'Color', short: 'Color' },
		{ id: 'detail', label: 'Detail', short: 'Detail' }
	];
</script>

<svelte:head><title>{meta?.fileName ?? 'Sidecar'}</title></svelte:head>
<svelte:window onkeydown={keydown} />

{#snippet wbSection()}
	{#if edit}
		<div class="grid gap-2">
			<div class="flex items-center justify-between text-xs">
				<span class="text-fg-2">{edit.whiteBalance.mode === 'asShot' ? 'As shot' : 'Custom'}</span>
				<button type="button" class="rounded px-2 py-1 text-fg-2 hover:bg-raised disabled:opacity-40" disabled={edit.whiteBalance.mode === 'asShot'} onclick={() => { edit!.whiteBalance.mode = 'asShot'; changed(); committed(); }}>Reset to as shot</button>
			</div>
			<Slider id="wb-temp" label="Temperature" value={edit.whiteBalance.mode === 'asShot' ? Math.round(resolvedWb.temperatureK) : edit.whiteBalance.temperatureK} min={2000} max={12000} step={10} defaultValue={Math.round(resolvedWb.temperatureK)} trackGradient={TEMP_TRACK} format={(v) => `${v} K`} onchange={(v) => setWb('temperatureK', v)} oncommit={committed} />
			<Slider id="wb-tint" label="Tint" value={edit.whiteBalance.mode === 'asShot' ? Math.round(resolvedWb.tint) : edit.whiteBalance.tint} min={-150} max={150} defaultValue={0} bipolar trackGradient={TINT_TRACK} format={(v) => signed(v)} onchange={(v) => setWb('tint', v)} oncommit={committed} />
		</div>
	{/if}
{/snippet}

{#snippet lightSection()}
	{#if edit}
		<div class="grid gap-2">
			<Slider id="exposure" label="Exposure" value={edit.exposure.ev} min={-5} max={5} step={0.01} defaultValue={0} bipolar format={(v) => `${signed(v, 2)} EV`} onchange={(v) => { edit!.exposure.ev = v; changed(); }} oncommit={committed} />
			<Slider id="contrast" label="Contrast" value={Math.round((edit.tone.contrast - 1) * 100)} min={-70} max={100} defaultValue={0} bipolar format={(v) => signed(v)} onchange={(v) => { edit!.tone.contrast = 1 + v / 100; changed(); }} oncommit={committed} />
			<Slider id="highlights" label="Highlights" value={curveOffset('highlights')} min={-100} max={100} defaultValue={0} bipolar format={(v) => signed(v)} onchange={(v) => setCurveOffset('highlights', v)} oncommit={committed} />
			<Slider id="shadows" label="Shadows" value={curveOffset('shadows')} min={-100} max={100} defaultValue={0} bipolar format={(v) => signed(v)} onchange={(v) => setCurveOffset('shadows', v)} oncommit={committed} />
			<Slider id="whites" label="Whites" value={Math.round(((4.5 - edit.tone.whiteEv) / 2) * 100)} min={-100} max={100} defaultValue={0} bipolar format={(v) => signed(v)} onchange={(v) => { edit!.tone.whiteEv = 4.5 - (v / 100) * 2; changed(); }} oncommit={committed} />
			<Slider id="blacks" label="Blacks" value={Math.round(((-8 - edit.tone.blackEv) / 3) * 100)} min={-100} max={100} defaultValue={0} bipolar format={(v) => signed(v)} onchange={(v) => { edit!.tone.blackEv = -8 - (v / 100) * 3; changed(); }} oncommit={committed} />
			<Slider id="hl-blend" label="Highlight recovery" value={Math.round(edit.exposure.highlightBlend * 100)} min={0} max={100} defaultValue={50} format={(v) => `${v} %`} onchange={(v) => { edit!.exposure.highlightBlend = v / 100; changed(); }} oncommit={committed} />
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
			<Slider id="sharpen" label="Sharpening" value={edit.detail.sharpenAmount} min={0} max={100} defaultValue={0} format={(v) => String(v)} onchange={(v) => { edit!.detail.sharpenAmount = v; changed(); }} oncommit={committed} />
			<Slider id="sharpen-radius" label="Sharpening radius" value={edit.detail.sharpenRadius} min={0.5} max={2} step={0.1} defaultValue={1} format={(v) => `${v.toFixed(1)} px`} onchange={(v) => { edit!.detail.sharpenRadius = v; changed(); }} oncommit={committed} />
			<Slider id="denoise-luma" label="Noise reduction" value={edit.detail.denoiseLuminance} min={0} max={100} defaultValue={0} format={(v) => String(v)} onchange={(v) => { edit!.detail.denoiseLuminance = v; changed(); }} oncommit={committed} />
			<Slider id="denoise-chroma" label="Colour noise reduction" value={edit.detail.denoiseChroma} min={0} max={100} defaultValue={0} format={(v) => String(v)} onchange={(v) => { edit!.detail.denoiseChroma = v; changed(); }} oncommit={committed} />
		</div>
	{/if}
{/snippet}

{#snippet colorSection()}
	{#if edit}
		<ColorMixer mixer={edit.mixer} onchange={(band: Band, field: keyof MixerBand, v: number) => { edit!.mixer[band][field] = v; changed(); }} oncommit={committed} />
	{/if}
{/snippet}

<div class="flex h-dvh flex-col overflow-hidden bg-ground text-fg select-none [-webkit-touch-callout:none] lg:flex-row">
	<div class="relative min-h-0 flex-1">
		<canvas
			bind:this={canvas}
			class="block h-full w-full touch-none"
			onpointerdown={canvasDown}
			onpointermove={canvasMove}
			onpointerup={canvasUp}
			onpointercancel={canvasUp}
			oncontextmenu={(e) => e.preventDefault()}
		></canvas>
		{#if status || error}
			<p class="absolute inset-x-4 top-[max(1rem,env(safe-area-inset-top))] text-center text-sm {error ? 'text-danger' : 'text-fg-2'}">{error || status}</p>
		{/if}
		{#if before}<span class="absolute top-[max(1rem,env(safe-area-inset-top))] left-4 rounded bg-ground/80 px-2 py-1 text-xs">As shot</span>{/if}
		<div class="absolute top-[max(0.75rem,env(safe-area-inset-top))] right-3 grid justify-items-end gap-1.5">
			{#if !wide && showPhoneHistogram}
				<Histogram data={histogram} {clipHigh} {clipLow} compact ontoggle={(w) => { if (w === 'high') clipHigh = !clipHigh; else clipLow = !clipLow; schedule(); }} />
			{/if}
			{#if !wide}
				<button type="button" aria-pressed={peaking} class="rounded-full px-2.5 py-1 text-[0.6875rem] {peaking ? 'bg-fg text-ground' : 'bg-ground/80 text-fg-2'}" onclick={() => { peaking = !peaking; schedule(); }}>Focus</button>
			{/if}
		</div>
	</div>

	{#if edit && meta}
		<!-- Phone: one tool at a time in a bottom sheet. Desktop: all sections in pipeline order. -->
		<aside class="flex max-h-[48dvh] shrink-0 flex-col bg-panel pb-[max(0.75rem,env(safe-area-inset-bottom))] lg:max-h-none lg:w-80 lg:pb-3">
			<header class="flex items-center justify-between gap-3 px-6 pt-3 pb-2 lg:px-4">
				<div class="min-w-0 text-xs text-fg-2">
					<p class="truncate text-sm text-fg">{meta.fileName}</p>
					<p class="truncate">{meta.info.make} {meta.info.model} · ISO {meta.info.iso} · {shutter(meta.info.shutterS)} · f/{meta.info.aperture}</p>
				</div>
				<div class="flex shrink-0 gap-2">
					<button type="button" class="h-9 rounded bg-raised px-3 text-sm" onclick={() => (exporting = true)}>Export</button>
					<button type="button" class="h-9 rounded bg-fg px-4 text-sm font-medium text-ground disabled:bg-raised disabled:text-muted" disabled={!dirty || saving || readOnly} onclick={() => commit()}>
						{saving ? 'Saving…' : dirty ? 'Save' : 'Saved'}
					</button>
				</div>
			</header>
			<div class="grid gap-2 px-6 lg:px-4">
				{#if readOnly}<p class="rounded bg-raised p-2 text-xs">A newer Sidecar edited this photo. Update Sidecar to change it.</p>{/if}
				{#if restoredDraft && dirty}<p class="rounded bg-raised p-2 text-xs">Restored unsaved changes from this device.</p>{/if}
				{#if conflict}
					<div class="grid gap-2 rounded bg-raised p-2 text-xs">
						<p>This photo was edited somewhere else since you opened it.</p>
						<div class="flex gap-2">
							<button type="button" class="rounded bg-fg px-3 py-1.5 text-ground" onclick={keepMine}>Keep mine</button>
							<button type="button" class="rounded bg-panel px-3 py-1.5" onclick={useTheirs}>Use theirs</button>
						</div>
					</div>
				{/if}
			</div>

			{#if wide}
			<div class="px-4 pb-2">
				<Histogram data={histogram} {clipHigh} {clipLow} ontoggle={(w) => { if (w === 'high') clipHigh = !clipHigh; else clipLow = !clipLow; schedule(); }} />
				<div class="mt-1 flex items-center justify-between gap-2 text-[0.6875rem]">
					<button type="button" aria-pressed={peaking} class="rounded-full px-2 py-0.5 {peaking ? 'bg-fg text-ground' : 'bg-raised text-fg-2'}" onclick={() => { peaking = !peaking; schedule(); }}>Focus peaking</button>
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
				{#if section === 'wb'}{@render wbSection()}{:else if section === 'light'}{@render lightSection()}{:else if section === 'curve'}{@render curveSection()}{:else if section === 'color'}{@render colorSection()}{:else}{@render detailSection()}{/if}
			</div>

			{/if}
			{#if wide}
			<div class="min-h-0 flex-1 overflow-y-auto px-4">
				{#each sections as s (s.id)}
					<details class="border-t border-line py-2" open={s.id === 'wb' || s.id === 'light'} ontoggle={(e) => s.id === 'detail' && (detailOpen = (e.currentTarget as HTMLDetailsElement).open)}>
						<summary class="cursor-pointer py-1 text-xs font-medium">{s.label}</summary>
						<div class="pt-2">
							{#if s.id === 'wb'}{@render wbSection()}{:else if s.id === 'light'}{@render lightSection()}{:else if s.id === 'curve'}{@render curveSection()}{:else if s.id === 'color'}{@render colorSection()}{:else if detailOpen}{@render detailSection()}{/if}
						</div>
					</details>
				{/each}
			</div>
			{/if}
		</aside>
	{/if}
</div>

{#if exporting}<ExportSheet assetKey={key} colorSpace={displaySpace} prepare={prepareExport} onclose={() => (exporting = false)} />{/if}
