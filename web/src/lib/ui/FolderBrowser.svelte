<!-- SPDX-License-Identifier: Apache-2.0
Browsing folders (DESIGN.md §3, "Browsing folders"): the folders Immich doesn't stand in front of — the editing
folder, a library given to Sidecar without Immich — listed one level at a time, and a photo opened by choosing it.
Names only: nothing is decoded or written by looking. -->
<script lang="ts">
	import { api } from '$lib/api';

	type Root = { index: number; name: string; intake: boolean };
	type Listing = { root: string; dir: string; folders: string[]; files: { name: string; size: number; modified: number; edited: boolean }[]; truncated: boolean };

	let roots = $state<Root[]>([]);
	let root = $state<number | null>(null);
	let listing = $state<Listing | null>(null);
	let busy = $state('');
	let error = $state('');

	$effect(() => {
		api('/api/folders')
			.then((r) => r.json())
			.then((r: Root[]) => (roots = r))
			.catch(() => (roots = []));
	});

	async function show(index: number, dir = '') {
		(error = ''), (root = index);
		try {
			listing = await (await api(`/api/folders/${index}/list?dir=${encodeURIComponent(dir)}`)).json();
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
	}

	async function open(name: string) {
		if (root === null || !listing) return;
		busy = name;
		try {
			const path = listing.dir ? `${listing.dir}/${name}` : name;
			const { key } = await (await api(`/api/folders/${root}/open`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ path }) })).json();
			location.href = `/edit/${key}`;
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
			busy = '';
		}
	}

	const crumbs = $derived(listing?.dir ? listing.dir.split('/') : []);
	const mb = (b: number) => `${(b / 1e6).toFixed(1)} MB`;
</script>

{#if roots.length}
	<section class="mt-8" aria-label="Folders">
		<div class="mb-3 flex flex-wrap items-baseline gap-2">
			<h2 class="text-sm font-medium text-[#c6c6c6]">Folders</h2>
			{#each roots as r (r.index)}
				<button type="button" aria-pressed={root === r.index} class="rounded-full px-2.5 py-0.5 text-xs {root === r.index ? 'bg-fg text-ground' : 'bg-[#262626] text-fg-2'}" onclick={() => show(r.index)}>{r.name}</button>
			{/each}
		</div>
		{#if listing && root !== null}
			<nav class="mb-2 flex flex-wrap items-center gap-1 text-xs text-fg-2" aria-label="Folder">
				<button type="button" class="underline underline-offset-4" onclick={() => show(root!)}>{listing.root}</button>
				{#each crumbs as c, i (i)}
					<span>/</span>
					<button type="button" class="underline underline-offset-4" onclick={() => show(root!, crumbs.slice(0, i + 1).join('/'))}>{c}</button>
				{/each}
			</nav>
			<ul class="grid gap-1 text-sm">
				{#each listing.folders as f (f)}
					<li><button type="button" class="w-full truncate rounded bg-[#262626] px-3 py-2 text-left hover:bg-[#393939]" onclick={() => show(root!, listing!.dir ? `${listing!.dir}/${f}` : f)}>📁 {f}</button></li>
				{/each}
				{#each listing.files as f (f.name)}
					<li>
						<button type="button" class="flex w-full items-center gap-2 rounded bg-[#1f1f1f] px-3 py-2 text-left hover:bg-[#393939] disabled:opacity-50" disabled={!!busy} onclick={() => open(f.name)}>
							<span class="min-w-0 flex-1 truncate">{f.name}</span>
							{#if f.edited}<span class="rounded-full bg-[#393939] px-1.5 py-0.5 text-[0.6875rem] text-fg-2" title="A sidecar is beside it">edited</span>{/if}
							<span class="text-xs text-fg-2">{busy === f.name ? 'Opening…' : mb(f.size)}</span>
						</button>
					</li>
				{/each}
			</ul>
			{#if !listing.folders.length && !listing.files.length}<p class="text-sm text-[#8d8d8d]">Nothing to open here.</p>{/if}
			{#if listing.truncated}<p class="mt-2 text-xs text-fg-2">This folder holds more than can be listed; the first 5,000 are shown.</p>{/if}
		{/if}
		{#if error}<p class="mt-2 text-sm text-danger">{error}</p>{/if}
	</section>
{/if}
