<!-- SPDX-License-Identifier: Apache-2.0
Snapshots (DESIGN.md §15, "Snapshots"): named versions of this photo's edit, taken in one click, to go back to or
to set beside the edit in the split view. A snapshot is a labelled version in the photo's history, so it is in
the sidecar with everything else and is never folded into a later step. -->
<script lang="ts">
	import type { HistoryState } from '$lib/types';

	let { history, readOnly, ontake, onopen, onsplit }: { history: HistoryState | null; readOnly: boolean; ontake: (name: string) => Promise<boolean>; onopen: (id: string) => void; onsplit: (id: string) => void } = $props();

	let naming = $state(false);
	let name = $state('');
	let busy = $state(false);

	const snapshots = $derived((history?.nodes ?? []).filter((n) => n.label).reverse());

	async function take() {
		busy = true;
		if (await ontake(name.trim())) (naming = false), (name = '');
		busy = false;
	}

	const pill = 'rounded-full bg-raised px-2.5 py-1 text-fg disabled:opacity-40';
</script>

<div class="mb-3 grid gap-1.5 text-xs">
	<div class="flex flex-wrap items-center gap-1.5">
		<span class="text-fg-2">Snapshots</span>
		<button type="button" class={pill} disabled={readOnly} aria-expanded={naming} onclick={() => ((naming = !naming), (name = `Snapshot ${snapshots.length + 1}`))}>Take snapshot</button>
	</div>
	{#if naming}
		<form class="flex gap-1.5" onsubmit={(e) => { e.preventDefault(); take(); }}>
			<label class="sr-only" for="snapshot-name">Snapshot name</label>
			<input id="snapshot-name" class="min-w-0 flex-1 rounded-md bg-raised px-2 py-1 text-fg" bind:value={name} />
			<button type="submit" class={pill} disabled={busy || !name.trim()}>{busy ? 'Saving…' : 'Keep'}</button>
		</form>
	{/if}
	{#if snapshots.length}
		<ul class="grid gap-1">
			{#each snapshots as s (s.id)}
				<li class="flex items-center gap-1.5">
					<span class="min-w-0 flex-1 truncate text-fg" title={s.created}>{s.label}{s.id === history?.head ? ' · current' : ''}</span>
					<button type="button" class="px-1.5 text-fg-2" aria-label="Open {s.label}" onclick={() => onopen(s.id)}>Open</button>
					<button type="button" class="px-1.5 text-fg-2" aria-label="{s.label} beside the edit" onclick={() => onsplit(s.id)}>Side by side</button>
				</li>
			{/each}
		</ul>
	{/if}
</div>
