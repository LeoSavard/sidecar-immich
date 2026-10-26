<!-- SPDX-License-Identifier: Apache-2.0
Detail tile rendered by the server from the full-resolution image, with denoise and sharpening applied — effects
that can't be judged on a downsampled proxy. Refreshes 250 ms after the last change. At 100 % one image pixel is
one screen pixel; at 150 % and 200 % the server renders a smaller tile and it is shown enlarged, pixel for pixel,
so what is judged is still the full-resolution render. A double tap (or double click) steps through the three; the
buttons pick one. -->
<script lang="ts">
	import type { DisplaySpace, EditParams } from '$lib/types';

	type Props = { assetKey: string; edit: EditParams; center: [number, number]; colorSpace: DisplaySpace };
	let { assetKey, edit, center, colorSpace }: Props = $props();

	const SCALES = [1, 1.5, 2] as const;
	type Scale = (typeof SCALES)[number];
	const STORED = 'sidecar.loupeScale';
	const stored = (): Scale => {
		try {
			const v = Number(localStorage.getItem(STORED));
			return (SCALES as readonly number[]).includes(v) ? (v as Scale) : 1;
		} catch {
			return 1;
		}
	};
	let scale = $state<Scale>(stored());

	let box: HTMLDivElement;
	let url = $state<string | null>(null);
	let loading = $state(false);
	let error = $state('');
	let seq = 0;

	$effect(() => {
		const body = JSON.stringify({ center, size: [0, 0], edit, colorSpace, scale });
		const timer = setTimeout(() => refresh(body), 250);
		return () => clearTimeout(timer);
	});

	function setScale(next: Scale) {
		scale = next;
		try {
			localStorage.setItem(STORED, String(next));
		} catch {
			// A private window: the choice lasts as long as the page.
		}
	}

	// A double tap or double click steps 100 → 150 → 200 → 100 %. Told from pointer events, which a phone sends for
	// taps and a desktop for clicks alike; the page's touch-action keeps a phone from zooming itself instead.
	let lastTap = { t: 0, x: 0, y: 0 };
	function tapped(e: PointerEvent) {
		const now = performance.now();
		if (now - lastTap.t < 320 && Math.hypot(e.clientX - lastTap.x, e.clientY - lastTap.y) < 24) {
			setScale(SCALES[(SCALES.indexOf(scale) + 1) % SCALES.length]);
			lastTap = { t: 0, x: 0, y: 0 };
		} else lastTap = { t: now, x: e.clientX, y: e.clientY };
	}

	async function refresh(json: string) {
		const mine = ++seq;
		const body = JSON.parse(json);
		// The tile in image pixels: the box's device pixels at 100 %, fewer when it is to be shown enlarged.
		const px = Math.min(1024, Math.round(((box?.clientWidth ?? 320) * devicePixelRatio) / body.scale));
		body.size = [px, px];
		delete body.scale;
		loading = true;
		try {
			const r = await fetch(`/api/assets/${assetKey}/loupe`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) });
			if (!r.ok) throw new Error(await r.text());
			const blob = await r.blob();
			if (mine !== seq) return;
			if (url) URL.revokeObjectURL(url);
			url = URL.createObjectURL(blob);
			error = '';
		} catch (e) {
			if (mine === seq) error = e instanceof Error ? e.message : String(e);
		} finally {
			if (mine === seq) loading = false;
		}
	}
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div bind:this={box} class="relative aspect-square w-full touch-manipulation overflow-hidden rounded bg-ground select-none" onpointerup={tapped}>
	{#if url}<img src={url} alt="Detail at {scale * 100} %" class="h-full w-full object-cover [image-rendering:pixelated]" draggable="false" />{/if}
	<div class="absolute bottom-1 left-1 flex items-center gap-0.5 rounded bg-ground/80 p-0.5 text-[10px] text-fg-2">
		{#each SCALES as s (s)}
			<button
				type="button"
				class="rounded px-1.5 py-0.5 {scale === s ? 'bg-fg text-ground' : ''}"
				aria-pressed={scale === s}
				title="Show the detail at {s * 100} % (double tap the loupe to step through)"
				onpointerup={(e) => e.stopPropagation()}
				onclick={() => setScale(s)}>{s * 100} %</button
			>
		{/each}
		{#if loading}<span class="px-1">rendering…</span>{/if}
	</div>
	{#if error}<p class="absolute inset-x-2 top-2 text-xs text-danger">{error}</p>{/if}
	{#if !url && !error}<p class="pointer-events-none absolute inset-0 grid place-items-center text-xs text-muted">Rendering detail…</p>{/if}
</div>
