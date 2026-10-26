<!-- SPDX-License-Identifier: Apache-2.0
Sidecar on the phone. A photo chosen here stays on the phone: a developed one (JPEG, HEIC, PNG…) is decoded and
developed by the phone's own GPU, through the same WGSL and the same colour packer as the editor; a RAW goes to
the server, which is the only place that decodes one today (docs/MOBILE.md, stages). -->
<script lang="ts">
	import { onMount, tick } from 'svelte';
	import { api } from '$lib/api';
	import { ColorPacker } from '$lib/color/pack';
	import { DevelopPass, requestDevice } from '$lib/gpu/develop';
	import { ViewPass } from '$lib/gpu/view';
	import { ACCEPT_RAW, isRaw } from '$lib/formats';
	import { destinations, importPhoto, type Who } from '$lib/intake';
	import { keepShared, sha1Hex } from '$lib/shared';
	import type { CameraColor, DisplaySpace, EditParams } from '$lib/types';
	import LightSection from '$lib/ui/sections/LightSection.svelte';
	import WhiteBalanceSection from '$lib/ui/sections/WhiteBalanceSection.svelte';
	import ColorSection from '$lib/ui/sections/ColorSection.svelte';
	import { decodeLocally } from '../local/decode';
	import { encodeJpeg, saveToDevice } from '../local/export';
	import { showPreview, type ShownPreview } from '../local/preview';
	import { decodeRaw } from '../local/raw';
	import { IngestPass, type Proxy } from '$lib/gpu/ingest';
	import type { SourceKind } from '$lib/types';

	/** The long edge a photo is decoded at for editing: the phone-sized proxy the server would send. */
	const EDIT_EDGE = 2560;

	type Phase = 'start' | 'decoding' | 'editing' | 'raw' | 'no-gpu' | 'undecodable';
	let phase = $state<Phase>('start');
	let message = $state('');
	let busy = $state('');

	let file: File | null = null;
	/** A RAW's embedded preview, shown at once while the RAW itself is decoded (or waits for the server). */
	let preview = $state<ShownPreview | null>(null);
	/** Where decoding a RAW on the phone stands, or why it can't happen here. */
	let rawStatus = $state('');
	/** Counts picks, so a decode that finishes after another photo was chosen doesn't open the old one. */
	let picks = 0;
	let sha1 = '';
	let fileName = $state('');

	// ---------- the local pipeline: the editor's own passes and packer
	let canvas = $state<HTMLCanvasElement | null>(null);
	let device: GPUDevice | null = null;
	let packer: ColorPacker | null = null;
	let develop: DevelopPass | null = null;
	let view: ViewPass | null = null;
	let ingest: IngestPass | null = null;
	let camera: CameraColor | null = null;
	/** A RAW renders as sensor light; a JPEG or HEIC as the picture it already is (DESIGN.md §7). */
	let sourceKind = $state<SourceKind>('developed');
	let display: DisplaySpace = 'srgb';
	let defaults: EditParams | null = null;
	let edit = $state<EditParams | null>(null);
	let asShot = $state({ temperatureK: 6504, tint: 0 });
	let before = $state(false);
	let tab = $state<'light' | 'colour' | 'white'>('light');
	let frame = 0;

	onMount(() => {
		// The packer is wasm and loads whether or not there is a GPU; start it while a photo is being chosen.
		ColorPacker.load().then((p) => (packer = p));
	});

	// The canvas is drawn at its own size in device pixels: a rotation or the toolbar resizing it redraws.
	$effect(() => {
		if (!canvas) return;
		const resize = new ResizeObserver(() => schedule());
		resize.observe(canvas);
		return () => resize.disconnect();
	});

	let library = $state<HTMLInputElement | null>(null);
	let files = $state<HTMLInputElement | null>(null);

	async function chosen(e: Event) {
		const input = e.currentTarget as HTMLInputElement;
		const picked = input.files?.[0];
		input.value = '';
		if (!picked) return;
		const pick = ++picks;
		file = picked;
		fileName = picked.name;
		message = '';
		rawStatus = '';
		phase = 'decoding';
		const bytes = await picked.arrayBuffer();
		sha1 = await sha1Hex(bytes);
		// A RAW is told by its extension, not by trying: a browser that can decode a RAW's embedded JPEG would
		// hand that back as the photo.
		if (isRaw(picked.name)) {
			// What the phone can show at once is the camera's own rendering, from inside the file (stage 1); the
			// RAW itself is developed meanwhile (stage 2).
			if (preview) URL.revokeObjectURL(preview.url);
			preview = await showPreview(new Uint8Array(bytes), EDIT_EDGE);
			if (pick !== picks) return;
			phase = 'raw';
			rawStatus = 'Developing it on this phone…';
			const local = await developRaw(bytes);
			if (pick !== picks) return;
			if (typeof local === 'string') {
				rawStatus = local;
				return;
			}
			await startEditing(local.proxy, local.camera, 'raw');
			return;
		}
		const photo = await decodeLocally(picked, EDIT_EDGE);
		if (pick !== picks) return;
		if (!photo) {
			phase = 'undecodable';
			return;
		}
		if (!(await gpu())) {
			phase = 'no-gpu';
			return;
		}
		packer ??= await ColorPacker.load();
		const camera = packer.sourceColor(photo.space);
		if (new URLSearchParams(location.search).has('inspect')) Object.assign(window, { __sidecarInspect: { proxy: photo, camera, space: photo.space } });
		await startEditing(photo, camera, 'developed');
	}

	/** The GPU device, asked for once; null where this browser has no WebGPU. */
	async function gpu(): Promise<GPUDevice | null> {
		if (device) return device;
		try {
			device = await requestDevice();
		} catch (err) {
			console.error('WebGPU unavailable:', err);
			return null;
		}
		device.lost.then((info) => {
			phase = 'start';
			message = `The GPU was reset (${info.reason}). Choose the photo again to carry on.`;
			device = develop = view = ingest = null;
		});
		return device;
	}

	/** Above this, a phone's GPU memory is the risk: the ingest holds about 60 bytes per sensor pixel today. */
	const PHONE_MAX_PIXELS = 26e6;

	/**
	 * A RAW decoded on the phone: LibRaw in a worker for the mosaic, the server's ingest shaders on this GPU for
	 * the proxy, and the colour model the server would give the file. A sentence saying why not, otherwise.
	 */
	async function developRaw(bytes: ArrayBuffer): Promise<{ proxy: Proxy; camera: CameraColor } | string> {
		const device = await gpu();
		if (!device) return "This browser has no WebGPU, so the phone can't develop the RAW itself; Sidecar on the server can.";
		const raw = await decodeRaw(bytes);
		if (raw.error !== undefined) return `The phone couldn't read this RAW (${raw.error}); the server may be able to.`;
		if (!raw.mosaic) return 'This RAW holds an already demosaiced image (an sRAW or a linear DNG), which the server decodes.';
		const { mosaic } = raw;
		const limit = device.limits.maxTextureDimension2D;
		if (mosaic.width > limit || mosaic.height > limit) return `At ${mosaic.width} × ${mosaic.height} it is larger than this GPU's textures go; the server can develop it.`;
		if (matchMedia('(pointer: coarse)').matches && mosaic.width * mosaic.height > PHONE_MAX_PIXELS) {
			return `At ${((mosaic.width * mosaic.height) / 1e6).toFixed(0)} megapixels it is more than a phone's GPU should be asked to hold for now; the server can develop it.`;
		}
		packer ??= await ColorPacker.load();
		const camera = packer.cameraColor(raw.info.color);
		const [r, , b] = camera.asShotMul;
		try {
			ingest ??= await IngestPass.create(device);
			const proxy = await ingest.proxy(mosaic, [r, 1, b], EDIT_EDGE);
			// For tests and for looking: `?inspect` leaves the proxy and its colour model on the window.
			if (new URLSearchParams(location.search).has('inspect')) Object.assign(window, { __sidecarInspect: { proxy, camera, info: raw.info } });
			return { proxy, camera };
		} catch (err) {
			console.error('ingest failed:', err);
			return `Developing it on this phone failed (${err instanceof Error ? err.message : err}); the server can develop it.`;
		}
	}

	/** Open the editor on a proxy: the same develop pass, packer and controls whatever the photo came from. */
	async function startEditing(proxy: { width: number; height: number; f16: Uint16Array }, color: CameraColor, kind: SourceKind) {
		packer ??= await ColorPacker.load();
		const gpuDevice = (await gpu())!;
		display = matchMedia('(color-gamut: p3)').matches ? 'display-p3' : 'srgb';
		camera = color;
		sourceKind = kind;
		defaults = packer.defaultEdit();
		const asShotPack = packer.pack(defaults, camera, kind, display);
		asShot = { temperatureK: asShotPack.temperatureK, tint: asShotPack.tint };
		edit = structuredClone(defaults);
		phase = 'editing';
		await tick();
		develop ??= await DevelopPass.create(gpuDevice);
		// A new canvas each time a photo opens, so a new view on it; the develop pass outlives them.
		view = await ViewPass.create(gpuDevice, canvas!, display === 'display-p3');
		develop.setSource(proxy.width, proxy.height, proxy.f16);
		schedule();
	}

	/** Develop and draw on the next frame; any number of slider moves before it cost one render. */
	function schedule() {
		if (!frame) frame = requestAnimationFrame(draw);
	}

	function render(source: EditParams, space: DisplaySpace) {
		const packed = packer!.pack(source, camera!, sourceKind, space);
		develop!.setUniforms(packed.uniform);
		develop!.setToneLut(packed.lut);
		develop!.setMono(packed.monoLut);
		develop!.setSlide(packed.slideExposure, new Float32Array(0));
		develop!.setMasks(packed.masks, [], packed.maskCurves);
		develop!.render();
	}

	function draw() {
		frame = 0;
		if (!develop || !view || !canvas || !edit) return;
		const dpr = devicePixelRatio;
		canvas.width = Math.round(canvas.clientWidth * dpr);
		canvas.height = Math.round(canvas.clientHeight * dpr);
		render(before ? defaults! : $state.snapshot(edit), display);
		view.render(develop.output!, canvas, { zoom: 1, panX: 0, panY: 0 }, 0);
	}

	/** What the editor's sections call: change the edit, then draw. */
	function onchange(mutate: (e: EditParams) => void) {
		if (!edit) return;
		mutate(edit);
		schedule();
	}

	const edited = $derived(!!edit && !!defaults && JSON.stringify($state.snapshot(edit)) !== JSON.stringify(defaults));

	function hold(on: boolean) {
		before = on;
		schedule();
	}

	// ---------- leaving the phone
	async function saveCopy() {
		if (!develop || !edit) return;
		busy = 'Saving…';
		try {
			// An export is sRGB, whatever the screen shows: what a file without its own profile is read as.
			render($state.snapshot(edit), 'srgb');
			const jpeg = await encodeJpeg(develop);
			render($state.snapshot(edit), display);
			schedule();
			const stem = fileName.replace(/\.[^.]+$/, '');
			const outcome = await saveToDevice(jpeg, `${stem}-sidecar.jpg`);
			message = outcome === 'cancelled' ? '' : outcome === 'shared' ? 'Saved.' : 'Downloaded.';
		} catch (e) {
			message = e instanceof Error ? e.message : String(e);
		}
		busy = '';
	}

	/**
	 * To the full editor on the server. Untouched — or a RAW — it goes the way a share does: kept on the phone
	 * and opened by checksum, so a photo already in the library opens as itself and one that isn't is offered for
	 * import. Edited, it is imported and the edit saved on it, so the work done here is the first version there.
	 */
	async function continueOnServer() {
		if (!file) return;
		busy = 'Sending…';
		message = '';
		try {
			if (!edited) {
				await keepShared(file, sha1);
				location.href = `/open?${new URLSearchParams({ sha1, name: file.name, size: String(file.size) })}`;
				return;
			}
			const who: Who = await (await api('/api/session')).json();
			const [to] = destinations(who);
			if (!to) throw new Error('This Sidecar has no library and no editing folder to put a photo into.');
			const key = await importPhoto(file, to);
			const state = await (await api(`/api/assets/${key}/edit`)).json();
			await api(`/api/assets/${key}/edit`, {
				method: 'PUT',
				headers: { 'content-type': 'application/json' },
				body: JSON.stringify({ edit: $state.snapshot(edit), base: state.head, checkpoint: true })
			});
			location.href = `/edit/${key}`;
		} catch (e) {
			message = e instanceof Error ? e.message : String(e);
			busy = '';
		}
	}

	function reset() {
		if (!defaults) return;
		edit = structuredClone(defaults);
		schedule();
	}
</script>

<svelte:head><title>Sidecar on this phone</title></svelte:head>

<input bind:this={library} type="file" class="sr-only" accept="image/*" onchange={chosen} />
<input bind:this={files} type="file" class="sr-only" accept={`image/*,${ACCEPT_RAW}`} onchange={chosen} />

{#if phase === 'editing' && edit}
	<main class="flex h-dvh flex-col bg-ground text-fg">
		<div class="relative min-h-0 flex-1">
			<!-- svelte-ignore a11y_no_static_element_interactions -->
			<canvas
				bind:this={canvas}
				class="block h-full w-full touch-none"
				aria-label="The photo, as edited. Press and hold to see it as it was."
				onpointerdown={() => hold(true)}
				onpointerup={() => hold(false)}
				onpointercancel={() => hold(false)}
				onpointerleave={() => before && hold(false)}
			></canvas>
			{#if before}<span class="pointer-events-none absolute top-3 left-3 rounded bg-ground/80 px-2 py-1 text-xs">As it was</span>{/if}
		</div>
		<nav class="flex border-t border-line text-sm" aria-label="Controls">
			{#each [{ v: 'light', label: 'Light' }, { v: 'colour', label: 'Colour' }, { v: 'white', label: 'White balance' }] as t (t.v)}
				<button type="button" aria-pressed={tab === t.v} class="h-11 flex-1 {tab === t.v ? 'text-fg' : 'text-fg-2'}" onclick={() => (tab = t.v as typeof tab)}>{t.label}</button>
			{/each}
			<button type="button" class="h-11 px-3 text-xs text-fg-2 disabled:opacity-40" disabled={!edited} onclick={reset}>Reset</button>
		</nav>
		<section class="max-h-[38dvh] overflow-y-auto px-4 py-3">
			{#if tab === 'light'}
				<LightSection {edit} {onchange} oncommit={() => {}} developed={sourceKind === 'developed'} />
			{:else if tab === 'colour'}
				<ColorSection {edit} {onchange} oncommit={() => {}} />
			{:else}
				<WhiteBalanceSection {edit} {onchange} oncommit={() => {}} developed={sourceKind === 'developed'} {asShot} />
			{/if}
		</section>
		<footer class="flex items-center gap-2 border-t border-line px-4 pt-2 pb-[max(0.5rem,env(safe-area-inset-bottom))] text-sm">
			<button type="button" class="h-11 rounded pr-3 text-fg-2" onclick={() => ((phase = 'start'), (edit = null))}>Close</button>
			<span class="flex-1"></span>
			<button type="button" class="h-11 rounded bg-raised px-3 disabled:opacity-50" disabled={!!busy} onclick={saveCopy}>Save a copy</button>
			<button type="button" class="h-11 rounded bg-raised px-3 disabled:opacity-50" disabled={!!busy} onclick={continueOnServer}>Open in Sidecar</button>
		</footer>
		{#if busy || message}<p role="status" class="px-4 pb-2 text-xs text-fg-2">{busy || message}</p>{/if}
	</main>
{:else}
	<main class="mx-auto min-h-dvh max-w-xl bg-ground px-4 pt-[max(1.5rem,env(safe-area-inset-top))] pb-[max(1.5rem,env(safe-area-inset-bottom))] text-fg">
		<header class="flex items-baseline justify-between">
			<h1 class="text-xl">Sidecar</h1>
			<a href="/" class="text-sm text-fg-2 underline underline-offset-4">Full editor</a>
		</header>
		<p class="mt-2 text-sm text-fg-2">A photo you choose here is developed on this phone and stays on it until you send it.</p>

		<div class="mt-6 grid gap-3">
			<button type="button" class="h-14 rounded bg-raised text-base" onclick={() => library?.click()}>Choose from Photos</button>
			<button type="button" class="h-14 rounded bg-panel text-base" onclick={() => files?.click()}>Choose a file, RAW included</button>
		</div>

		{#if phase === 'decoding'}
			<p class="mt-4 text-sm text-fg-2" aria-live="polite">Opening <span class="font-mono">{fileName}</span>…</p>
		{:else if phase === 'raw'}
			<section class="mt-4 overflow-hidden rounded bg-panel text-sm">
				{#if preview}
					<img src={preview.url} alt="The camera's own preview of {fileName}" class="block max-h-[60dvh] w-full bg-ground object-contain" />
					<p class="px-3 pt-2 text-xs text-fg-2">
						The camera's own rendering, {preview.stored[0]} × {preview.stored[1]}, from inside the file: what the camera made of it, not
						Sidecar's development.
					</p>
				{/if}
				<div class="p-3">
					<p aria-live="polite">{rawStatus}</p>
					<button type="button" class="mt-3 h-11 rounded bg-raised px-4 disabled:opacity-50" disabled={!!busy} onclick={continueOnServer}>Open it in Sidecar</button>
				</div>
			</section>
		{:else if phase === 'no-gpu'}
			<section class="mt-4 rounded bg-panel p-3 text-sm">
				<p>This browser has no WebGPU, so it can't develop a photo on the phone. Sidecar on the server can, and shows you the result.</p>
				<button type="button" class="mt-3 h-11 rounded bg-raised px-4 disabled:opacity-50" disabled={!!busy} onclick={continueOnServer}>Open it in Sidecar</button>
			</section>
		{:else if phase === 'undecodable'}
			<section class="mt-4 rounded bg-panel p-3 text-sm">
				<p>This browser can't decode <span class="font-mono">{fileName}</span>. The server may be able to.</p>
				<button type="button" class="mt-3 h-11 rounded bg-raised px-4 disabled:opacity-50" disabled={!!busy} onclick={continueOnServer}>Open it in Sidecar</button>
			</section>
		{/if}
		{#if busy || message}<p role="status" class="mt-3 text-sm text-fg-2">{busy || message}</p>{/if}
	</main>
{/if}
