<!-- SPDX-License-Identifier: Apache-2.0 -->
<script lang="ts">
	import Slider from './Slider.svelte';
	import { signed } from './format';
	import type { Band, MixerBand } from '$lib/types';

	type Props = { mixer: Record<Band, MixerBand>; onchange: (band: Band, field: keyof MixerBand, v: number) => void; oncommit: () => void };
	let { mixer, onchange, oncommit }: Props = $props();

	const bands: { band: Band; label: string; swatch: string }[] = [
		{ band: 'red', label: 'Red', swatch: '#c0392b' },
		{ band: 'orange', label: 'Orange', swatch: '#d9822b' },
		{ band: 'yellow', label: 'Yellow', swatch: '#c9b52e' },
		{ band: 'green', label: 'Green', swatch: '#3f9a4a' },
		{ band: 'aqua', label: 'Aqua', swatch: '#2f9c9c' },
		{ band: 'blue', label: 'Blue', swatch: '#3b62c4' },
		{ band: 'purple', label: 'Purple', swatch: '#7b4bb8' },
		{ band: 'magenta', label: 'Magenta', swatch: '#b8418f' }
	];
	let selected = $state<Band>('orange');
	const edited = (b: Band) => mixer[b].hue !== 0 || mixer[b].saturation !== 0 || mixer[b].luminance !== 0;
</script>

<div class="grid gap-3">
	<div class="flex justify-between" role="radiogroup" aria-label="Colour band">
		{#each bands as b (b.band)}
			<button
				type="button"
				role="radio"
				aria-checked={selected === b.band}
				aria-label={b.label}
				class="grid size-9 place-items-center rounded-full {selected === b.band ? 'ring-2 ring-fg' : ''}"
				onclick={() => (selected = b.band)}
			>
				<span class="relative block size-6 rounded-full" style:background={b.swatch}>
					{#if edited(b.band)}<span class="absolute -top-0.5 -right-0.5 size-2 rounded-full bg-fg"></span>{/if}
				</span>
			</button>
		{/each}
	</div>
	{#key selected}
		<Slider id="mix-hue" label="{bands.find((b) => b.band === selected)?.label} hue" value={mixer[selected].hue} min={-100} max={100} defaultValue={0} bipolar format={(v) => signed(v)} onchange={(v) => onchange(selected, 'hue', v)} {oncommit} />
		<Slider id="mix-sat" label="Saturation" value={mixer[selected].saturation} min={-100} max={100} defaultValue={0} bipolar format={(v) => signed(v)} onchange={(v) => onchange(selected, 'saturation', v)} {oncommit} />
		<Slider id="mix-lum" label="Luminance" value={mixer[selected].luminance} min={-100} max={100} defaultValue={0} bipolar format={(v) => signed(v)} onchange={(v) => onchange(selected, 'luminance', v)} {oncommit} />
	{/key}
</div>
