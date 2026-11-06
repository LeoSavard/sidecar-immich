<!-- SPDX-License-Identifier: Apache-2.0
Colour grading (DESIGN.md §7, "Colour grading"): the shadows, midtones and highlights each pushed toward a
colour, and one wheel for all of it, after the tone curve and the mixer. A wheel's colour moves only colour and
its luminance only lightness (the grade runs in OKLab). Blending is how much the three ranges overlap; Balance
moves where the shadows give way to the highlights. Everything at rest is no grading, and no field in the edit. -->
<script lang="ts">
	import Slider from '$lib/ui/Slider.svelte';
	import ColorWheel from '$lib/ui/ColorWheel.svelte';
	import { decimals, signed } from '$lib/ui/format';
	import type { ColorGrading, GradeWheel } from '$lib/types';
	import type { SectionProps } from './section';

	let { edit, onchange, oncommit }: SectionProps = $props();

	const rest: GradeWheel = { hue: 0, saturation: 0, luminance: 0 };
	const fresh = (): ColorGrading => ({ shadows: { ...rest }, midtones: { ...rest }, highlights: { ...rest }, global: { ...rest }, blending: 50, balance: 0 });
	const g = $derived(edit.colorGrading ?? fresh());
	const atRest = (c: ColorGrading) => [c.shadows, c.midtones, c.highlights, c.global].every((w) => w.saturation === 0 && w.luminance === 0);

	function set(patch: Partial<ColorGrading>) {
		onchange((e) => {
			const next = { ...(e.colorGrading ?? fresh()), ...patch };
			if (atRest(next) && next.blending === 50 && next.balance === 0) delete e.colorGrading;
			else e.colorGrading = next;
		});
	}
	const wheels: { key: 'shadows' | 'midtones' | 'highlights' | 'global'; label: string }[] = [
		{ key: 'shadows', label: 'Shadows' },
		{ key: 'midtones', label: 'Midtones' },
		{ key: 'highlights', label: 'Highlights' },
		{ key: 'global', label: 'Global' }
	];
</script>

<div class="grid gap-3">
	<div class="grid grid-cols-2 gap-x-4 gap-y-3">
		{#each wheels as w (w.key)}
			<ColorWheel id="grade-{w.key}" label={w.label} wheel={g[w.key]} onchange={(next) => set({ [w.key]: next })} {oncommit} />
		{/each}
	</div>
	<Slider id="grade-blending" label="Blending" value={g.blending} min={0} max={100} defaultValue={50} format={(v) => decimals(v)} onchange={(v) => set({ blending: v })} {oncommit} />
	<Slider id="grade-balance" label="Balance" value={g.balance} min={-100} max={100} defaultValue={0} bipolar format={(v) => signed(v)} onchange={(v) => set({ balance: v })} {oncommit} />
</div>
