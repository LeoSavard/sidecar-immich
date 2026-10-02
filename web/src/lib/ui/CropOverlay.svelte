<!-- SPDX-License-Identifier: Apache-2.0
The crop tool over the photo: the crop's frame with a rule-of-thirds grid, everything outside it dimmed, and
handles to resize it (corners and edges) or move it (inside). Works in the output's own coordinates (0…1 of
what is shown, already turned); `accept` says whether a crop still lies inside the straightened picture, and a
drag stops at the last place that does. -->
<script lang="ts">
	type Rect = { x: number; y: number; w: number; h: number };
	type C = [number, number, number, number];
	type Props = {
		/** Where the output is on screen, CSS pixels relative to the overlay's container. */
		rect: Rect;
		/** left, top, right, bottom in the output, 0…1. */
		crop: C;
		/** Width ÷ height to hold, in screen proportions; 0 is free. */
		ratio: number;
		accept: (c: C) => boolean;
		onchange: (c: C) => void;
		oncommit: () => void;
	};
	let { rect, crop, ratio, accept, onchange, oncommit }: Props = $props();

	type Grip = 'nw' | 'n' | 'ne' | 'e' | 'se' | 's' | 'sw' | 'w' | 'move';
	let drag: { grip: Grip; start: C; at: [number, number]; last: C } | null = null;
	let svg: SVGSVGElement;
	/** Smallest crop side, CSS pixels. */
	const MIN = 32;

	const px = $derived({ l: rect.x + crop[0] * rect.w, t: rect.y + crop[1] * rect.h, r: rect.x + crop[2] * rect.w, b: rect.y + crop[3] * rect.h });

	function point(e: PointerEvent): [number, number] {
		const r = svg.getBoundingClientRect();
		return [(e.clientX - r.left - rect.x) / rect.w, (e.clientY - r.top - rect.y) / rect.h];
	}

	/** The crop a grip asks for with the pointer at `p`, before it is checked against the picture. */
	function wanted(grip: Grip, s: C, p: [number, number], d: [number, number]): C {
		let [l, t, r, b] = s;
		const minW = MIN / rect.w;
		const minH = MIN / rect.h;
		if (grip === 'move') {
			const dx = Math.min(Math.max(d[0], -l), 1 - r);
			const dy = Math.min(Math.max(d[1], -t), 1 - b);
			return [l + dx, t + dy, r + dx, b + dy];
		}
		const cx = Math.min(Math.max(p[0], 0), 1);
		const cy = Math.min(Math.max(p[1], 0), 1);
		if (grip.includes('w')) l = Math.min(cx, r - minW);
		if (grip.includes('e')) r = Math.max(cx, l + minW);
		if (grip.includes('n')) t = Math.min(cy, b - minH);
		if (grip.includes('s')) b = Math.max(cy, t + minH);
		if (ratio > 0) {
			// Screen proportions: a crop's width over its height in pixels.
			const k = ratio * (rect.h / rect.w);
			if (grip === 'n' || grip === 's') {
				const w = (b - t) * k;
				const mid = (s[0] + s[2]) / 2;
				[l, r] = [mid - w / 2, mid + w / 2];
			} else if (grip === 'e' || grip === 'w') {
				const h = (r - l) / k;
				const mid = (s[1] + s[3]) / 2;
				[t, b] = [mid - h / 2, mid + h / 2];
			} else {
				// A corner: the larger of the two sizes the pointer implies, from the opposite corner.
				const w = Math.max(r - l, (b - t) * k);
				const h = w / k;
				if (grip.includes('w')) l = r - w;
				else r = l + w;
				if (grip.includes('n')) t = b - h;
				else b = t + h;
			}
		}
		return [l, t, r, b];
	}

	const inside = (c: C) => c[0] >= -1e-6 && c[1] >= -1e-6 && c[2] <= 1 + 1e-6 && c[3] <= 1 + 1e-6 && accept(c);

	function down(e: PointerEvent, grip: Grip) {
		e.stopPropagation();
		e.preventDefault();
		(e.currentTarget as Element).setPointerCapture(e.pointerId);
		drag = { grip, start: [...crop] as C, at: point(e), last: [...crop] as C };
	}

	function move(e: PointerEvent) {
		if (!drag) return;
		const p = point(e);
		const next = wanted(drag.grip, drag.start, p, [p[0] - drag.at[0], p[1] - drag.at[1]]);
		let found = drag.last;
		if (inside(next)) found = next;
		else {
			// As far toward the pointer as the picture allows.
			let [lo, hi] = [0, 1];
			for (let i = 0; i < 20; i++) {
				const mid = (lo + hi) / 2;
				const c = drag.last.map((v, j) => v + (next[j] - v) * mid) as C;
				if (inside(c)) [lo, found] = [mid, c];
				else hi = mid;
			}
		}
		drag.last = found;
		onchange(found);
	}

	function up() {
		if (!drag) return;
		drag = null;
		oncommit();
	}

	const grips: { grip: Grip; x: (p: typeof px) => number; y: (p: typeof px) => number; cursor: string }[] = [
		{ grip: 'nw', x: (p) => p.l, y: (p) => p.t, cursor: 'nwse-resize' },
		{ grip: 'n', x: (p) => (p.l + p.r) / 2, y: (p) => p.t, cursor: 'ns-resize' },
		{ grip: 'ne', x: (p) => p.r, y: (p) => p.t, cursor: 'nesw-resize' },
		{ grip: 'e', x: (p) => p.r, y: (p) => (p.t + p.b) / 2, cursor: 'ew-resize' },
		{ grip: 'se', x: (p) => p.r, y: (p) => p.b, cursor: 'nwse-resize' },
		{ grip: 's', x: (p) => (p.l + p.r) / 2, y: (p) => p.b, cursor: 'ns-resize' },
		{ grip: 'sw', x: (p) => p.l, y: (p) => p.b, cursor: 'nesw-resize' },
		{ grip: 'w', x: (p) => p.l, y: (p) => (p.t + p.b) / 2, cursor: 'ew-resize' }
	];
</script>

<svg bind:this={svg} class="absolute inset-0 h-full w-full touch-none" role="presentation" onpointermove={move} onpointerup={up} onpointercancel={up}>
	<path
		d="M{rect.x},{rect.y}h{rect.w}v{rect.h}h{-rect.w}Z M{px.l},{px.t}V{px.b}H{px.r}V{px.t}Z"
		fill="black"
		fill-opacity="0.55"
		fill-rule="evenodd"
		class="pointer-events-none"
	/>
	<rect
		x={px.l}
		y={px.t}
		width={px.r - px.l}
		height={px.b - px.t}
		fill="transparent"
		stroke="#f4f4f4"
		stroke-width="1.5"
		class="cursor-move"
		role="slider"
		aria-label="Crop: drag to move"
		aria-valuenow={0}
		tabindex="-1"
		onpointerdown={(e) => down(e, 'move')}
	/>
	{#each [1, 2] as k (k)}
		<line x1={px.l + ((px.r - px.l) * k) / 3} x2={px.l + ((px.r - px.l) * k) / 3} y1={px.t} y2={px.b} stroke="#f4f4f4" stroke-opacity="0.45" stroke-width="1" class="pointer-events-none" />
		<line y1={px.t + ((px.b - px.t) * k) / 3} y2={px.t + ((px.b - px.t) * k) / 3} x1={px.l} x2={px.r} stroke="#f4f4f4" stroke-opacity="0.45" stroke-width="1" class="pointer-events-none" />
	{/each}
	{#each grips as g (g.grip)}
		<circle
			cx={g.x(px)}
			cy={g.y(px)}
			r={g.grip.length === 2 ? 9 : 7}
			fill="#f4f4f4"
			stroke="black"
			stroke-opacity="0.5"
			style="cursor: {g.cursor}"
			role="slider"
			aria-label="Crop {g.grip} edge"
			aria-valuenow={0}
			tabindex="-1"
			onpointerdown={(e) => down(e, g.grip)}
		/>
		<!-- A larger, invisible target for a finger. -->
		<circle cx={g.x(px)} cy={g.y(px)} r="22" fill="transparent" style="cursor: {g.cursor}" role="presentation" onpointerdown={(e) => down(e, g.grip)} />
	{/each}
</svg>
