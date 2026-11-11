<!-- SPDX-License-Identifier: Apache-2.0
Sync across a selection (DESIGN.md §15, "Presets, copy and paste, sync"): the copied look, or a preset, applied
to every photo chosen, each saved as a version of its own. -->
<script lang="ts">
	import { applyToPhotos, type BatchResult } from '$lib/state/batch';
	import { describeSections, pasteLook, readLookClipboard } from '$lib/state/look-clipboard';
	import * as presets from '$lib/state/presets';
	import type { Preset } from '$lib/state/presets';

	let { keys, names, ondone, onclear }: { keys: string[]; names: Record<string, string>; ondone: (changed: string[]) => void; onclear: () => void } = $props();

	const clipboard = readLookClipboard();
	let list = $state<Preset[]>([]);
	let presetId = $state('');
	let busy = $state(false);
	let results = $state<BatchResult[] | null>(null);

	$effect(() => {
		presets.list().then((l) => (list = l)).catch(() => (list = []));
	});

	async function run(change: Parameters<typeof applyToPhotos>[1]) {
		(busy = true), (results = []);
		const done = await applyToPhotos(keys, change, (r) => (results = r));
		busy = false;
		ondone(done.filter((r) => r.ok).map((r) => r.key));
	}

	const preset = $derived(list.find((p) => p.id === presetId));
	const failed = $derived(results?.filter((r) => !r.ok) ?? []);
	const pill = 'rounded-full bg-raised px-3 py-1.5 text-fg disabled:opacity-40';
</script>

<div class="sticky bottom-[max(0.75rem,env(safe-area-inset-bottom))] mt-4 grid gap-2 rounded-lg bg-[#262626] p-3 text-sm shadow-lg" role="region" aria-label="Selected photos">
	<div class="flex flex-wrap items-center gap-2">
		<span class="text-fg-2">{keys.length} selected</span>
		<button type="button" class={pill} disabled={busy || !clipboard} title={clipboard ? `${describeSections(clipboard.sections)}, from ${clipboard.from}` : 'Copy a look in the editor first'} onclick={() => run((e) => pasteLook(e))}>Paste look</button>
		<label class="sr-only" for="batch-preset">Preset</label>
		<select id="batch-preset" class="min-w-0 rounded-md bg-raised px-2 py-1.5 text-fg" bind:value={presetId} disabled={busy || !list.length}>
			<option value="">{list.length ? 'Choose a preset…' : 'No presets yet'}</option>
			{#each list as p (p.id)}<option value={p.id}>{p.name}</option>{/each}
		</select>
		<button type="button" class={pill} disabled={busy || !preset} title={preset ? describeSections(preset.sections) : undefined} onclick={() => { const p = preset && $state.snapshot(preset); if (p) run((e) => presets.apply(e, p)); }}>Apply preset</button>
		<button type="button" class="ml-auto text-fg-2 underline underline-offset-4" disabled={busy} onclick={onclear}>Done</button>
	</div>
	{#if results}
		<p class="text-fg-2" aria-live="polite">
			{busy ? `Applying… ${results.length} of ${keys.length}` : `Applied to ${results.length - failed.length} of ${keys.length}.`}
		</p>
		{#each failed as f (f.key)}<p class="text-warning">{names[f.key] ?? f.key}: {f.reason}</p>{/each}
	{/if}
</div>
