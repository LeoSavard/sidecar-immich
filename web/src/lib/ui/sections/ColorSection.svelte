<!-- SPDX-License-Identifier: Apache-2.0
Colour: global saturation, then the eight-band mixer (DESIGN.md §7, module 5). -->
<script lang="ts">
	import Slider from '$lib/ui/Slider.svelte';
	import ColorMixer from '$lib/ui/ColorMixer.svelte';
	import { signed } from '$lib/ui/format';
	import type { Band, MixerBand } from '$lib/types';
	import type { SectionProps } from './section';

	let { edit, onchange, oncommit }: SectionProps = $props();
</script>

<div class="grid gap-2">
	<Slider id="saturation" label="Saturation" value={edit.saturation ?? 0} min={-100} max={100} defaultValue={0} bipolar format={(v) => signed(v)} onchange={(v) => onchange((e) => { e.saturation = v; })} {oncommit} />
	<ColorMixer mixer={edit.mixer} onchange={(band: Band, field: keyof MixerBand, v: number) => onchange((e) => { e.mixer[band][field] = v; })} {oncommit} />
</div>
