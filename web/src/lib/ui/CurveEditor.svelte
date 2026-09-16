<!-- SPDX-License-Identifier: Apache-2.0
Tone curve over display-encoded values. The drawn curve is the LUT the shader uses (monotone cubic, computed in
Rust), so what you see is exactly what renders. Tap empty space to add a point; double-tap a point to remove it. -->
<script lang="ts">
	type Props = { points: [number, number][]; lut: Float32Array | null; onchange: (p: [number, number][]) => void; oncommit: () => void };
	let { points, lut, onchange, oncommit }: Props = $props();

	let svg: SVGSVGElement;
	let dragging = -1;
	let lastTap = { index: -1, at: 0 };
	const S = 240;

	const path = $derived.by(() => {
		if (!lut) return '';
		let d = '';
		for (let i = 0; i <= 64; i++) {
			const x = i / 64;
			const y = lut[Math.min(lut.length - 1, Math.round(x * (lut.length - 1)))];
			d += `${i ? 'L' : 'M'}${(x * S).toFixed(1)},${((1 - y) * S).toFixed(1)}`;
		}
		return d;
	});

	function toCurve(e: PointerEvent): [number, number] {
		const r = svg.getBoundingClientRect();
		return [Math.min(1, Math.max(0, (e.clientX - r.left) / r.width)), Math.min(1, Math.max(0, 1 - (e.clientY - r.top) / r.height))];
	}

	function down(e: PointerEvent) {
		svg.setPointerCapture(e.pointerId);
		const [x, y] = toCurve(e);
		const r = svg.getBoundingClientRect();
		const hit = points.findIndex(([px, py]) => Math.hypot((px - x) * r.width, (py - y) * r.height) < 22);
		const now = performance.now();
		if (hit >= 0) {
			if (lastTap.index === hit && now - lastTap.at < 320 && hit > 0 && hit < points.length - 1) {
				onchange(points.filter((_, i) => i !== hit));
				oncommit();
				dragging = -1;
				return;
			}
			lastTap = { index: hit, at: now };
			dragging = hit;
			return;
		}
		const next = [...points, [x, y] as [number, number]].sort((a, b) => a[0] - b[0]);
		dragging = next.findIndex((p) => p[0] === x);
		onchange(next);
	}

	function move(e: PointerEvent) {
		if (dragging < 0) return;
		const [x, y] = toCurve(e);
		const next = points.map((p) => [...p] as [number, number]);
		const lo = dragging === 0 ? 0 : next[dragging - 1][0] + 0.01;
		const hi = dragging === next.length - 1 ? 1 : next[dragging + 1][0] - 0.01;
		const endpoint = dragging === 0 || dragging === next.length - 1;
		next[dragging] = [endpoint ? next[dragging][0] : Math.min(hi, Math.max(lo, x)), y];
		onchange(next);
	}

	function up() {
		if (dragging >= 0) oncommit();
		dragging = -1;
	}
</script>

<svg
	bind:this={svg}
	viewBox="0 0 {S} {S}"
	class="aspect-square w-full touch-none rounded bg-ground"
	role="img"
	aria-label="Tone curve with {points.length} points"
	onpointerdown={down}
	onpointermove={move}
	onpointerup={up}
	onpointercancel={up}
>
	{#each [0.25, 0.5, 0.75] as g}
		<line x1={g * S} x2={g * S} y1="0" y2={S} stroke="#393939" stroke-width="1" />
		<line y1={g * S} y2={g * S} x1="0" x2={S} stroke="#393939" stroke-width="1" />
	{/each}
	<line x1="0" y1={S} x2={S} y2="0" stroke="#393939" stroke-width="1" stroke-dasharray="4 4" />
	<path d={path} fill="none" stroke="#f4f4f4" stroke-width="2" />
	{#each points as [x, y], i (i)}
		<circle cx={x * S} cy={(1 - y) * S} r="7" fill="#161616" stroke="#f4f4f4" stroke-width="2" />
	{/each}
</svg>
