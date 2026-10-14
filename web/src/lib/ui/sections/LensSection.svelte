<!-- SPDX-License-Identifier: Apache-2.0
Lens correction (DESIGN.md §15): the file's own profile or lensfun's, on or off, and each correction's strength. -->
<script lang="ts">
	import Slider from '$lib/ui/Slider.svelte';
	import { decimals } from '$lib/ui/format';
	import type { Lens, Meta } from '$lib/types';
	import type { SectionProps } from './section';

	let { edit, onchange, oncommit, info, developed }: SectionProps & { info: Meta['info']; developed: boolean } = $props();

	const profile = $derived(info.lensProfile);
	const name = $derived(info.lensIdentity?.lensId ?? info.lensIdentity?.lensModel ?? info.lens);

	function setOn(on: boolean) {
		const p = profile;
		if (!p) return;
		onchange((e) => { e.lens = on ? { profile: p.id, distortion: 100, lateralCa: 100, vignetting: 100 } : null; });
		oncommit();
	}

	function setStrength(field: keyof Omit<Lens, 'profile'>, v: number) {
		onchange((e) => {
			if (e.lens) e.lens[field] = v;
		});
	}
</script>

<div class="grid gap-2">
	{#if name}<p class="text-xs text-fg-2">{name}</p>{/if}
	{#if !profile}
		<p class="text-xs text-fg-2">
			{developed ? 'This picture was already developed, lens and all.' : 'No correction for this lens: the file carries none, and no lens database profile matches it yet.'}
		</p>
	{:else}
		<div class="flex items-center justify-between text-xs">
			<span class="text-fg-2">{profile.label}</span>
			<button type="button" aria-pressed={!!edit.lens} class="rounded-full px-3 py-1 {edit.lens ? 'bg-fg text-ground' : 'bg-raised text-fg-2'}" onclick={() => setOn(!edit.lens)}>{edit.lens ? 'On' : 'Off'}</button>
		</div>
		{#if profile.required && !edit.lens}
			<p class="text-xs text-warning">This lens is designed to be corrected in software. Without it the frame shows distortion the camera never showed.</p>
		{/if}
		{#if edit.lens}
			{#if profile.distortion}
				<Slider id="lens-distortion" label="Distortion" value={edit.lens.distortion} min={0} max={200} defaultValue={100} format={(v) => `${decimals(v)} %`} onchange={(v) => setStrength('distortion', v)} {oncommit} />
			{/if}
			{#if profile.caRed || profile.caBlue}
				<Slider id="lens-ca" label="Chromatic aberration" value={edit.lens.lateralCa} min={0} max={200} defaultValue={100} format={(v) => `${decimals(v)} %`} onchange={(v) => setStrength('lateralCa', v)} {oncommit} />
			{/if}
			{#if profile.vignetting}
				<Slider id="lens-vignetting" label="Vignetting" value={edit.lens.vignetting} min={0} max={200} defaultValue={100} format={(v) => `${decimals(v)} %`} onchange={(v) => setStrength('vignetting', v)} {oncommit} />
			{/if}
		{/if}
	{/if}
</div>
