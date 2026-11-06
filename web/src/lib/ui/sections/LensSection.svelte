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

	/** Defringe's controls (`sidecar_color::params::Defringe`): an amount and a hue band for purple and for green. */
	const DEFRINGE = { purple: 0, purpleHue: [280, 345] as [number, number], green: 0, greenHue: [120, 165] as [number, number] };
	function setFringe(patch: Partial<typeof DEFRINGE>) {
		onchange((e) => {
			const d = { ...DEFRINGE, ...e.defringe, ...patch };
			if (d.purple === 0 && d.green === 0) delete e.defringe;
			else e.defringe = d;
		});
	}
	const fringe = $derived({ ...DEFRINGE, ...edit.defringe });

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
	<p class="pt-1 text-[11px] font-medium tracking-wide text-muted uppercase">Defringe</p>
	<Slider id="fringe-purple" label="Purple" value={fringe.purple} min={0} max={100} defaultValue={0} format={(v) => decimals(v)} onchange={(v) => setFringe({ purple: v })} {oncommit} />
	{#if fringe.purple > 0}
		<Slider id="fringe-purple-from" label="Purple hues from" value={fringe.purpleHue[0]} min={0} max={360} defaultValue={280} format={(v) => `${decimals(v)}°`} onchange={(v) => setFringe({ purpleHue: [v, fringe.purpleHue[1]] })} {oncommit} />
		<Slider id="fringe-purple-to" label="to" value={fringe.purpleHue[1]} min={0} max={360} defaultValue={345} format={(v) => `${decimals(v)}°`} onchange={(v) => setFringe({ purpleHue: [fringe.purpleHue[0], v] })} {oncommit} />
	{/if}
	<Slider id="fringe-green" label="Green" value={fringe.green} min={0} max={100} defaultValue={0} format={(v) => decimals(v)} onchange={(v) => setFringe({ green: v })} {oncommit} />
	{#if fringe.green > 0}
		<Slider id="fringe-green-from" label="Green hues from" value={fringe.greenHue[0]} min={0} max={360} defaultValue={120} format={(v) => `${decimals(v)}°`} onchange={(v) => setFringe({ greenHue: [v, fringe.greenHue[1]] })} {oncommit} />
		<Slider id="fringe-green-to" label="to" value={fringe.greenHue[1]} min={0} max={360} defaultValue={165} format={(v) => `${decimals(v)}°`} onchange={(v) => setFringe({ greenHue: [fringe.greenHue[0], v] })} {oncommit} />
	{/if}
	<p class="text-xs text-muted">Takes the colour out of purple and green fringes beside high-contrast edges (the lens focusing colours at different distances), and leaves purple and green things alone. Hues are OKLab's.</p>
</div>
