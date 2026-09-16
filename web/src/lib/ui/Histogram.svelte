<!-- SPDX-License-Identifier: Apache-2.0 -->
<script lang="ts">
	import type { HistogramData } from '$lib/gpu/histogram';

	type Props = { data: HistogramData | null; clipHigh: boolean; clipLow: boolean; ontoggle: (which: 'high' | 'low') => void; compact?: boolean };
	let { data, clipHigh, clipLow, ontoggle, compact = false }: Props = $props();
	let canvas: HTMLCanvasElement;

	$effect(() => {
		if (!data || !canvas) return;
		const w = (canvas.width = Math.round(canvas.clientWidth * devicePixelRatio));
		const h = (canvas.height = Math.round(canvas.clientHeight * devicePixelRatio));
		const ctx = canvas.getContext('2d')!;
		ctx.clearRect(0, 0, w, h);
		let peak = 1;
		for (const ch of [data.r, data.g, data.b]) for (let i = 1; i < 255; i++) peak = Math.max(peak, ch[i]);
		const scale = (v: number) => Math.sqrt(v / peak) * h * 0.95;
		ctx.globalCompositeOperation = 'lighter';
		const channels: [Uint32Array, string][] = [[data.r, 'rgba(255,80,80,0.55)'], [data.g, 'rgba(80,220,80,0.55)'], [data.b, 'rgba(90,120,255,0.6)']];
		for (const [bins, color] of channels) {
			ctx.beginPath();
			ctx.moveTo(0, h);
			for (let i = 0; i < 256; i++) ctx.lineTo((i / 255) * w, h - Math.min(h, scale(bins[i])));
			ctx.lineTo(w, h);
			ctx.closePath();
			ctx.fillStyle = color;
			ctx.fill();
		}
	});

	const pct = (n: number) => (data && data.total ? ((n / data.total) * 100).toFixed(n / data.total < 0.001 ? 2 : 1) : '0');
</script>

<div class="relative {compact ? 'h-12 w-24' : 'h-20 w-full'} overflow-hidden rounded bg-ground">
	<canvas bind:this={canvas} class="h-full w-full" aria-hidden="true"></canvas>
	<button
		type="button"
		class="absolute top-0 left-0 grid size-8 place-items-start p-1"
		aria-pressed={clipLow}
		aria-label="Show shadow clipping ({pct(data?.clippedLow ?? 0)} % of pixels)"
		title="Shadows at black: {pct(data?.clippedLow ?? 0)} %"
		onclick={() => ontoggle('low')}
	>
		<span class="block size-0 border-t-[10px] border-r-[10px] border-t-current border-r-transparent {clipLow ? 'text-[#0000ff]' : (data?.clippedLow ?? 0) > 0 ? 'text-fg-2' : 'text-raised'}"></span>
	</button>
	<button
		type="button"
		class="absolute top-0 right-0 grid size-8 place-items-end p-1"
		style="align-items: start"
		aria-pressed={clipHigh}
		aria-label="Show highlight clipping ({pct(data?.clippedHigh ?? 0)} % of pixels)"
		title="Highlights at white: {pct(data?.clippedHigh ?? 0)} %"
		onclick={() => ontoggle('high')}
	>
		<span class="block size-0 border-t-[10px] border-l-[10px] border-t-current border-l-transparent {clipHigh ? 'text-[#ff0000]' : (data?.clippedHigh ?? 0) > 0 ? 'text-fg-2' : 'text-raised'}"></span>
	</button>
</div>
