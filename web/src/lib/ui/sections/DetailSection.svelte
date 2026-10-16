<!-- SPDX-License-Identifier: Apache-2.0
Detail: sharpening and noise reduction, judged in the loupe — they run at full resolution on the server, so
the proxy on screen can't show them (DESIGN.md §5, "Loupe"). -->
<script lang="ts">
	import Slider from '$lib/ui/Slider.svelte';
	import Loupe from '$lib/ui/Loupe.svelte';
	import { decimals } from '$lib/ui/format';
	import type { DisplaySpace } from '$lib/types';
	import type { SectionProps } from './section';

	type Props = SectionProps & { assetKey: string; loupeCenter: [number, number]; colorSpace: DisplaySpace };
	let { edit, onchange, oncommit, assetKey, loupeCenter, colorSpace }: Props = $props();
</script>

<div class="grid gap-2">
	<Loupe {assetKey} edit={$state.snapshot(edit)} center={loupeCenter} {colorSpace} />
	<p class="text-xs text-muted">Tap the photo to move the loupe.</p>
	<Slider id="sharpen" label="Sharpening" value={edit.detail.sharpenAmount} min={0} max={100} defaultValue={0} format={(v) => decimals(v)} onchange={(v) => onchange((e) => { e.detail.sharpenAmount = v; })} {oncommit} />
	<Slider id="sharpen-radius" label="Sharpening radius" value={edit.detail.sharpenRadius} min={0.5} max={2} step={0.1} defaultValue={1} format={(v) => `${decimals(v, 1)} px`} onchange={(v) => onchange((e) => { e.detail.sharpenRadius = v; })} {oncommit} />
	<Slider id="denoise-luma" label="Noise reduction" value={edit.detail.denoiseLuminance} min={0} max={100} defaultValue={0} format={(v) => decimals(v)} onchange={(v) => onchange((e) => { e.detail.denoiseLuminance = v; })} {oncommit} />
	<Slider id="denoise-chroma" label="Colour noise reduction" value={edit.detail.denoiseChroma} min={0} max={100} defaultValue={0} format={(v) => decimals(v)} onchange={(v) => onchange((e) => { e.detail.denoiseChroma = v; })} {oncommit} />
</div>
