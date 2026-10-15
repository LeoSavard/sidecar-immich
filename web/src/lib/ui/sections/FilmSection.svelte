<!-- SPDX-License-Identifier: Apache-2.0
Rendering (DESIGN.md §7, "Film" and "Black and white"): neutral; film, each colour channel through the
characteristic curve on its own as a colour film's layers each have theirs; or black and white, as a film type
sees the light, through a filter. -->
<script lang="ts">
	import Slider from '$lib/ui/Slider.svelte';
	import { decimals } from '$lib/ui/format';
	import { grainMicrons } from '$lib/state/edit-view';
	import type { SectionProps } from './section';

	let { edit, onchange, oncommit, developed }: SectionProps & { developed: boolean } = $props();

	type Rendering = 'neutral' | 'film' | 'mono';
	const rendering: Rendering = $derived(edit.monochrome ? 'mono' : edit.film ? 'film' : 'neutral');

	/** The film types `sidecar_color::spectral` knows, and what each does to colours. */
	const FILMS = [
		{ id: 'panchromatic', label: 'Panchromatic', note: 'Classic black-and-white film (the HP5 and Tri-X kind): sees every colour, a little more of blue, reds somewhat darker than the eye sees them.' },
		{ id: 'panchromatic-extended', label: 'Extended red', note: 'Modern T-grain film: flatter, sees further into the red, so skin and reds come out lighter.' },
		{ id: 'orthochromatic', label: 'Orthochromatic', note: 'Blind to red: reds and lips go dark, skies stay bright, skin shows every freckle.' },
		{ id: 'blue-sensitive', label: 'Blue-sensitive', note: "Wet collodion and the earliest plates: sees only blue, so warm colours go near black and blue skies blow out. Real plates also saw ultraviolet, which no camera records." }
	];
	const FILTERS = [
		{ id: 'none', label: 'None' },
		{ id: 'yellow', label: 'Yellow' },
		{ id: 'orange', label: 'Orange' },
		{ id: 'red', label: 'Red' },
		{ id: 'green', label: 'Green' }
	];

	function setRendering(next: Rendering) {
		onchange((e) => {
			if (next === 'film') e.film = { amount: 100, interlayer: 45 };
			else delete e.film;
			if (next === 'mono') e.monochrome = { film: 'panchromatic', filter: 'none' };
			else delete e.monochrome;
		});
		oncommit();
	}

	function setMono(field: 'film' | 'filter', value: string) {
		onchange((e) => {
			if (e.monochrome) e.monochrome[field] = value;
		});
		oncommit();
	}

	/** Grain, kept while its size is set even at amount 0, so the size isn't lost on the way back up. */
	function setGrain(field: 'amount' | 'size', v: number) {
		onchange((e) => {
			const g = { amount: e.grain?.amount ?? 0, size: e.grain?.size ?? 50, [field]: v };
			if (g.amount === 0 && g.size === 50) delete e.grain;
			else e.grain = g;
		});
	}

	const choice = (on: boolean) => `rounded-full px-2.5 py-1 ${on ? 'bg-fg text-ground' : 'bg-raised text-fg-2'}`;
</script>

<div class="grid gap-2">
	<div class="flex flex-wrap items-center gap-1.5 text-xs" role="radiogroup" aria-label="Rendering">
		<button type="button" role="radio" aria-checked={rendering === 'neutral'} class={choice(rendering === 'neutral')} onclick={() => setRendering('neutral')}>{developed ? 'Colour' : 'Neutral'}</button>
		{#if !developed}
			<button type="button" role="radio" aria-checked={rendering === 'film'} class={choice(rendering === 'film')} onclick={() => setRendering('film')}>Film</button>
		{/if}
		<button type="button" role="radio" aria-checked={rendering === 'mono'} class={choice(rendering === 'mono')} onclick={() => setRendering('mono')}>Black &amp; white</button>
	</div>
	{#if edit.monochrome}
		{@const mono = edit.monochrome}
		<p class="pt-1 text-[11px] font-medium tracking-wide text-muted uppercase">Film type</p>
		<div class="grid gap-1 text-xs" role="radiogroup" aria-label="Film type">
			{#each FILMS as f (f.id)}
				<button type="button" role="radio" aria-checked={mono.film === f.id} class="rounded px-2 py-1.5 text-left {mono.film === f.id ? 'bg-raised text-fg' : 'text-fg-2 hover:bg-raised/60'}" onclick={() => setMono('film', f.id)}>
					<span class="font-medium">{f.label}</span>
					{#if mono.film === f.id}<span class="block pt-0.5 text-muted">{f.note}</span>{/if}
				</button>
			{/each}
		</div>
		<p class="pt-1 text-[11px] font-medium tracking-wide text-muted uppercase">Filter</p>
		<div class="flex flex-wrap gap-1.5 text-xs" role="radiogroup" aria-label="Filter">
			{#each FILTERS as f (f.id)}
				<button type="button" role="radio" aria-checked={mono.filter === f.id} class={choice(mono.filter === f.id)} onclick={() => setMono('filter', f.id)}>{f.label}</button>
			{/each}
		</div>
		<p class="text-xs text-muted">A filter on the lens lets its own colour through: yellow and orange darken skies a little and a lot, red darkens them to drama and lightens skin, green lightens foliage and darkens lips.</p>
		<p class="text-xs text-muted">Each colour's spectrum is estimated from the photo, then seen as the film type would. The types are the classes' sensitivities, not any one brand's measured curve.</p>
	{:else if edit.film}
		<Slider id="film-amount" label="Amount" value={edit.film.amount} min={0} max={100} defaultValue={100} format={(v) => `${decimals(v)} %`} onchange={(v) => onchange((e) => { if (e.film) e.film.amount = v; })} {oncommit} />
		<Slider id="film-interlayer" label="Interlayer effects" value={edit.film.interlayer} min={0} max={100} defaultValue={45} format={(v) => decimals(v)} onchange={(v) => onchange((e) => { if (e.film) e.film.interlayer = v; })} {oncommit} />
		<p class="text-xs text-muted">Each colour gets its own tone curve, so highlights fade to white and midtones gain colour. Interlayer effects spread colours apart as film's layers hold each other back; greys stay grey.</p>
	{:else if developed}
		<p class="text-xs text-muted">This picture was already rendered by whatever wrote it, so film rendering, which works on a RAW's light, isn't offered. Black and white is.</p>
	{:else}
		<p class="text-xs text-muted">Neutral keeps every colour's hue through the tone curve. Film renders each colour channel through it separately, as film does. Black and white sees the light as a film type would.</p>
	{/if}
	<p class="pt-1 text-[11px] font-medium tracking-wide text-muted uppercase">Grain</p>
	<Slider id="grain-amount" label="Amount" value={edit.grain?.amount ?? 0} min={0} max={100} defaultValue={0} format={(v) => decimals(v)} onchange={(v) => setGrain('amount', v)} {oncommit} />
	<Slider id="grain-size" label="Size" value={edit.grain?.size ?? 50} min={0} max={100} defaultValue={50} format={(v) => `${Math.round(grainMicrons(v))} µm`} onchange={(v) => setGrain('size', v)} {oncommit} />
	<p class="text-xs text-muted">Size is a grain on a 35 mm negative, so it keeps its share of the picture at any size. The preview, smaller than the photo, shows grain softer than the export does; zoom in to see it as exported.</p>
</div>
