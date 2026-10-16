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
	};
	let { edit, onchange, oncommit, developed, asShot }: Props = $props();

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
	<div class="flex items-center justify-between text-xs">
		<span class="text-fg-2">{edit.whiteBalance.mode === 'asShot' ? (developed ? 'As saved' : 'As shot') : 'Custom'}</span>
		<button type="button" class="rounded px-2 py-1 text-fg-2 hover:bg-raised disabled:opacity-40" disabled={edit.whiteBalance.mode === 'asShot'} onclick={() => { onchange((e) => { e.whiteBalance.mode = 'asShot'; }); oncommit(); }}>Reset to {asShotLabel}</button>
	</div>
	<Slider id="wb-temp" label="Temperature" value={edit.whiteBalance.mode === 'asShot' ? Math.round(asShot.temperatureK) : edit.whiteBalance.temperatureK} min={2000} max={12000} step={10} defaultValue={Math.round(asShot.temperatureK)} trackGradient={TEMP_TRACK} format={(v) => `${decimals(v)} K`} onchange={(v) => set('temperatureK', v)} {oncommit} />
	<Slider id="wb-tint" label="Tint" value={edit.whiteBalance.mode === 'asShot' ? Math.round(asShot.tint) : edit.whiteBalance.tint} min={-150} max={150} defaultValue={0} bipolar trackGradient={TINT_TRACK} format={(v) => signed(v)} onchange={(v) => set('tint', v)} {oncommit} />
</div>
