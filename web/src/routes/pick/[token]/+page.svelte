<!-- SPDX-License-Identifier: Apache-2.0 -->
<script lang="ts">
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import { api, thumbnailUrl } from '$lib/api';

	type Candidate = { path: string; fileName: string; immichAssetId: string | null; taken: string | null };
	const token = page.params.token!;
	let query = $state('');
	let candidates = $state<Candidate[] | null>(null);
	let error = $state('');

	async function load(q?: string) {
		error = '';
		try {
			const r = await api(`/api/picks/${token}${q ? `?q=${encodeURIComponent(q)}` : ''}`);
			const body = await r.json();
			query = body.query;
			candidates = body.candidates;
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
	}

	async function choose(index: number) {
		try {
			const r = await api(`/api/picks/${token}`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ index }) });
			const { assetKey } = await r.json();
			goto(`/edit/${assetKey}`, { replaceState: true });
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
	}

	$effect(() => {
		load();
	});
</script>

<svelte:head><title>Choose a photo · Sidecar</title></svelte:head>

<main class="mx-auto min-h-dvh max-w-3xl bg-[#161616] px-4 pt-6 pb-[max(1.5rem,env(safe-area-inset-bottom))] text-[#f4f4f4]">
	<h1 class="text-lg">{candidates?.length ? 'Which photo did you mean?' : "Sidecar couldn't find that photo"}</h1>
	<form class="mt-4 flex gap-2" onsubmit={(e) => { e.preventDefault(); load(query); }}>
		<label class="sr-only" for="pick-search">File name</label>
		<input id="pick-search" bind:value={query} placeholder="File name, e.g. DSC_1234" class="h-11 flex-1 rounded bg-[#262626] px-3 outline-none focus:ring-2 focus:ring-[#f4f4f4]" />
		<button class="h-11 rounded bg-[#393939] px-4">Search</button>
	</form>
	{#if error}<p class="mt-3 text-sm text-[#ff8389]">{error}</p>{/if}
	{#if candidates}
		<ul class="mt-4 grid grid-cols-2 gap-3 sm:grid-cols-3">
			{#each candidates as c, i (c.path)}
				<li>
					<button class="w-full overflow-hidden rounded bg-[#262626] text-left hover:bg-[#393939]" onclick={() => choose(i)}>
						{#if c.immichAssetId}<img src={thumbnailUrl(c.immichAssetId)} alt="" class="aspect-[3/2] w-full object-cover" loading="lazy" />{/if}
						<span class="block truncate px-2 pt-2 text-sm">{c.fileName}</span>
						<span class="block truncate px-2 pb-2 text-xs text-[#c6c6c6]">{c.taken?.slice(0, 10) ?? c.path}</span>
					</button>
				</li>
			{:else}
				<li class="col-span-full text-sm text-[#c6c6c6]">No matches. Try the file name without its extension.</li>
			{/each}
		</ul>
	{/if}
</main>
