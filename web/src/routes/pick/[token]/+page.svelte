<!-- SPDX-License-Identifier: Apache-2.0 -->
<script lang="ts">
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import { api, thumbnailUrl } from '$lib/api';
	import { destinations, importPhoto, type Destination, type Who } from '$lib/intake';
	import { forgetShared, sharedFile } from '$lib/shared';

	type Candidate = { path: string; fileName: string; immichAssetId: string | null; taken: string | null };
	type Folder = { configured: boolean; name?: string; limit?: number; full?: boolean; nextOut?: string | null; items?: unknown[] };
	const token = page.params.token!;
	let query = $state('');
	let candidates = $state<Candidate[] | null>(null);
	let error = $state('');

	// ---------- a photo shared from this phone that the server found nothing for: offer to import it
	/** The checksum `/open` was given, which is what the share target kept the file under. */
	let sha1 = $state<string | null>(null);
	let shared = $state<File | null>(null);
	let who = $state<Who | null>(null);
	let folder = $state<Folder | null>(null);
	let to = $state<Destination>('immich');
	let importing = $state(false);
	/** The full-folder question has been answered with yes; nothing is removed before that. */
	let consented = $state(false);
	const where = $derived(destinations(who));
	const into = $derived(where.includes(to) ? to : where[0]);
	const full = $derived(into === 'folder' && !!folder?.full);

	async function offerImport(checksum: string | null) {
		sha1 = checksum;
		shared = checksum ? await sharedFile(checksum).catch(() => null) : null;
		if (!shared) return;
		[who, folder] = await Promise.all([
			api('/api/session').then((r) => r.json()),
			api('/api/intake/folder')
				.then((r) => r.json())
				.catch(() => null)
		]);
	}

	async function importShared() {
		if (!shared || !into) return;
		if (full && !consented) {
			consented = true;
			return;
		}
		importing = true;
		error = '';
		try {
			const key = await importPhoto(shared, into);
			if (sha1) await forgetShared(sha1);
			goto(`/edit/${key}`, { replaceState: true });
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
			importing = false;
		}
	}

	async function load(q?: string) {
		error = '';
		try {
			const r = await api(`/api/picks/${token}${q ? `?q=${encodeURIComponent(q)}` : ''}`);
			const body = await r.json();
			query = body.query;
			candidates = body.candidates;
			if (q === undefined) await offerImport(body.sha1 ?? null);
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
	}

	async function choose(index: number) {
		try {
			const r = await api(`/api/picks/${token}`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ index }) });
			const { assetKey } = await r.json();
			// Found on the server after all: the copy kept on the phone has done its job.
			if (sha1) await forgetShared(sha1).catch(() => {});
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

{#snippet importOffer(file: File)}
	<!-- The share sheet's own file, still on this phone. Offered first when nothing matched, and under the
	     matches otherwise, since a match by name alone may be a different photo. -->
	<section class="mt-4 rounded bg-[#262626] p-3 text-sm" aria-labelledby="import-title">
		<h2 id="import-title" class="font-medium">
			{candidates?.length ? 'None of these? Import the one you shared' : 'Import the photo you shared'}
		</h2>
		<p class="mt-1 text-[#c6c6c6]">
			<span class="font-mono">{file.name}</span>, {(file.size / 1e6).toFixed(1)} MB, is still on this phone.
			{#if into === 'immich'}Importing uploads it to your Immich library and opens it here.
			{:else if into === 'folder'}Importing writes it to <span class="font-mono">{who?.intakeFolder}</span> and opens it here. Immich never sees it.{/if}
		</p>
		{#if where.length > 1}
			<fieldset class="mt-2 flex gap-1 text-xs">
				<legend class="sr-only">Where the photo goes</legend>
				{#each [{ v: 'immich', label: 'Into my library' }, { v: 'folder', label: who?.intakeFolder ?? 'Editing folder' }] as o (o.v)}
					<label class="cursor-pointer rounded-full px-2.5 py-1 {into === o.v ? 'bg-[#f4f4f4] text-[#161616]' : 'bg-[#393939] text-[#c6c6c6]'}">
						<input type="radio" class="sr-only" name="import-to" value={o.v} checked={into === o.v} onchange={() => ((to = o.v as Destination), (consented = false))} />
						{o.label}
					</label>
				{/each}
			</fieldset>
		{/if}
		{#if !into}
			<p class="mt-2 text-[#c6c6c6]">This Sidecar has no library and no editing folder to put a photo into, so it can only open photos already on the server.</p>
		{:else if full && consented}
			<p role="alert" class="mt-2 text-[#c6c6c6]">
				{#if folder?.nextOut}
					{folder.name ?? 'The editing folder'} is full. Importing removes <span class="font-mono">{folder.nextOut}</span>, the oldest one you have
					already exported, and its edits; the exported file is unaffected.
				{:else}
					{folder?.name ?? 'The editing folder'} is full and nothing in it has been exported, so there is nothing Sidecar will remove for you.
					Export or remove a photo from the <a href="/" class="underline underline-offset-4">home page</a> first.
				{/if}
			</p>
		{/if}
		{#if into && !(full && consented && !folder?.nextOut)}
			<button class="mt-3 h-11 rounded bg-[#393939] px-4 disabled:opacity-50" disabled={importing} onclick={importShared}>
				{importing ? 'Importing…' : full && consented ? 'Remove it and import' : 'Import this photo'}
			</button>
		{/if}
	</section>
{/snippet}

<main class="mx-auto min-h-dvh max-w-3xl bg-[#161616] px-4 pt-6 pb-[max(1.5rem,env(safe-area-inset-bottom))] text-[#f4f4f4]">
	<h1 class="text-lg">{candidates?.length ? 'Which photo did you mean?' : "Sidecar couldn't find that photo"}</h1>
	<form class="mt-4 flex gap-2" onsubmit={(e) => { e.preventDefault(); load(query); }}>
		<label class="sr-only" for="pick-search">File name</label>
		<input id="pick-search" bind:value={query} placeholder="File name, e.g. DSC_1234" class="h-11 flex-1 rounded bg-[#262626] px-3 outline-none focus:ring-2 focus:ring-[#f4f4f4]" />
		<button class="h-11 rounded bg-[#393939] px-4">Search</button>
	</form>
	{#if shared && !candidates?.length}{@render importOffer(shared)}{/if}
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
	{#if shared && candidates?.length}<div class="mt-6">{@render importOffer(shared)}</div>{/if}
</main>
