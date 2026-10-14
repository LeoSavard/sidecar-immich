<!-- SPDX-License-Identifier: Apache-2.0
Mask layers (DESIGN.md §15). A layer is a mask — one the models made (the subject, skin, hair…), refined with
a brush or not, or one painted from nothing — and its own light and colour: everything the global edit has,
added on top of it where the mask is. "Show" greys out what the selected mask leaves out. -->
<script lang="ts">
	import type { Band, MaskAdjustment, MixerBand } from '$lib/types';
	import { BRUSH, DEPTH, MASK_LABELS, type BrushMode, type BrushSettings, type MaskSet } from '$lib/masks';
	import { curvePointOffset, isIdentityCurve, withCurvePoint } from '$lib/state/edit-view';
	import Slider from '$lib/ui/Slider.svelte';
	import ColorMixer from '$lib/ui/ColorMixer.svelte';
	import CurveEditor from '$lib/ui/CurveEditor.svelte';
	import { decimals, signed } from '$lib/ui/format';

	type Props = {
		masks: MaskAdjustment[];
		/** The models' masks for the photo; `null` while the worker is still making them. */
		set: MaskSet | null;
		selected: number | null;
		/** The selected layer's mask is shown over the photo. */
		shown: boolean;
		brush: BrushSettings;
		/** Painting needs WebGPU: without it the photo on screen is a JPEG the server made. */
		canPaint: boolean;
		/** The selected layer's curve as it renders. */
		curveLut: Float32Array | null;
		disabled?: boolean;
		onadd: (name: string) => void;
		onremove: (index: number) => void;
		onselect: (index: number) => void;
		/** A field set to `undefined` is removed: the layer is then stored without it. */
		onchange: (index: number, patch: Partial<MaskAdjustment>) => void;
		oncommit: () => void;
		onshow: (on: boolean) => void;
		onbrush: (patch: Partial<BrushSettings>) => void;
	};
	let { masks, set, selected, shown, brush, canPaint, curveLut, disabled = false, onadd, onremove, onselect, onchange, oncommit, onshow, onbrush }: Props = $props();

	const MAX = 8;
	const TEMP_TRACK = 'linear-gradient(90deg,#5f7fb6,#9aa0a6 50%,#c9a15f)';
	const TINT_TRACK = 'linear-gradient(90deg,#6f9f6f,#9aa0a6 50%,#b07aa6)';
	const BANDS: Band[] = ['red', 'orange', 'yellow', 'green', 'aqua', 'blue', 'purple', 'magenta'];
	const zeroMixer = () => Object.fromEntries(BANDS.map((b) => [b, { hue: 0, saturation: 0, luminance: 0 }])) as Record<Band, MixerBand>;

	/** "Subject", "Not subject", and a number after the first when a mask is used twice. */
	function label(i: number): string {
		const m = masks[i];
		const base = MASK_LABELS[m.mask] ?? m.mask;
		const same = masks.slice(0, i).filter((o) => o.mask === m.mask).length;
		return `${m.invert ? 'Not ' : ''}${base}${same ? ` ${same + 1}` : ''}`;
	}

	const layer = $derived(selected !== null ? masks[selected] : undefined);
	const models = $derived(set?.available ? set.names : []);

	function setCurve(curve: [number, number][]) {
		if (selected === null) return;
		onchange(selected, { curve: isIdentityCurve(curve) ? undefined : curve });
	}

	function setMixer(band: Band, field: keyof MixerBand, v: number) {
		if (selected === null || !layer) return;
		const mixer = structuredClone(layer.mixer ?? zeroMixer());
		mixer[band][field] = v;
		const unused = BANDS.every((b) => mixer[b].hue === 0 && mixer[b].saturation === 0 && mixer[b].luminance === 0);
		onchange(selected, { mixer: unused ? undefined : mixer });
	}

	const chip = (on: boolean) => `rounded-full px-2.5 py-1 ${on ? 'bg-fg text-ground' : 'bg-raised text-fg-2'}`;
</script>

{#snippet group(title: string)}
	<p class="pt-1 text-[11px] font-medium tracking-wide text-muted uppercase">{title}</p>
{/snippet}

<div class="grid gap-3">
	{#if masks.length}
		<div class="flex flex-wrap gap-1.5 text-xs" role="listbox" aria-label="Mask layers">
			{#each masks as _, i (i)}
				<button type="button" role="option" aria-selected={selected === i} class={chip(selected === i)} onclick={() => onselect(i)}>{label(i)}</button>
			{/each}
		</div>
	{/if}

	{#if masks.length < MAX}
		<div class="flex flex-wrap items-center gap-1.5 text-xs">
			<span class="text-fg-2">Add</span>
			<button type="button" disabled={disabled || !canPaint} class="rounded-full bg-raised px-2.5 py-1 text-fg disabled:opacity-40" onclick={() => onadd(BRUSH)}>{MASK_LABELS[BRUSH]}</button>
			{#each models as name (name)}
				<button type="button" {disabled} class="rounded-full bg-raised px-2.5 py-1 text-fg disabled:opacity-40" onclick={() => onadd(name)}>{MASK_LABELS[name] ?? name}</button>
			{/each}
		</div>
		{#if set === null}
			<p class="text-xs text-fg-2">Finding the subject, skin, hair and depth in this photo… The first time on a new server takes a few minutes while the models tune themselves to its GPU.</p>
		{:else if !set.available}
			<p class="text-xs text-fg-2">Subject, skin, hair and depth masks need the Sidecar image with the machine-learning models (the <code>-ml</code> tag). Brush masks work everywhere.</p>
		{/if}
	{/if}
	{#if !canPaint}<p class="text-xs text-fg-2">Painting needs WebGPU (Safari 26 or a current Chromium); masks already painted still render here.</p>{/if}

	{#if layer && selected !== null}
		{@const i = selected}
		<div class="grid gap-2 rounded bg-raised/50 p-2">
			<div class="flex items-center justify-between gap-2 text-xs">
				<span class="truncate font-medium text-fg">{label(i)}</span>
				<div class="flex shrink-0 gap-1">
					<button type="button" aria-pressed={shown} class={chip(shown)} onclick={() => onshow(!shown)}>Show</button>
					<button type="button" aria-pressed={layer.invert} {disabled} class={chip(layer.invert)} onclick={() => { onchange(i, { invert: !layer.invert }); oncommit(); }}>Invert</button>
					<button type="button" {disabled} class="rounded-full bg-raised px-2.5 py-1 text-fg-2" aria-label="Remove {label(i)}" onclick={() => onremove(i)}>✕</button>
				</div>
			</div>
			<Slider id="mask-amount" label="Amount" value={layer.amount} min={0} max={100} defaultValue={100} {disabled} format={(v) => `${decimals(v)} %`} onchange={(v) => onchange(i, { amount: v })} {oncommit} />
			{#if layer.mask === DEPTH}
				{@const r = layer.range ?? { from: 0, to: 100, feather: 0 }}
				{@render group('Distance')}
				<p class="text-xs text-muted">0 is the farthest part of the photo, 100 the nearest. Show the mask to see the band.</p>
				<Slider id="mask-depth-near" label="Near edge" value={r.to} min={0} max={100} defaultValue={100} {disabled} format={(v) => decimals(v)} onchange={(v) => onchange(i, { range: { ...r, to: v } })} {oncommit} />
				<Slider id="mask-depth-far" label="Far edge" value={r.from} min={0} max={100} defaultValue={50} {disabled} format={(v) => decimals(v)} onchange={(v) => onchange(i, { range: { ...r, from: v } })} {oncommit} />
				<Slider id="mask-depth-feather" label="Feather" value={r.feather} min={0} max={50} defaultValue={15} {disabled} format={(v) => decimals(v)} onchange={(v) => onchange(i, { range: { ...r, feather: v } })} {oncommit} />
			{/if}

			{@render group('Brush')}
			<div class="flex gap-1 text-xs" role="radiogroup" aria-label="Brush">
				{#each [['off', 'Off'], ['paint', 'Paint'], ['erase', 'Erase']] as [mode, text] (mode)}
					<button type="button" role="radio" aria-checked={brush.mode === mode} disabled={disabled || (!canPaint && mode !== 'off')} class="{chip(brush.mode === mode)} disabled:opacity-40" onclick={() => onbrush({ mode: mode as BrushMode })}>{text}</button>
				{/each}
				{#if layer.strokes?.length}
					<button type="button" {disabled} class="ml-auto rounded-full bg-raised px-2.5 py-1 text-fg-2" onclick={() => { onchange(i, { strokes: undefined }); oncommit(); }}>Clear strokes</button>
				{/if}
			</div>
			{#if brush.mode !== 'off'}
				<p class="text-[11px] text-muted">One finger paints; two pinch and pan.</p>
				<Slider id="brush-size" label="Size" value={brush.size} min={0} max={100} defaultValue={50} format={(v) => decimals(v)} onchange={(v) => onbrush({ size: v })} oncommit={() => {}} />
				<Slider id="brush-feather" label="Feather" value={brush.feather} min={0} max={100} defaultValue={50} format={(v) => `${decimals(v)} %`} onchange={(v) => onbrush({ feather: v })} oncommit={() => {}} />
				<Slider id="brush-flow" label="Flow" value={brush.flow} min={1} max={100} defaultValue={100} format={(v) => `${decimals(v)} %`} onchange={(v) => onbrush({ flow: v })} oncommit={() => {}} />
			{/if}

			{@render group('Light')}
			<Slider id="mask-exposure" label="Exposure" value={layer.exposure} min={-4} max={4} step={0.01} defaultValue={0} bipolar {disabled} format={(v) => `${signed(v, 2)} EV`} onchange={(v) => onchange(i, { exposure: v })} {oncommit} />
			<Slider id="mask-contrast" label="Contrast" value={layer.contrast} min={-100} max={100} defaultValue={0} bipolar {disabled} format={(v) => signed(v)} onchange={(v) => onchange(i, { contrast: v })} {oncommit} />
			<Slider id="mask-highlights" label="Highlights" value={curvePointOffset(layer.curve ?? [], 'highlights')} min={-100} max={100} defaultValue={0} bipolar {disabled} format={(v) => signed(v)} onchange={(v) => setCurve(withCurvePoint(layer.curve ?? [], 'highlights', v))} {oncommit} />
			<Slider id="mask-shadows" label="Shadows" value={curvePointOffset(layer.curve ?? [], 'shadows')} min={-100} max={100} defaultValue={0} bipolar {disabled} format={(v) => signed(v)} onchange={(v) => setCurve(withCurvePoint(layer.curve ?? [], 'shadows', v))} {oncommit} />
			<Slider id="mask-whites" label="Whites" value={layer.whites ?? 0} min={-100} max={100} defaultValue={0} bipolar {disabled} format={(v) => signed(v)} onchange={(v) => onchange(i, { whites: v || undefined })} {oncommit} />
			<Slider id="mask-blacks" label="Blacks" value={layer.blacks ?? 0} min={-100} max={100} defaultValue={0} bipolar {disabled} format={(v) => signed(v)} onchange={(v) => onchange(i, { blacks: v || undefined })} {oncommit} />

			{@render group('Colour')}
			<Slider id="mask-temperature" label="Temperature" value={layer.temperature} min={-100} max={100} defaultValue={0} bipolar trackGradient={TEMP_TRACK} {disabled} format={(v) => signed(v)} onchange={(v) => onchange(i, { temperature: v })} {oncommit} />
			<Slider id="mask-tint" label="Tint" value={layer.tint} min={-100} max={100} defaultValue={0} bipolar trackGradient={TINT_TRACK} {disabled} format={(v) => signed(v)} onchange={(v) => onchange(i, { tint: v })} {oncommit} />
			<Slider id="mask-saturation" label="Saturation" value={layer.saturation} min={-100} max={100} defaultValue={0} bipolar {disabled} format={(v) => signed(v)} onchange={(v) => onchange(i, { saturation: v })} {oncommit} />
			<details class="text-xs">
				<summary class="cursor-pointer py-1 text-fg-2">Colour mixer{layer.mixer ? ' · edited' : ''}</summary>
				<div class="pt-2"><ColorMixer mixer={layer.mixer ?? zeroMixer()} onchange={setMixer} {oncommit} /></div>
			</details>
			<details class="text-xs">
				<summary class="cursor-pointer py-1 text-fg-2">Curve{isIdentityCurve(layer.curve) ? '' : ' · edited'}</summary>
				<div class="grid gap-2 pt-2">
					<CurveEditor points={layer.curve ?? [[0, 0], [1, 1]]} lut={curveLut} onchange={setCurve} {oncommit} />
					<button type="button" class="justify-self-end rounded px-2 py-1 text-fg-2 hover:bg-raised" onclick={() => { setCurve([[0, 0], [1, 1]]); oncommit(); }}>Reset curve</button>
				</div>
			</details>
		</div>
	{/if}
	{#if set?.faces.length}<p class="text-[11px] text-muted">{set.faces.length} {set.faces.length === 1 ? 'face' : 'faces'} found.</p>{/if}
</div>
