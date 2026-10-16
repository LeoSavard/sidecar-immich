<!-- SPDX-License-Identifier: Apache-2.0
Rendering (DESIGN.md §7, "Film" and "Black and white"): neutral; film, each colour channel through the
characteristic curve on its own as a colour film's layers each have theirs; or black and white, as a film type
sees the light, through a filter. -->
<script lang="ts">
	import Slider from '$lib/ui/Slider.svelte';
	import { decimals } from '$lib/ui/format';
	import { grainMicrons } from '$lib/state/edit-view';
	import { stocks as offeredStocks } from '$lib/film';
	import type { SectionProps } from './section';

	let { edit, onchange, oncommit, developed }: SectionProps & { developed: boolean } = $props();

	type Rendering = 'neutral' | 'film' | 'mono';
	const rendering: Rendering = $derived(edit.monochrome ? 'mono' : edit.film ? 'film' : 'neutral');

	/** The film types `sidecar_color::spectral` knows, and what each does to colours. */
	const FILMS = [
		{ id: 'panchromatic', label: 'Panchromatic', note: 'Classic black-and-white film, as a class: sees every colour, a little more of blue, reds somewhat darker than the eye sees them.' },
		{ id: 'panchromatic-extended', label: 'Extended red', note: 'Modern flat-grain film, as a class: flatter, sees further into the red, so skin and reds come out lighter.' },
		{ id: 'orthochromatic', label: 'Orthochromatic', note: 'Blind to red: reds and lips go dark, skies stay bright, skin shows every freckle.' },
		{ id: 'blue-sensitive', label: 'Blue-sensitive', note: "Wet collodion and the earliest plates: sees only blue, so warm colours go near black and blue skies blow out. Real plates also saw ultraviolet, which no camera records." }
	];
	/** Named stocks, each under its film type: the maker's measured curves (`sidecar_color::stock`). */
	const STOCKS = [
		{ id: 'hp5-plus', type: 'panchromatic', label: 'ILFORD HP5 Plus', note: "HARMAN's datasheet: its spectral sensitivity and characteristic curve, a toe four stops under grey and an almost straight line above." },
		{ id: 'tri-x-400', type: 'panchromatic', label: 'KODAK TRI-X 400', note: "Kodak's datasheet (D-76): its spectral sensitivity, a short toe and a gentle shoulder well above white." },
		{ id: 'tri-x-320', type: 'panchromatic', label: 'KODAK TRI-X 320', note: "Kodak's datasheet (D-76, sheet film): the long toe it is known for, so shadows ease in softly instead of falling off. Same spectral curve as Tri-X 400, the only one the sheet gives." },
		{ id: 'sfx-200', type: 'panchromatic-extended', label: 'ILFORD SFX 200', note: "HARMAN's datasheet: sees out to 740 nm, so a red filter darkens skies hard and lightens foliage; its curve rolls off into a shoulder at the white point. Cameras record little past 700 nm." }
	];
	/** The film type a film id belongs to: a stock's type, or the id itself for a type. */
	const typeOf = (film: string) => STOCKS.find((s) => s.id === film)?.type ?? film;

	/** A film type: the type itself (its class's curve), or the stock already chosen if it is of that type. */
	function setType(type: string) {
		if (edit.monochrome && typeOf(edit.monochrome.film) === type) return;
		setMono('film', type);
	}

	const FILTERS = [
		{ id: 'none', label: 'None' },
		{ id: 'yellow', label: 'Yellow' },
		{ id: 'orange', label: 'Orange' },
		{ id: 'red', label: 'Red' },
		{ id: 'green', label: 'Green' }
	];

	function setRendering(next: Rendering) {
		// Choosing the rendering already chosen keeps its settings.
		if (next === rendering) return;
		onchange((e) => {
			if (next === 'film') e.film = { amount: 100, interlayer: 45 };
			else delete e.film;
			if (next === 'mono') e.monochrome = { film: 'panchromatic', filter: 'none' };
			else delete e.monochrome;
		});
		oncommit();
	}

	/** The scene's light, as `sidecar_color::spectral::light` knows it. */
	const LIGHTS = [
		{ id: 'daylight', label: 'Daylight' },
		{ id: 'tungsten', label: 'Tungsten' },
		{ id: 'white-balance', label: 'From white balance' }
	];

	/** The developers `sidecar_color::developer` knows, and what each does. */
	const DEVELOPERS = [
		{ id: 'standard', label: 'Standard', note: 'A general-purpose developer: the film as its datasheet shows it.' },
		{ id: 'fine-grain', label: 'Fine grain', note: 'A solvent developer: smaller, smoother grain.' },
		{ id: 'acutance', label: 'High acutance', note: 'A dilute sharp developer: crisp edges, grain shown off.' },
		{ id: 'compensating', label: 'Compensating', note: 'Two-bath or stand development: highlights held back, broad halos of local contrast at edges.' },
		{ id: 'staining', label: 'Staining', note: 'A pyro developer: highlights held and their grain hidden by the stain, edges a little sharper.' }
	];

	/**
	 * Colour reversal stocks rendered from their data sheets (`sidecar_color::slide`), offered only where the
	 * server has the two ISO standards they are measured through — without those there is nothing to render
	 * them with, so the choice is not shown at all.
	 */
	const SLIDES = [
		{
			id: 'provia-100f',
			label: 'FUJICHROME PROVIA 100F',
			note: "FUJIFILM's data sheet end to end: each layer's spectral sensitivity and characteristic curve, the dyes those densities call for, and the slide seen on a light table. It keeps a slide's density, so it reads darker than the neutral rendering — as a slide does."
		}
	];
	let available = $state<string[]>([]);
	offeredStocks().then((s) => (available = s));
	const slides = $derived(SLIDES.filter((s) => available.includes(s.id)));

	/** The stock an edit names, or nothing for the per-channel approximation. */
	function setStock(id: string) {
		onchange((e) => {
			if (!e.film) return;
			if (id) e.film.stock = id;
			else delete e.film.stock;
		});
		oncommit();
	}

	/** The scene's light for a spectral stock; daylight, the default, is left out of the edit. */
	function setFilmLight(id: string) {
		onchange((e) => {
			if (!e.film) return;
			if (id === 'daylight') delete e.film.light;
			else e.film.light = id;
		});
		oncommit();
	}

	/** Defaults are left out of the edit, as the sidecar leaves them out. */
	const MONO_DEFAULTS: Record<string, string> = { light: 'daylight', developer: 'standard' };

	function setMono(field: 'film' | 'filter' | 'light' | 'developer', value: string) {
		onchange((e) => {
			if (!e.monochrome) return;
			if (MONO_DEFAULTS[field] === value) delete e.monochrome[field as 'light' | 'developer'];
			else e.monochrome[field] = value;
		});
		oncommit();
	}

	const zone = (v: number) => (v === 0 ? 'N (normal)' : `N${v > 0 ? '+' : '−'}${decimals(Math.abs(v), 1)}`);

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
				{@const on = typeOf(mono.film) === f.id}
				{@const stocks = STOCKS.filter((s) => s.type === f.id)}
				{@const stock = STOCKS.find((s) => s.id === mono.film)}
				<button type="button" role="radio" aria-checked={on} class="rounded px-2 py-1.5 text-left {on ? 'bg-raised text-fg' : 'text-fg-2 hover:bg-raised/60'}" onclick={() => setType(f.id)}>
					<span class="font-medium">{f.label}</span>
					{#if on}<span class="block pt-0.5 text-muted">{f.note}</span>{/if}
				</button>
				{#if on && stocks.length}
					<div class="flex flex-wrap gap-1.5 pb-1 pl-2" role="radiogroup" aria-label="Stock">
						<button type="button" role="radio" aria-checked={!stock} class={choice(!stock)} onclick={() => setMono('film', f.id)}>Generic</button>
						{#each stocks as s (s.id)}
							<button type="button" role="radio" aria-checked={mono.film === s.id} class={choice(mono.film === s.id)} onclick={() => setMono('film', s.id)}>{s.label}</button>
						{/each}
					</div>
					{#if stock}<p class="pb-1 pl-2 text-muted">{stock.note}</p>{/if}
				{/if}
			{/each}
		</div>
		<p class="pt-1 text-[11px] font-medium tracking-wide text-muted uppercase">Filter</p>
		<div class="flex flex-wrap gap-1.5 text-xs" role="radiogroup" aria-label="Filter">
			{#each FILTERS as f (f.id)}
				<button type="button" role="radio" aria-checked={mono.filter === f.id} class={choice(mono.filter === f.id)} onclick={() => setMono('filter', f.id)}>{f.label}</button>
			{/each}
		</div>
		<p class="text-xs text-muted">A filter on the lens lets its own colour through: yellow and orange darken skies a little and a lot, red darkens them to drama and lightens skin, green lightens foliage and darkens lips.</p>
		<p class="pt-1 text-[11px] font-medium tracking-wide text-muted uppercase">Light</p>
		<div class="flex flex-wrap gap-1.5 text-xs" role="radiogroup" aria-label="Light">
			{#each LIGHTS as l (l.id)}
				<button type="button" role="radio" aria-checked={(mono.light ?? 'daylight') === l.id} class={choice((mono.light ?? 'daylight') === l.id)} onclick={() => setMono('light', l.id)}>{l.label}</button>
			{/each}
		</div>
		<p class="text-xs text-muted">Film has no white balance: under a tungsten bulb it gets plenty of red and little blue, so reds and skin print lighter and blues darker than in daylight. From white balance takes the light the white balance names.</p>
		<p class="pt-1 text-[11px] font-medium tracking-wide text-muted uppercase">Developer</p>
		<div class="grid gap-1 text-xs" role="radiogroup" aria-label="Developer">
			{#each DEVELOPERS as d (d.id)}
				{@const on = (mono.developer ?? 'standard') === d.id}
				<button type="button" role="radio" aria-checked={on} class="rounded px-2 py-1.5 text-left {on ? 'bg-raised text-fg' : 'text-fg-2 hover:bg-raised/60'}" onclick={() => setMono('developer', d.id)}>
					<span class="font-medium">{d.label}</span>
					{#if on}<span class="block pt-0.5 text-muted">{d.note}</span>{/if}
				</button>
			{/each}
		</div>
		<Slider id="mono-development" label="Development" value={mono.development ?? 0} min={-2} max={2} step={0.5} defaultValue={0} bipolar format={zone} onchange={(v) => onchange((e) => { if (!e.monochrome) return; if (v === 0) delete e.monochrome.development; else e.monochrome.development = v; })} {oncommit} />
		<p class="text-xs text-muted">Longer development (N+) gives the negative more contrast above the shadows and coarser grain; shorter (N−) less, for a scene too contrasty to hold.</p>
		<p class="text-xs text-muted">Each colour's spectrum is estimated from the photo, then seen as the film type would. The types are the classes' sensitivities; a named stock is its maker's measured curve.</p>
	{:else if edit.film}
		{@const stock = slides.find((s) => s.id === edit.film?.stock)}
		{#if slides.length}
			<p class="pt-1 text-[11px] font-medium tracking-wide text-muted uppercase">Colour stock</p>
			<div class="grid gap-1 text-xs" role="radiogroup" aria-label="Colour stock">
				<button type="button" role="radio" aria-checked={!stock} class="rounded px-2 py-1.5 text-left {!stock ? 'bg-raised text-fg' : 'text-fg-2 hover:bg-raised/60'}" onclick={() => setStock('')}>
					<span class="font-medium">Generic</span>
					{#if !stock}<span class="block pt-0.5 text-muted">Film's way with light, not any one stock: each channel through its own curve.</span>{/if}
				</button>
				{#each slides as s (s.id)}
					{@const on = edit.film?.stock === s.id}
					<button type="button" role="radio" aria-checked={on} class="rounded px-2 py-1.5 text-left {on ? 'bg-raised text-fg' : 'text-fg-2 hover:bg-raised/60'}" onclick={() => setStock(s.id)}>
						<span class="font-medium">{s.label}</span>
						{#if on}<span class="block pt-0.5 text-muted">{s.note}</span>{/if}
					</button>
				{/each}
			</div>
		{/if}
		<Slider id="film-amount" label="Amount" value={edit.film.amount} min={0} max={100} defaultValue={100} format={(v) => `${decimals(v)} %`} onchange={(v) => onchange((e) => { if (e.film) e.film.amount = v; })} {oncommit} />
		{#if stock}
			<p class="pt-1 text-[11px] font-medium tracking-wide text-muted uppercase">The scene's light</p>
			<div class="flex flex-wrap items-center gap-1.5 text-xs" role="radiogroup" aria-label="The scene's light">
				{#each LIGHTS as l (l.id)}
					{@const on = (edit.film?.light ?? 'daylight') === l.id}
					<button type="button" role="radio" aria-checked={on} class={choice(on)} onclick={() => setFilmLight(l.id)}>{l.label}</button>
				{/each}
			</div>
			<p class="text-xs text-muted">Colour film is not white balanced either: it sees the surface's own colour times the light. The stock's own curves set the tones, so Contrast, Blacks and the curve reach the picture only through the neutral rendering Amount mixes in.</p>
		{:else}
			<Slider id="film-interlayer" label="Interlayer effects" value={edit.film.interlayer} min={0} max={100} defaultValue={45} format={(v) => decimals(v)} onchange={(v) => onchange((e) => { if (e.film) e.film.interlayer = v; })} {oncommit} />
			<p class="text-xs text-muted">Each colour gets its own tone curve, so highlights fade to white and midtones gain colour. Interlayer effects spread colours apart as film's layers hold each other back; greys stay grey.</p>
		{/if}
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
