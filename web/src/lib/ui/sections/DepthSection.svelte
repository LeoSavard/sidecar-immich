<!-- SPDX-License-Identifier: Apache-2.0
Lens blur and haze by distance (DESIGN.md §15, "Depth effects"), from the depth mask the models make. -->
<script lang="ts">
	import Slider from '$lib/ui/Slider.svelte';
	import { decimals, signed } from '$lib/ui/format';
	import type { SectionProps } from './section';

	type Props = SectionProps & {
		/** Whether this photo has a depth mask: null while the masks are being made. */
		available: boolean | null;
		/** Tap-to-focus waiting for a tap on the photo; undefined where it can't work (no WebGPU, read-only). */
		picking?: boolean;
		onpick?: (on: boolean) => void;
	};
	let { edit, onchange, oncommit, available, picking, onpick }: Props = $props();

	const BLUR = { amount: 0, focus: 100, range: 10 };
	const blur = $derived(edit.lensBlur ?? BLUR);

	function setBlur(patch: Partial<typeof BLUR>) {
		onchange((e) => {
			const next = { ...(e.lensBlur ?? BLUR), ...patch };
			// No blur is no field: the edit then reads as one made before lens blur existed.
			if (next.amount > 0) e.lensBlur = next;
			else delete e.lensBlur;
		});
	}
</script>

{#if available === null}
	<p class="text-xs text-fg-2">Finding how far everything in this photo is… The first time on a new server takes a few minutes while the models tune themselves to its GPU.</p>
{:else if !available}
	<p class="text-xs text-fg-2">Lens blur and haze by distance need the Sidecar image with the machine-learning models (the <code>-ml</code> tag).</p>
{:else}
	<div class="grid gap-2">
		<p class="pt-1 text-[11px] font-medium tracking-wide text-muted uppercase">Lens blur</p>
		<Slider id="blur-amount" label="Amount" value={blur.amount} min={0} max={100} defaultValue={0} format={(v) => decimals(v)} onchange={(v) => setBlur({ amount: v })} {oncommit} />
		{#if blur.amount > 0}
			<div class="flex items-center gap-1 text-xs">
				<span class="text-fg-2">Focus</span>
				<span class="flex-1"></span>
				{#if picking !== undefined}
					<button type="button" class="rounded px-2 py-1 {picking ? 'bg-fg text-ground' : 'text-fg-2 hover:bg-raised'}" aria-pressed={picking} onclick={() => onpick?.(!picking)}>Tap to focus</button>
				{/if}
			</div>
			{#if picking}<p class="text-xs text-fg-2">Tap what should be sharp. Esc cancels.</p>{/if}
			<Slider id="blur-focus" label="Focus distance" value={blur.focus} min={0} max={100} defaultValue={100} format={(v) => decimals(v)} onchange={(v) => setBlur({ focus: v })} {oncommit} />
			<Slider id="blur-range" label="Depth of field" value={blur.range} min={0} max={100} defaultValue={10} format={(v) => decimals(v)} onchange={(v) => setBlur({ range: v })} {oncommit} />
			<p class="text-xs text-muted">Focus distance runs from the farthest part of the photo (0) to the nearest (100). Depth of field is how much around it stays sharp.</p>
		{/if}
		<p class="pt-2 text-[11px] font-medium tracking-wide text-muted uppercase">Atmosphere</p>
		<Slider id="haze" label="Haze" value={edit.haze ?? 0} min={-100} max={100} defaultValue={0} bipolar format={(v) => signed(v)} onchange={(v) => onchange((e) => { if (v) e.haze = v; else delete e.haze; })} {oncommit} />
		<p class="text-xs text-muted">More haze the farther things are, toward the colour of the distance; below 0 it is taken away.</p>
	</div>
{/if}
