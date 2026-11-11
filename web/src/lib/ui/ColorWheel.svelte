<!-- SPDX-License-Identifier: Apache-2.0
One colour grading wheel: a point on a hue circle — the angle is OKLCh's hue, as the grade applies it, the
distance from the centre the saturation — and the luminance beside it. The ring is drawn with CSS oklch(), so
the colour under a point is the hue it pushes toward. Double-click the wheel to put it back at rest. -->
<script lang="ts">
	import type { GradeWheel } from '$lib/types';
	import Slider from '$lib/ui/Slider.svelte';
	import { decimals, signed } from '$lib/ui/format';

	type Props = { id: string; label: string; wheel: GradeWheel; disabled?: boolean; onchange: (w: GradeWheel) => void; oncommit: () => void };
	let { id, label, wheel, disabled = false, onchange, oncommit }: Props = $props();

	// Hue increases counter-clockwise from the right, as an angle does; a conic gradient runs clockwise, so its
	// stops go down in hue.
	const ring = `conic-gradient(from 90deg, ${Array.from({ length: 13 }, (_, k) => `oklch(0.74 0.13 ${(360 - k * 30) % 360}) ${k * 30}deg`).join(', ')})`;
	const dot = $derived({ x: 50 + 50 * (wheel.saturation / 100) * Math.cos((wheel.hue * Math.PI) / 180), y: 50 - 50 * (wheel.saturation / 100) * Math.sin((wheel.hue * Math.PI) / 180) });
	let dragging = false;

	function at(e: PointerEvent) {
		const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
		const [x, y] = [((e.clientX - r.left) / r.width) * 2 - 1, -(((e.clientY - r.top) / r.height) * 2 - 1)];
		const hue = ((Math.atan2(y, x) * 180) / Math.PI + 360) % 360;
		onchange({ ...wheel, hue: Math.round(hue), saturation: Math.round(Math.min(1, Math.hypot(x, y)) * 100) });
	}
</script>

<div class="grid justify-items-center gap-1">
	<span class="text-xs text-fg-2">{label}</span>
	<div
		role="slider"
		tabindex={disabled ? -1 : 0}
		aria-label="{label} colour"
		aria-valuetext="hue {decimals(wheel.hue)}°, saturation {decimals(wheel.saturation)}"
		aria-valuenow={wheel.saturation}
		class="relative aspect-square w-full max-w-28 touch-none rounded-full {disabled ? 'opacity-40' : 'cursor-crosshair'}"
		style="background: radial-gradient(circle, rgb(128 128 128) 0%, transparent 70%), {ring};"
		onpointerdown={(e) => {
			if (disabled) return;
			dragging = true;
			(e.currentTarget as Element).setPointerCapture(e.pointerId);
			at(e);
		}}
		onpointermove={(e) => dragging && at(e)}
		onpointerup={() => {
			if (dragging) oncommit();
			dragging = false;
		}}
		ondblclick={() => {
			onchange({ ...wheel, hue: 0, saturation: 0 });
			oncommit();
		}}
	>
		<span class="pointer-events-none absolute size-3 -translate-x-1/2 -translate-y-1/2 rounded-full border-2 border-white shadow-[0_0_0_1px_rgba(0,0,0,0.6)]" style="left: {dot.x}%; top: {dot.y}%;"></span>
	</div>
	<div class="w-full">
		<Slider id="{id}-luminance" label="Luminance" value={wheel.luminance} min={-100} max={100} defaultValue={0} bipolar {disabled} format={(v) => signed(v)} onchange={(v) => onchange({ ...wheel, luminance: v })} {oncommit} />
	</div>
</div>
