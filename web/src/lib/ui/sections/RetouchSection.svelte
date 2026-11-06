<!-- SPDX-License-Identifier: Apache-2.0
Retouch (DESIGN.md §15, "Retouch"): heal, clone and red-eye spots. Arm a tool, tap the photo; heal and clone find
their own source, which can be dragged elsewhere. The spots are applied before everything else, in order. -->
<script lang="ts">
	import Slider from '$lib/ui/Slider.svelte';
	import { decimals } from '$lib/ui/format';
	import type { RetouchTool, SectionProps, SpotSettings } from './section';
	import type { SpotKind } from '$lib/types';

	let {
		edit,
		onchange,
		oncommit,
		tool,
		selected,
		settings,
		readOnly,
		ontool,
		onselect,
		onsettings
	}: SectionProps & {
		tool: RetouchTool;
		selected: number | null;
		settings: SpotSettings;
		readOnly: boolean;
		ontool: (t: RetouchTool) => void;
		onselect: (i: number | null) => void;
		onsettings: (s: Partial<SpotSettings>) => void;
	} = $props();

	const spots = $derived(edit.spots ?? []);
	const current = $derived(selected !== null ? spots[selected] : undefined);
	const TOOLS: { id: SpotKind; label: string }[] = [
		{ id: 'heal', label: 'Heal' },
		{ id: 'clone', label: 'Clone' },
		{ id: 'redEye', label: 'Red eye' }
	];
	const NAMES: Record<SpotKind, string> = { heal: 'Heal', clone: 'Clone', redEye: 'Red eye' };

	/** A slider sets the selected spot, and what the next one gets. */
	function set(patch: Partial<SpotSettings>) {
		onsettings(patch);
		if (selected === null || !current) return;
		onchange((e) => {
			const s = e.spots?.[selected!];
			if (s) Object.assign(s, patch);
		});
	}

	function remove(i: number) {
		onchange((e) => {
			e.spots?.splice(i, 1);
			if (!e.spots?.length) delete e.spots;
		});
		onselect(null);
		oncommit();
	}

	// Size as the slider shows it: 0…100 from 0.2 % to 10 % of the long edge, evenly in ratio.
	const toSize = (r: number) => (Math.log(r / 0.002) / Math.log(0.1 / 0.002)) * 100;
	const fromSize = (v: number) => 0.002 * (0.1 / 0.002) ** (v / 100);
	const shown = $derived(current ?? { ...settings });
</script>

<div class="grid gap-2">
	<div class="flex flex-wrap items-center gap-1.5 text-xs" role="group" aria-label="Retouch tool">
		{#each TOOLS as t (t.id)}
			<button type="button" disabled={readOnly} aria-pressed={tool === t.id} class="rounded-full px-2.5 py-1 {tool === t.id ? 'bg-fg text-ground' : 'bg-raised text-fg'}" onclick={() => ontool(tool === t.id ? 'off' : t.id)}>{t.label}</button>
		{/each}
	</div>
	<p class="text-xs text-muted">
		{#if tool === 'off'}Pick a tool, then tap the photo where something should go.{:else if tool === 'redEye'}Tap each red pupil.{:else}Tap a blemish; the source is found nearby — drag its dashed circle to take from somewhere else.{/if}
	</p>
	<Slider id="spot-size" label="Size" value={toSize(shown.radius)} min={0} max={100} defaultValue={toSize(0.02)} disabled={readOnly} format={(v) => `${decimals(fromSize(v) * 100, 1)} %`} onchange={(v) => set({ radius: fromSize(v) })} {oncommit} />
	<Slider id="spot-feather" label="Feather" value={shown.feather} min={0} max={100} defaultValue={0} disabled={readOnly} format={(v) => decimals(v)} onchange={(v) => set({ feather: v })} {oncommit} />
	<Slider id="spot-opacity" label="Opacity" value={shown.opacity} min={0} max={100} defaultValue={100} disabled={readOnly} format={(v) => `${decimals(v)} %`} onchange={(v) => set({ opacity: v })} {oncommit} />
	{#if spots.length}
		<ul class="grid gap-1 text-xs" aria-label="Spots">
			{#each spots as s, i (i)}
				<li class="flex items-center gap-2">
					<button type="button" class="flex-1 rounded px-2 py-1 text-left {selected === i ? 'bg-raised text-fg' : 'text-fg-2'}" aria-pressed={selected === i} onclick={() => onselect(selected === i ? null : i)}>{NAMES[s.kind]} {i + 1}</button>
					<button type="button" disabled={readOnly} class="rounded px-2 py-1 text-fg-2 hover:bg-raised" aria-label="Remove {NAMES[s.kind]} {i + 1}" onclick={() => remove(i)}>×</button>
				</li>
			{/each}
		</ul>
	{/if}
</div>
