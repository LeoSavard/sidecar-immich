<!-- SPDX-License-Identifier: Apache-2.0
White balance (DESIGN.md §7, module 1). The sliders read as-shot until moved; moving one makes it custom. -->
<script lang="ts">
	import Slider from '$lib/ui/Slider.svelte';
	import { decimals, signed } from '$lib/ui/format';
	import type { SectionProps } from './section';

	type Props = SectionProps & {
		developed: boolean;
		/**
		 * What the camera recorded, resolved once per photo: the Temperature slider's detent. It must not follow
		 * the edit — it did, and the detent then sat under the thumb, catching it every few pixels of a drag.
		 */
		asShot: { temperatureK: number; tint: number };
		/** The eyedropper waiting for a tap on the photo; undefined where it can't work (no WebGPU, read-only). */
		picking?: boolean;
		onpick?: (on: boolean) => void;
	};
	let { edit, onchange, oncommit, developed, asShot, picking, onpick }: Props = $props();

	const TEMP_TRACK = 'linear-gradient(90deg,#5f7fb6,#9aa0a6 45%,#c9a15f)';
	const TINT_TRACK = 'linear-gradient(90deg,#6f9f6f,#9aa0a6 50%,#b07aa6)';
	const asShotLabel = $derived(developed ? 'as saved' : 'as shot');

	function set(field: 'temperatureK' | 'tint', value: number) {
		onchange((e) => {
			if (e.whiteBalance.mode === 'asShot') {
				e.whiteBalance.temperatureK = Math.round(asShot.temperatureK);
				e.whiteBalance.tint = Math.round(asShot.tint);
				e.whiteBalance.mode = 'custom';
			}
			e.whiteBalance[field] = value;
		});
	}
</script>

<div class="grid gap-2">
	<div class="flex items-center gap-1 text-xs">
		<span class="text-fg-2">{edit.whiteBalance.mode === 'asShot' ? (developed ? 'As saved' : 'As shot') : 'Custom'}</span>
		<span class="flex-1"></span>
		{#if picking !== undefined}
			<button type="button" class="rounded p-1 {picking ? 'bg-fg text-ground' : 'text-fg-2 hover:bg-raised'}" aria-pressed={picking} aria-label="Pick a neutral" title="Pick something grey or white in the photo" onclick={() => onpick?.(!picking)}>
				<svg viewBox="0 0 24 24" class="h-4 w-4" fill="currentColor" aria-hidden="true"><rect x="8" y="2.4" width="8" height="5.2" rx="1.6" /><path d="M10.2 8.8h3.6l-.9 7.5L12 21.6l-.9-5.3z" /></svg>
			</button>
		{/if}
		<!-- Normal text colour while it can do something, muted when there is nothing to reset: grey either way read as
		     permanently disabled (the October interface review, item 16). -->
		<button type="button" class="rounded px-2 py-1 hover:bg-raised disabled:opacity-40 {edit.whiteBalance.mode === 'asShot' ? 'text-fg-2' : 'text-fg'}" disabled={edit.whiteBalance.mode === 'asShot'} onclick={() => { onchange((e) => { e.whiteBalance.mode = 'asShot'; }); oncommit(); }}>Reset to {asShotLabel}</button>
	</div>
	{#if picking}<p class="text-xs text-fg-2">Tap something in the photo that should be grey or white. Esc cancels.</p>{/if}
	<Slider id="wb-temp" label="Temperature" value={edit.whiteBalance.mode === 'asShot' ? Math.round(asShot.temperatureK) : edit.whiteBalance.temperatureK} min={2000} max={12000} step={10} defaultValue={Math.round(asShot.temperatureK)} trackGradient={TEMP_TRACK} format={(v) => `${decimals(v)} K`} onchange={(v) => set('temperatureK', v)} {oncommit} />
	<Slider id="wb-tint" label="Tint" value={edit.whiteBalance.mode === 'asShot' ? Math.round(asShot.tint) : edit.whiteBalance.tint} min={-150} max={150} defaultValue={0} bipolar trackGradient={TINT_TRACK} format={(v) => signed(v)} onchange={(v) => set('tint', v)} {oncommit} />
</div>
