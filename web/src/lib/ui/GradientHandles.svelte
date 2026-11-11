<!-- SPDX-License-Identifier: Apache-2.0
Handles for a gradient layer (DESIGN.md §15, "Gradients"), drawn over the photo. The gradient is stored in the
photo as shown before lens correction, so everything here is in that frame and the editor supplies the two
mappings to and from the screen — through lens correction, crop and straighten, as strokes are placed.
Linear: the line from full effect to none, with a guide across each end. Radial: the ellipse, a handle at the
centre to move it, and one at the end of each axis to size and turn it. -->
<script lang="ts">
	import type { Gradient } from '$lib/types';

	type Point = [number, number];
	type Props = {
		gradient: Gradient;
		/** The photo as shown, in pixels: what the radial axes' lengths (fractions of the long edge) are of. */
		shown: [number, number];
		/** A point of the photo (fractions) → CSS pixels over the canvas; null off screen or not yet known. */
		toScreen: (p: Point) => { x: number; y: number } | null;
		/** CSS pixels over the canvas → a point of the photo (fractions). */
		toSource: (x: number, y: number) => Point | null;
		onchange: (g: Gradient) => void;
		oncommit: () => void;
	};
	let { gradient, shown, toScreen, toSource, onchange, oncommit }: Props = $props();

	const long = $derived(Math.max(shown[0], shown[1]));
	let dragging = $state<string | null>(null);
	let grabbed: { at: Point; gradient: Gradient } | null = null;

	/** The end of a radial axis, as a point of the photo. */
	function axisEnd(g: Extract<Gradient, { kind: 'radial' }>, which: 0 | 1): Point {
		const a = (g.angle * Math.PI) / 180 + (which === 1 ? Math.PI / 2 : 0);
		const len = g.axes[which] * long;
		return [g.centre[0] + (len * Math.cos(a)) / shown[0], g.centre[1] + (len * Math.sin(a)) / shown[1]];
	}

	/** The ellipse's outline (`scale` of its size), on screen. */
	function outline(g: Extract<Gradient, { kind: 'radial' }>, scale: number): string {
		const pts: string[] = [];
		for (let k = 0; k <= 64; k++) {
			const t = (k / 64) * Math.PI * 2;
			const [ca, sa] = [Math.cos((g.angle * Math.PI) / 180), Math.sin((g.angle * Math.PI) / 180)];
			const [u, v] = [Math.cos(t) * g.axes[0] * long * scale, Math.sin(t) * g.axes[1] * long * scale];
			const s = toScreen([g.centre[0] + (u * ca - v * sa) / shown[0], g.centre[1] + (u * sa + v * ca) / shown[1]]);
			if (s) pts.push(`${s.x},${s.y}`);
		}
		return pts.join(' ');
	}

	/** A guide across the linear gradient at `p`, perpendicular to it, on screen. */
	function across(g: Extract<Gradient, { kind: 'linear' }>, p: Point): [number, number, number, number] | null {
		const [dx, dy] = [(g.to[0] - g.from[0]) * shown[0], (g.to[1] - g.from[1]) * shown[1]];
		const n = Math.hypot(dx, dy) || 1;
		const reach = long * 2;
		const a = toScreen([p[0] - ((dy / n) * reach) / shown[0], p[1] + ((dx / n) * reach) / shown[1]]);
		const b = toScreen([p[0] + ((dy / n) * reach) / shown[0], p[1] - ((dx / n) * reach) / shown[1]]);
		return a && b ? [a.x, a.y, b.x, b.y] : null;
	}

	function down(e: PointerEvent, handle: string) {
		e.stopPropagation();
		(e.currentTarget as Element).setPointerCapture(e.pointerId);
		const box = (e.currentTarget as SVGElement).ownerSVGElement!.getBoundingClientRect();
		const p = toSource(e.clientX - box.left, e.clientY - box.top);
		dragging = handle;
		grabbed = p && { at: p, gradient: structuredClone($state.snapshot(gradient)) as Gradient };
	}

	function move(e: PointerEvent) {
		if (!dragging || !grabbed) return;
		const box = (e.currentTarget as SVGSVGElement).getBoundingClientRect();
		const p = toSource(e.clientX - box.left, e.clientY - box.top);
		if (!p) return;
		const g = structuredClone(grabbed.gradient) as Gradient;
		const shift: Point = [p[0] - grabbed.at[0], p[1] - grabbed.at[1]];
		if (g.kind === 'linear') {
			if (dragging === 'from') g.from = p;
			else if (dragging === 'to') g.to = p;
			else {
				g.from = [g.from[0] + shift[0], g.from[1] + shift[1]];
				g.to = [g.to[0] + shift[0], g.to[1] + shift[1]];
			}
		} else if (dragging === 'centre') {
			g.centre = [g.centre[0] + shift[0], g.centre[1] + shift[1]];
		} else {
			// An axis end: its length from the centre, in the long edge's units; the first one also turns it.
			const [dx, dy] = [((p[0] - g.centre[0]) * shown[0]) / long, ((p[1] - g.centre[1]) * shown[1]) / long];
			if (dragging === 'a') {
				g.axes = [Math.max(0.01, Math.hypot(dx, dy)), g.axes[1]];
				g.angle = (Math.atan2(dy, dx) * 180) / Math.PI;
			} else {
				const a = (g.angle * Math.PI) / 180;
				g.axes = [g.axes[0], Math.max(0.01, Math.abs(-dx * Math.sin(a) + dy * Math.cos(a)))];
			}
		}
		onchange(g);
	}

	function up() {
		if (!dragging) return;
		dragging = null;
		grabbed = null;
		oncommit();
	}

	const knob = 'cursor-grab fill-white/90 stroke-black/60 [stroke-width:1.5] pointer-events-auto';
</script>

<svg class="pointer-events-none absolute inset-0 h-full w-full" role="group" aria-label="Gradient handles" onpointermove={move} onpointerup={up} onpointercancel={up}>
	{#if gradient.kind === 'linear'}
		{@const g = gradient}
		{@const a = toScreen(g.from)}
		{@const b = toScreen(g.to)}
		{#each [across(g, g.from), across(g, g.to)] as l, k (k)}
			{#if l}<line x1={l[0]} y1={l[1]} x2={l[2]} y2={l[3]} class="stroke-white/70 [stroke-width:1] {k ? '[stroke-dasharray:4_4]' : ''}" />{/if}
		{/each}
		{#if a && b}
			<line x1={a.x} y1={a.y} x2={b.x} y2={b.y} class="pointer-events-auto cursor-move stroke-white/80 [stroke-width:6] opacity-0" role="presentation" onpointerdown={(e) => down(e, 'line')} />
			<line x1={a.x} y1={a.y} x2={b.x} y2={b.y} class="stroke-white/80 [stroke-width:1.5]" />
			<circle cx={a.x} cy={a.y} r="9" class={knob} role="button" aria-label="Full effect" tabindex="-1" onpointerdown={(e) => down(e, 'from')} />
			<circle cx={b.x} cy={b.y} r="7" class={knob} role="button" aria-label="No effect" tabindex="-1" onpointerdown={(e) => down(e, 'to')} />
		{/if}
	{:else}
		{@const g = gradient}
		{@const c = toScreen(g.centre)}
		{@const ea = toScreen(axisEnd(g, 0))}
		{@const eb = toScreen(axisEnd(g, 1))}
		<polyline points={outline(g, 1)} class="fill-none stroke-white/80 [stroke-width:1.5]" />
		{#if g.feather > 0}<polyline points={outline(g, 1 - g.feather / 100)} class="fill-none stroke-white/60 [stroke-width:1] [stroke-dasharray:4_4]" />{/if}
		{#if c}<circle cx={c.x} cy={c.y} r="9" class={knob} role="button" aria-label="Centre" tabindex="-1" onpointerdown={(e) => down(e, 'centre')} />{/if}
		{#if ea}<circle cx={ea.x} cy={ea.y} r="7" class={knob} role="button" aria-label="Size and angle" tabindex="-1" onpointerdown={(e) => down(e, 'a')} />{/if}
		{#if eb}<circle cx={eb.x} cy={eb.y} r="7" class={knob} role="button" aria-label="Width" tabindex="-1" onpointerdown={(e) => down(e, 'b')} />{/if}
	{/if}
</svg>
