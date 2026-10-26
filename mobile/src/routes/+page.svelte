<!-- SPDX-License-Identifier: Apache-2.0
Sidecar on the phone: choose a photo, and it opens in Sidecar's one editor. The photo is imported the way the
home page imports one — into the editing folder, which Immich never sees, or the library where there is no
folder — and kept on the phone, so the editor develops it here rather than downloading the server's proxy
(docs/MOBILE.md). A RAW shows the camera's own preview from inside the file while it uploads. -->
<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '$lib/api';
	import { ACCEPT_RAW, isRaw } from '$lib/formats';
	import { destinations, importPhoto, type Destination, type Who } from '$lib/intake';
	import { showPreview, type ShownPreview } from '$lib/local/preview';

	/** The long edge the camera's preview is shown at while the photo uploads. */
	const PREVIEW_EDGE = 2560;

	let who = $state<Who | null>(null);
	let message = $state('');
	let busy = $state('');
	let fileName = $state('');
	/** A RAW's embedded preview, shown at once while the RAW uploads. */
	let preview = $state<ShownPreview | null>(null);
	/** Counts picks, so an import that finishes after another photo was chosen doesn't open the old one. */
	let picks = 0;

	/** The editing folder where there is one: the import that isn't Immich's. The library otherwise. */
	const destination = $derived<Destination | undefined>(who ? (destinations(who).includes('folder') ? 'folder' : destinations(who)[0]) : undefined);

	onMount(async () => {
		try {
			who = await (await api('/api/session')).json();
		} catch (e) {
			message = e instanceof Error ? e.message : String(e);
		}
	});

	let library = $state<HTMLInputElement | null>(null);
	let files = $state<HTMLInputElement | null>(null);

	async function chosen(e: Event) {
		const input = e.currentTarget as HTMLInputElement;
		const picked = input.files?.[0];
		input.value = '';
		if (!picked) return;
		const pick = ++picks;
		fileName = picked.name;
		message = '';
		if (preview) URL.revokeObjectURL(preview.url);
		preview = null;
		if (!destination) {
			message = 'This Sidecar has no editing folder and no library to put a photo into.';
			return;
		}
		busy = `Adding it to ${destination === 'folder' ? (who?.intakeFolder ?? 'the editing folder') : 'your library'}…`;
		// What the phone can show at once is the camera's own rendering, from inside the file.
		if (isRaw(picked.name)) showPreview(new Uint8Array(await picked.arrayBuffer()), PREVIEW_EDGE).then((p) => pick === picks && (preview = p));
		try {
			const key = await importPhoto(picked, destination);
			if (pick !== picks) return;
			// What a test asked of this page (`?inspect`, `?device`) it asks of the editor too.
			location.href = `/edit/${key}${location.search}`;
		} catch (err) {
			if (pick !== picks) return;
			busy = '';
			message = err instanceof Error ? err.message : String(err);
		}
	}
</script>

<svelte:head><title>Sidecar on this phone</title></svelte:head>

<input bind:this={library} type="file" class="sr-only" accept="image/*" onchange={chosen} />
<input bind:this={files} type="file" class="sr-only" accept={`image/*,${ACCEPT_RAW}`} onchange={chosen} />

<main class="mx-auto min-h-dvh max-w-xl bg-ground px-4 pt-[max(1.5rem,env(safe-area-inset-top))] pb-[max(1.5rem,env(safe-area-inset-bottom))] text-fg">
	<header class="flex items-baseline justify-between">
		<h1 class="text-xl">Sidecar</h1>
		<a href="/" class="text-sm text-fg-2 underline underline-offset-4">Home</a>
	</header>
	<p class="mt-2 text-sm text-fg-2">
		A photo you choose here is added to
		{#if destination === 'folder'}<span class="font-mono">{who?.intakeFolder ?? 'the editing folder'}</span>{:else}your library{/if}
		and opens in the editor, which develops it on this phone where it can.
	</p>

	<div class="mt-6 grid gap-3">
		<button type="button" class="h-14 rounded bg-raised text-base disabled:opacity-50" disabled={!!busy} onclick={() => library?.click()}>Choose from Photos</button>
		<button type="button" class="h-14 rounded bg-panel text-base disabled:opacity-50" disabled={!!busy} onclick={() => files?.click()}>Choose a file, RAW included</button>
	</div>

	{#if busy}
		<section class="mt-4 overflow-hidden rounded bg-panel text-sm">
			{#if preview}
				<img src={preview.url} alt="The camera's own preview of {fileName}" class="block max-h-[60dvh] w-full bg-ground object-contain" />
				<p class="px-3 pt-2 text-xs text-fg-2">
					The camera's own rendering, {preview.stored[0]} × {preview.stored[1]}, from inside the file: what the camera made of it, not
					Sidecar's development.
				</p>
			{/if}
			<p class="p-3" aria-live="polite"><span class="font-mono">{fileName}</span>: {busy}</p>
		</section>
	{/if}
	{#if message}<p role="status" class="mt-3 text-sm text-danger">{message}</p>{/if}
</main>
