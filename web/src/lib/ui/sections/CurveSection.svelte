<!-- SPDX-License-Identifier: Apache-2.0
The tone curve (DESIGN.md §7, module 4): the hand-drawn curve over what Highlights and Shadows already do. -->
<script lang="ts">
	import CurveEditor from '$lib/ui/CurveEditor.svelte';
	import type { SectionProps } from './section';

	type Props = SectionProps & {
		/** The curve as it renders: the LUT the shader uses. */
		lut: Float32Array | null;
		/** What Highlights and Shadows alone do to an identity curve, drawn behind the real one. */
		ghostLut: Float32Array | null;
	};
	let { edit, onchange, oncommit, lut, ghostLut }: Props = $props();
</script>

<div class="grid gap-2">
	<CurveEditor points={edit.tone.curve} {lut} {ghostLut} onchange={(p) => onchange((e) => { e.tone.curve = p; })} {oncommit} />
	<button type="button" class="justify-self-end rounded px-2 py-1 text-xs text-fg-2 hover:bg-raised" onclick={() => { onchange((e) => { e.tone.curve = [[0, 0], [1, 1]]; }); oncommit(); }}>Reset curve</button>
</div>
