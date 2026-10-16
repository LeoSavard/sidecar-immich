<!-- SPDX-License-Identifier: Apache-2.0
Crop and straighten (DESIGN.md §15). The geometry is fitted by the page — it needs the picture's size and the
colour packer — so this section only asks for changes; the crop itself is drawn on the photo (CropOverlay). -->
<script lang="ts">
	import Slider from '$lib/ui/Slider.svelte';
	import { signed } from '$lib/ui/format';
	import { NO_GEOMETRY } from '$lib/state/edit-view';
	import type { EditParams, Geometry } from '$lib/types';

	type Props = {
		edit: EditParams;
		readOnly: boolean;
		/** The photo's own proportions in the output's orientation. */
		originalRatio: number;
		/** Change the geometry; the page fits the crop to it. */
		ongeometry: (patch: Partial<Geometry>) => void;
		onturn: (by: 1 | -1) => void;
		onreset: () => void;
		oncommit: () => void;
	};
	let { edit, readOnly, originalRatio, ongeometry, onturn, onreset, oncommit }: Props = $props();

	const g = $derived(edit.geometry ?? NO_GEOMETRY);
	const ratios = $derived([{ label: 'Free', value: 0 }, { label: 'Original', value: originalRatio }, { label: '1:1', value: 1 }, { label: '4:5', value: 4 / 5 }, { label: '3:2', value: 3 / 2 }, { label: '16:9', value: 16 / 9 }]);
	const once = (patch: Partial<Geometry>) => {
		ongeometry(patch);
		oncommit();
	};
</script>

<div class="grid gap-2">
	<div class="flex flex-wrap items-center gap-1.5 text-xs" role="radiogroup" aria-label="Crop proportions">
		{#each ratios as r (r.label)}
			{@const on = r.value === 0 ? !g.ratio : Math.abs(g.ratio - r.value) < 1e-3 || (r.label !== 'Original' && Math.abs(g.ratio - 1 / r.value) < 1e-3 && r.value !== 1)}
			<button type="button" role="radio" aria-checked={on} disabled={readOnly} class="rounded-full px-2.5 py-1 {on ? 'bg-fg text-ground' : 'bg-raised text-fg-2'}" onclick={() => once({ ratio: r.value })}>{r.label}</button>
		{/each}
		<button type="button" disabled={readOnly || !g.ratio || g.ratio === 1} class="rounded-full bg-raised px-2.5 py-1 text-fg-2 disabled:opacity-40" aria-label="Swap portrait and landscape" onclick={() => once({ ratio: 1 / g.ratio })}>⇄</button>
	</div>
	<div class="flex flex-wrap gap-1.5 text-xs">
		<button type="button" disabled={readOnly} class="rounded-full bg-raised px-2.5 py-1 text-fg" onclick={() => onturn(-1)}>⟲ Left</button>
		<button type="button" disabled={readOnly} class="rounded-full bg-raised px-2.5 py-1 text-fg" onclick={() => onturn(1)}>⟳ Right</button>
		<button type="button" disabled={readOnly} aria-pressed={g.flip} class="rounded-full px-2.5 py-1 {g.flip ? 'bg-fg text-ground' : 'bg-raised text-fg'}" onclick={() => once({ flip: !g.flip })}>Mirror</button>
		<button type="button" disabled={readOnly || !edit.geometry} class="ml-auto rounded-full bg-raised px-2.5 py-1 text-fg-2 disabled:opacity-40" onclick={onreset}>Reset</button>
	</div>
	<Slider id="geo-angle" label="Straighten" value={g.angle} min={-45} max={45} step={0.1} defaultValue={0} bipolar disabled={readOnly} format={(v) => `${signed(v, 1)}°`} onchange={(v) => ongeometry({ angle: v })} {oncommit} />
	<Slider id="geo-vertical" label="Vertical" value={g.vertical} min={-100} max={100} defaultValue={0} bipolar disabled={readOnly} format={(v) => signed(v)} onchange={(v) => ongeometry({ vertical: v })} {oncommit} />
	<Slider id="geo-horizontal" label="Horizontal" value={g.horizontal} min={-100} max={100} defaultValue={0} bipolar disabled={readOnly} format={(v) => signed(v)} onchange={(v) => ongeometry({ horizontal: v })} {oncommit} />
	<Slider id="geo-aspect" label="Aspect" value={g.aspect} min={-100} max={100} defaultValue={0} bipolar disabled={readOnly} format={(v) => signed(v)} onchange={(v) => ongeometry({ aspect: v })} {oncommit} />
	<p class="text-xs text-muted">Drag the frame's corners and edges to crop, or inside it to move it. Straighten and keystone keep the crop inside the photo.</p>
</div>
