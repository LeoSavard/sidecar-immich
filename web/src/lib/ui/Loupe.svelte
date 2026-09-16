<!-- SPDX-License-Identifier: Apache-2.0
1:1 detail tile rendered by the server from the full-resolution image, with denoise and sharpening applied —
effects that can't be judged on a downsampled proxy. Refreshes 250 ms after the last change. -->
<script lang="ts">
	import type { DisplaySpace, EditParams } from '$lib/types';

	type Props = { assetKey: string; edit: EditParams; center: [number, number]; colorSpace: DisplaySpace };
	let { assetKey, edit, center, colorSpace }: Props = $props();

	let box: HTMLDivElement;
	let url = $state<string | null>(null);
	let loading = $state(false);
	let error = $state('');
	let seq = 0;

	$effect(() => {
		const body = JSON.stringify({ center, size: [0, 0], edit, colorSpace });
		const timer = setTimeout(() => refresh(body), 250);
		return () => clearTimeout(timer);
	});

	async function refresh(json: string) {
		const mine = ++seq;
		const px = Math.min(1024, Math.round((box?.clientWidth ?? 320) * devicePixelRatio));
		const body = JSON.parse(json);
		body.size = [px, px];
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

<div bind:this={box} class="relative aspect-square w-full overflow-hidden rounded bg-ground">
	{#if url}<img src={url} alt="Detail at 100 %" class="h-full w-full object-cover [image-rendering:pixelated]" draggable="false" />{/if}
	<span class="absolute bottom-1 left-1 rounded bg-ground/80 px-1.5 py-0.5 text-[10px] text-fg-2">100 %{loading ? ' · rendering…' : ''}</span>
	{#if error}<p class="absolute inset-x-2 top-2 text-xs text-danger">{error}</p>{/if}
	{#if !url && !error}<p class="absolute inset-0 grid place-items-center text-xs text-muted">Rendering detail…</p>{/if}
</div>
