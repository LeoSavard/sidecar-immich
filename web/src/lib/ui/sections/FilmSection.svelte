<!-- SPDX-License-Identifier: Apache-2.0
Film rendering (DESIGN.md §7, "Film"): each colour channel through the characteristic curve on its own, as a
colour film's layers each have their own, instead of the neutral rendering's one curve on luminance. -->
<script lang="ts">
	import Slider from '$lib/ui/Slider.svelte';
	import { decimals } from '$lib/ui/format';
	import type { SectionProps } from './section';

	let { edit, onchange, oncommit, developed }: SectionProps & { developed: boolean } = $props();

	function setFilm(on: boolean) {
		onchange((e) => {
			if (on) e.film = { amount: 100, interlayer: 30 };
			else delete e.film;
		});
		oncommit();
	}
</script>

{#if developed}
	<p class="text-xs text-fg-2">Film rendering works on a RAW's light. This picture was already rendered by whatever wrote it.</p>
{:else}
	<div class="grid gap-2">
		<div class="flex items-center gap-1.5 text-xs" role="radiogroup" aria-label="Rendering">
			<button type="button" role="radio" aria-checked={!edit.film} class="rounded-full px-2.5 py-1 {!edit.film ? 'bg-fg text-ground' : 'bg-raised text-fg-2'}" onclick={() => setFilm(false)}>Neutral</button>
			<button type="button" role="radio" aria-checked={!!edit.film} class="rounded-full px-2.5 py-1 {edit.film ? 'bg-fg text-ground' : 'bg-raised text-fg-2'}" onclick={() => setFilm(true)}>Film</button>
		</div>
		{#if edit.film}
			<Slider id="film-amount" label="Amount" value={edit.film.amount} min={0} max={100} defaultValue={100} format={(v) => `${decimals(v)} %`} onchange={(v) => onchange((e) => { if (e.film) e.film.amount = v; })} {oncommit} />
			<Slider id="film-interlayer" label="Interlayer effects" value={edit.film.interlayer} min={0} max={100} defaultValue={30} format={(v) => decimals(v)} onchange={(v) => onchange((e) => { if (e.film) e.film.interlayer = v; })} {oncommit} />
			<p class="text-xs text-muted">Each colour gets its own tone curve, so highlights fade to white and midtones gain colour. Interlayer effects spread colours apart as film's layers hold each other back; greys stay grey.</p>
		{:else}
			<p class="text-xs text-muted">Neutral keeps every colour's hue through the tone curve. Film renders each colour channel through it separately, as film does.</p>
		{/if}
	</div>
{/if}
