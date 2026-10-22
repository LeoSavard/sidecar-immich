<!-- SPDX-License-Identifier: Apache-2.0
Light: exposure and the tone map's controls (DESIGN.md §7, modules 2 and 4). -->
<script lang="ts">
	import Slider from '$lib/ui/Slider.svelte';
	import { decimals, signed } from '$lib/ui/format';
	import { view as sliders } from '$lib/state/edit-view';
	import type { SectionProps } from './section';

	let { edit, onchange, oncommit, developed }: SectionProps & { developed: boolean } = $props();
</script>

<div class="grid gap-2">
	<!-- A developed file has no headroom past white to recover: ±2 EV is already most of what it holds. -->
	<Slider id="exposure" label="Exposure" value={edit.exposure.ev} min={developed ? -2 : -5} max={developed ? 2 : 5} step={0.01} defaultValue={0} bipolar format={(v) => `${signed(v, 2)} EV`} onchange={(v) => onchange((e) => { e.exposure.ev = v; })} {oncommit} />
	<Slider id="contrast" label="Contrast" value={sliders.contrast(edit)} min={-70} max={100} defaultValue={0} bipolar format={(v) => signed(v)} onchange={(v) => onchange((e) => { e.tone.contrast = 1 + v / 100; })} {oncommit} />
	<Slider id="highlights" label="Highlights" value={edit.tone.highlights ?? 0} min={-100} max={100} defaultValue={0} bipolar format={(v) => signed(v)} onchange={(v) => onchange((e) => { e.tone.highlights = v; })} {oncommit} />
	<Slider id="shadows" label="Shadows" value={edit.tone.shadows ?? 0} min={-100} max={100} defaultValue={0} bipolar format={(v) => signed(v)} onchange={(v) => onchange((e) => { e.tone.shadows = v; })} {oncommit} />
	<Slider id="whites" label="Whites" value={sliders.whites(edit)} min={-100} max={100} defaultValue={0} bipolar format={(v) => signed(v)} onchange={(v) => onchange((e) => { e.tone.whiteEv = e.tone.whiteZeroEv - (v / 100) * 2; })} {oncommit} />
	<Slider id="blacks" label="Blacks" value={sliders.blacks(edit)} min={-100} max={100} defaultValue={0} bipolar format={(v) => signed(v)} onchange={(v) => onchange((e) => { e.tone.blackEv = -8 - (v / 100) * 3; })} {oncommit} />
	<!-- Both answer for sensor clipping, which an already-developed file does not have: Highlight recovery
	     neutralises the cast white balance leaves on clipped pixels, Neutral highlights takes a pixel whose
	     every sample saturated to neutral at its brightest channel, and so to white. -->
	{#if !developed}
		<Slider id="hl-blend" label="Highlight recovery" value={sliders.highlightRecovery(edit)} min={0} max={100} defaultValue={50} format={(v) => `${decimals(v)} %`} onchange={(v) => onchange((e) => { e.exposure.highlightBlend = v / 100; })} {oncommit} />
		<Slider id="neutral-highlights" label="Neutral highlights" value={sliders.neutralHighlights(edit)} min={0} max={100} defaultValue={25} format={(v) => `${decimals(v)} %`} onchange={(v) => onchange((e) => { e.exposure.neutralHighlights = v / 100; })} {oncommit} />
	{/if}
</div>
