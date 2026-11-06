<!-- SPDX-License-Identifier: Apache-2.0
Virtual copies (DESIGN.md §15, "Virtual copies"): further entries for this photo — two deliverable looks from one
file — each with its own edit and history, kept in the photo's own sidecar. The photo and its copies are listed
here; a new copy starts from the version on screen. Moving between them reloads the page: the editor is built
around one key. -->
<script lang="ts">
	import type { VirtualCopy } from '$lib/state/edit-session.svelte';

	let {
		photoKey,
		fileName,
		current,
		copies,
		readOnly,
		onnew,
		onrename,
		onremove
	}: {
		photoKey: string;
		fileName: string;
		current: VirtualCopy | null;
		copies: VirtualCopy[];
		readOnly: boolean;
		onnew: (name: string) => void;
		onrename: (id: string, name: string) => void;
		onremove: (id: string) => void;
	} = $props();

	let naming = $state<'new' | string | null>(null);
	let name = $state('');
	let armed = $state<string | null>(null);

	function submit(e: SubmitEvent) {
		e.preventDefault();
		const n = name.trim();
		if (!n) return;
		if (naming === 'new') onnew(n);
		else if (naming) onrename(naming, n);
		naming = null;
	}

	const pill = 'rounded-full bg-raised px-2.5 py-1 text-fg disabled:opacity-40';
</script>

<div class="mb-3 grid gap-1.5 text-xs">
	<div class="flex flex-wrap items-center gap-1.5">
		<span class="text-fg-2">Virtual copies</span>
		<button type="button" class={pill} disabled={readOnly} aria-expanded={naming === 'new'} onclick={() => ((naming = naming === 'new' ? null : 'new'), (name = `Copy ${copies.length + 1}`))}>New virtual copy</button>
	</div>
	{#if naming}
		<form class="flex gap-1.5" onsubmit={submit}>
			<label class="sr-only" for="copy-name">Name</label>
			<input id="copy-name" class="min-w-0 flex-1 rounded-md bg-raised px-2 py-1 text-fg" bind:value={name} />
			<button type="submit" class={pill} disabled={!name.trim()}>{naming === 'new' ? 'Make it' : 'Rename'}</button>
		</form>
	{/if}
	{#if copies.length}
		<ul class="grid gap-1">
			<li class="flex items-center gap-1.5">
				{#if current}<a class="min-w-0 flex-1 truncate text-fg underline underline-offset-4" data-sveltekit-reload href="/edit/{photoKey}">{fileName}</a>
				{:else}<span class="min-w-0 flex-1 truncate text-fg">{fileName} · this one</span>{/if}
			</li>
			{#each copies as c (c.id)}
				<li class="flex items-center gap-1.5">
					{#if current?.id === c.id}<span class="min-w-0 flex-1 truncate text-fg">{c.name} · this one</span>
					{:else}<a class="min-w-0 flex-1 truncate text-fg underline underline-offset-4" data-sveltekit-reload href="/edit/{photoKey}~{c.id}">{c.name}</a>{/if}
					<button type="button" class="px-1.5 text-fg-2" disabled={readOnly} aria-label="Rename {c.name}" onclick={() => ((naming = c.id), (name = c.name))}>Rename</button>
					<button type="button" class="px-1.5 {armed === c.id ? 'text-danger' : 'text-fg-2'}" disabled={readOnly || current?.id === c.id} aria-label={armed === c.id ? `Confirm removing ${c.name}` : `Remove ${c.name}`} onclick={() => (armed === c.id ? ((armed = null), onremove(c.id)) : (armed = c.id))}>{armed === c.id ? 'Remove?' : 'Remove'}</button>
				</li>
			{/each}
		</ul>
		<p class="text-muted">A copy's versions stay in the photo's history when it is removed.</p>
	{/if}
</div>
