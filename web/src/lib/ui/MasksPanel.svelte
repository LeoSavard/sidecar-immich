<!-- SPDX-License-Identifier: Apache-2.0
Adjustments through the masks the models made: pick a mask (the subject, skin, hair…), optionally invert it
to reach everything else, and move exposure, contrast, saturation or white balance there alone. Each
adjustment adds to the global edit where its mask is (DESIGN.md §15). "Show" greys out what a mask leaves out. -->
<script lang="ts">
	import type { MaskAdjustment } from '$lib/types';
	import { MASK_LABELS, type MaskSet } from '$lib/masks';
	import Slider from '$lib/ui/Slider.svelte';
	import { decimals, signed } from '$lib/ui/format';

	type Props = {
		masks: MaskAdjustment[];
		/** What the photo has; `null` while the worker is still making them. */
		set: MaskSet | null;
		/** The adjustment whose mask is shown over the photo. */
		shown: number | null;
		disabled?: boolean;
		onadd: (name: string) => void;
		onremove: (index: number) => void;
		onchange: (index: number, patch: Partial<MaskAdjustment>) => void;
		oncommit: () => void;
		onshow: (index: number | null) => void;
	};
	let { masks, set, shown, disabled = false, onadd, onremove, onchange, oncommit, onshow }: Props = $props();

	const MAX = 8;
	const TEMP_TRACK = 'linear-gradient(90deg,#5f7fb6,#9aa0a6 50%,#c9a15f)';
	const TINT_TRACK = 'linear-gradient(90deg,#6f9f6f,#9aa0a6 50%,#b07aa6)';
	const label = (m: MaskAdjustment) => `${m.invert ? 'Not ' : ''}${MASK_LABELS[m.mask] ?? m.mask}`;
</script>

<div class="grid gap-3">
	{#if set === null}
		<p class="text-xs text-fg-2">Finding the subject, skin and hair in this photo… The first time on a new server takes a few minutes while the models tune themselves to its GPU.</p>
	{:else if !set.available}
		<p class="text-xs text-fg-2">Masks need the Sidecar image with the machine-learning models (the <code>-ml</code> tag); this server runs the one without them.</p>
	{:else}
		{#each masks as m, i (i)}
			<div class="grid gap-2 rounded bg-raised/50 p-2">
				<div class="flex items-center justify-between gap-2 text-xs">
					<span class="truncate font-medium text-fg">{label(m)}</span>
					<div class="flex shrink-0 gap-1">
						<button type="button" aria-pressed={shown === i} class="rounded-full px-2.5 py-1 {shown === i ? 'bg-fg text-ground' : 'bg-raised text-fg-2'}" onclick={() => onshow(shown === i ? null : i)}>Show</button>
						<button type="button" aria-pressed={m.invert} {disabled} class="rounded-full px-2.5 py-1 {m.invert ? 'bg-fg text-ground' : 'bg-raised text-fg-2'}" onclick={() => { onchange(i, { invert: !m.invert }); oncommit(); }}>Invert</button>
						<button type="button" {disabled} class="rounded-full bg-raised px-2.5 py-1 text-fg-2" aria-label="Remove {label(m)}" onclick={() => onremove(i)}>✕</button>
					</div>
				</div>
				<Slider id="mask-{i}-amount" label="Amount" value={m.amount} min={0} max={100} defaultValue={100} {disabled} format={(v) => `${decimals(v)} %`} onchange={(v) => onchange(i, { amount: v })} {oncommit} />
				<Slider id="mask-{i}-exposure" label="Exposure" value={m.exposure} min={-4} max={4} step={0.01} defaultValue={0} bipolar {disabled} format={(v) => `${signed(v, 2)} EV`} onchange={(v) => onchange(i, { exposure: v })} {oncommit} />
				<Slider id="mask-{i}-contrast" label="Contrast" value={m.contrast} min={-100} max={100} defaultValue={0} bipolar {disabled} format={(v) => signed(v)} onchange={(v) => onchange(i, { contrast: v })} {oncommit} />
				<Slider id="mask-{i}-saturation" label="Saturation" value={m.saturation} min={-100} max={100} defaultValue={0} bipolar {disabled} format={(v) => signed(v)} onchange={(v) => onchange(i, { saturation: v })} {oncommit} />
				<Slider id="mask-{i}-temperature" label="Temperature" value={m.temperature} min={-100} max={100} defaultValue={0} bipolar trackGradient={TEMP_TRACK} {disabled} format={(v) => signed(v)} onchange={(v) => onchange(i, { temperature: v })} {oncommit} />
				<Slider id="mask-{i}-tint" label="Tint" value={m.tint} min={-100} max={100} defaultValue={0} bipolar trackGradient={TINT_TRACK} {disabled} format={(v) => signed(v)} onchange={(v) => onchange(i, { tint: v })} {oncommit} />
			</div>
		{/each}
		{#if masks.length < MAX}
			<div class="flex flex-wrap items-center gap-1.5 text-xs">
				<span class="text-fg-2">Add</span>
				{#each set.names as name (name)}
					<button type="button" {disabled} class="rounded-full bg-raised px-2.5 py-1 text-fg disabled:opacity-40" onclick={() => onadd(name)}>{MASK_LABELS[name] ?? name}</button>
				{/each}
			</div>
		{/if}
		{#if set.faces.length}<p class="text-[11px] text-muted">{set.faces.length} {set.faces.length === 1 ? 'face' : 'faces'} found.</p>{/if}
	{/if}
</div>
