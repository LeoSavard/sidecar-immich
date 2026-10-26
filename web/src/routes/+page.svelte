<!-- SPDX-License-Identifier: Apache-2.0 -->
<script lang="ts">
	import { api, thumbnailUrl } from '$lib/api';
	import { importPhoto, type Who } from '$lib/intake';
	import { ACCEPT_RAW } from '$lib/formats';

	type Recent = { assetKey: string; fileName: string; immichAssetId: string | null; openedAt: string };
	let recent = $state<Recent[] | null>(null);
	let who = $state<Who | null>(null);

	$effect(() => {
		(async () => {
			who = await (await api('/api/session')).json();
			recent = await (await api('/api/recent')).json();
			await loadFolder();
		})().catch(() => {});
	});

	// ---------- the editing folder, which is bounded on purpose
	type FolderItem = { assetKey: string; fileName: string; at: string; bytes: number; modifiedUnix: number; exported: boolean; edited: boolean };
	type Folder = { configured: boolean; name?: string; limit?: number; full?: boolean; nextOut?: string | null; items?: FolderItem[] };
	let folder = $state<Folder | null>(null);
	/** The photo whose Remove button is armed: removing takes two taps, and the first one only says so. */
	let armed = $state<string | null>(null);
	let armedTimer: ReturnType<typeof setTimeout> | undefined;
	/** The full-folder message stands between the import button and the file picker. */
	let blocked = $state(false);
	let folderBusy = $state('');

	async function loadFolder() {
		try {
			folder = await (await api('/api/intake/folder')).json();
		} catch {
			folder = null;
		}
	}

	function arm(key: string) {
		clearTimeout(armedTimer);
		armed = key;
		// A tap that was meant for something else shouldn't leave a loaded button behind.
		armedTimer = setTimeout(() => (armed = null), 4000);
	}

	async function remove(item: FolderItem) {
		clearTimeout(armedTimer);
		armed = null;
		folderBusy = item.assetKey;
		try {
			await api(`/api/intake/folder/${item.assetKey}`, { method: 'DELETE' });
			await Promise.all([loadFolder(), (async () => (recent = await (await api('/api/recent')).json()))()]);
			blocked = false;
		} catch (e) {
			intakeError = e instanceof Error ? e.message : String(e);
		}
		folderBusy = '';
	}

	const mb = (n: number) => `${(n / 1e6).toFixed(1)} MB`;

	/** Sidecar's own thumbnail: the photo as the edit renders it, for anything Immich can't or won't show. */
	const ownThumb = (assetKey: string) => `/api/assets/${assetKey}/thumb`;

	/**
	 * Immich's thumbnail where there is one, since it is already made; Sidecar's own otherwise. `onerror`
	 * covers the other half of the problem — a photo Immich holds but has no thumbnail for, an iPhone DNG
	 * among them — by falling back to the same place rather than leaving a grey box.
	 */
	function thumbFallback(e: Event, assetKey: string) {
		const el = e.currentTarget as HTMLImageElement;
		if (el.dataset.fell === '1') return;
		el.dataset.fell = '1';
		el.src = ownThumb(assetKey);
	}

	/** What is in the editing folder, so Recent doesn't show the same photo twice. */
	const importedKeys = $derived(new Set(folder?.items?.map((i) => i.assetKey) ?? []));
	const recentElsewhere = $derived(recent?.filter((r) => !importedKeys.has(r.assetKey)) ?? null);

	let link = $state('');
	let linkError = $state('');

	const UUID = '[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}';

	/**
	 * The photo's own id in an Immich link. Several of Immich's views put the id of whatever was being
	 * browsed in front of it — `/people/<person>/photos/<asset>`, an album, a shared link — so the first id
	 * in the link is not the photo. The one after `/photo/` or `/photos/` is; in a shape this doesn't know,
	 * the photo's id is the last one (`?assetId=`, `?at=`), which is still better than the first.
	 */
	function assetIdIn(url: string): string | undefined {
		const path = url.match(new RegExp(`/photos?/(${UUID})`, 'i'));
		if (path) return path[1];
		const all = url.match(new RegExp(UUID, 'gi'));
		return all?.[all.length - 1];
	}

	function openLink(e: SubmitEvent) {
		e.preventDefault();
		const id = assetIdIn(link);
		if (!id) {
			linkError = "That doesn't look like an Immich photo link. Copy it from the photo's page in Immich.";
			return;
		}
		location.href = `/open?immich=${id}`;
	}

	// ---------- a photo from this device: into the library or the editing folder, then opened like any other
	let intakeName = $state('');
	let intakeError = $state('');
	let dragging = $state(false);
	/** Where an imported photo goes. The library when there is one, since that is what this always did. */
	let intakeTo = $state<'immich' | 'folder'>('immich');
	const canChoose = $derived(!!who?.immich && !!who?.intakeFolder);
	/** Settled once the session says what this deployment has: one destination needs no question asked. */
	const destination = $derived(who?.immich ? intakeTo : 'folder');

	async function intake(file: File) {
		intakeName = file.name;
		intakeError = '';
		try {
			location.href = `/edit/${await importPhoto(file, destination)}`;
		} catch (e) {
			intakeError = e instanceof Error ? e.message : String(e);
			intakeName = '';
		}
	}

	function chosen(e: Event) {
		const input = e.currentTarget as HTMLInputElement;
		const file = input.files?.[0];
		// Cleared so choosing the same file twice in a row still fires a change event.
		input.value = '';
		if (file) intake(file);
	}

	let picker = $state<HTMLInputElement | null>(null);

	/**
	 * Is there room for one more? Only the editing folder is bounded, so a library import is never stopped.
	 * The server asks the same question when the bytes arrive; this is so the answer comes before the file
	 * picker rather than after a photo has been chosen and uploaded.
	 */
	const noRoom = $derived(destination === 'folder' && !!folder?.full);

	/** The import button: the message first when the folder is full, the picker only once it is dismissed. */
	function choose() {
		if (noRoom) {
			blocked = true;
			moreInfo = true;
			return;
		}
		picker?.click();
	}

	/** Dismissing the message is the consent: what goes is named in it, and the import takes it. */
	function proceedAnyway() {
		blocked = false;
		picker?.click();
	}

	function dropped(e: DragEvent) {
		e.preventDefault();
		dragging = false;
		if (noRoom) {
			blocked = true;
			moreInfo = true;
			return;
		}
		const file = e.dataTransfer?.files?.[0];
		if (file) intake(file);
	}

	/** Extra detail under each imported thumbnail: size, whether it has been exported, and Remove. Off by
	 *  default so the row is one row high; the full-folder message turns it on, since that is where the
	 *  answer to it is. */
	let moreInfo = $state(false);

	async function signOut() {
		await fetch('/api/session', { method: 'DELETE' });
		location.href = '/signin';
	}
</script>

<svelte:head><title>Sidecar</title></svelte:head>

<main class="mx-auto min-h-dvh max-w-3xl bg-[#161616] px-4 pt-[max(1.5rem,env(safe-area-inset-top))] pb-[max(1.5rem,env(safe-area-inset-bottom))] text-[#f4f4f4]">
	<header class="flex items-baseline justify-between">
		<h1 class="text-xl">Sidecar</h1>
		<div class="flex items-baseline gap-4">
			<!-- Only in an image built with the mobile module (docs/MOBILE.md). -->
			{#if who?.mobile}<a href="/m/" class="text-sm text-[#c6c6c6] underline underline-offset-4">On this phone</a>{/if}
			{#if who?.authRequired}<button class="text-sm text-[#c6c6c6]" onclick={signOut}>Sign out {who.name}</button>{/if}
		</div>
	</header>
	<!-- With no Immich library there is nothing to share from and no link to paste: this deployment opens
	     photos from its own folders, so saying otherwise would only send someone looking for a library. -->
	{#if who && !who.immich}
		<p class="mt-2 text-sm text-fg-2">Choose a photo below to import it, or open one already in a folder Sidecar is configured for.</p>
	{:else}
		<p class="mt-2 text-sm text-fg-2">Share a photo from the Immich app and choose <em>Edit in Sidecar</em>, or paste a photo link from Immich.</p>
		<form class="mt-4 flex gap-2" onsubmit={openLink}>
			<label class="sr-only" for="immich-link">Immich photo link</label>
			<input id="immich-link" bind:value={link} placeholder="https://immich…/photos/…" class="h-11 min-w-0 flex-1 rounded bg-panel px-3 text-sm outline-none focus:ring-2 focus:ring-fg" />
			<button class="h-11 rounded bg-raised px-4 text-sm">Open</button>
		</form>
		{#if linkError}<p class="mt-2 text-sm text-danger">{linkError}</p>{/if}
	{/if}

	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<div
		class="mt-3 rounded border border-dashed p-4 text-center text-sm transition-colors {dragging ? 'border-fg bg-raised' : 'border-[#393939]'}"
		ondragover={(e) => {
			e.preventDefault();
			dragging = true;
		}}
		ondragleave={() => (dragging = false)}
		ondrop={dropped}
	>
		{#if intakeName}
			<p aria-live="polite">
				Adding <span class="font-mono">{intakeName}</span>
				to {destination === 'folder' ? (who?.intakeFolder ?? 'the editing folder') : 'your library'}…
			</p>
		{:else}
			<input bind:this={picker} type="file" class="sr-only" accept="image/*,{ACCEPT_RAW}" onchange={chosen} />
			<button type="button" class="underline underline-offset-4" onclick={choose}>Choose a photo on this device</button>
			{#if canChoose}
				<fieldset class="mt-2 flex items-center justify-center gap-1 text-xs">
					<legend class="sr-only">Where the photo goes</legend>
					{#each [{ v: 'immich', label: 'Into my library' }, { v: 'folder', label: who?.intakeFolder ?? 'Editing folder' }] as o (o.v)}
						<label class="cursor-pointer rounded-full px-2.5 py-1 {intakeTo === o.v ? 'bg-fg text-ground' : 'bg-raised text-fg-2'}">
							<input type="radio" class="sr-only" name="intake-to" value={o.v} checked={intakeTo === o.v} onchange={() => (intakeTo = o.v as 'immich' | 'folder')} />
							{o.label}
						</label>
					{/each}
				</fieldset>
				<p class="mt-1 text-xs text-fg-2">
					{#if intakeTo === 'immich'}It goes into your Immich library first, then opens in the editor.
					{:else}It is written to <span class="font-mono">{who?.intakeFolder}</span> and opens in the editor. Immich never sees it.{/if}
				</p>
			{:else if destination === 'folder'}
				<p class="mt-1 text-xs text-fg-2">It is written to <span class="font-mono">{who?.intakeFolder ?? 'the editing folder'}</span>, then opens in the editor.</p>
			{:else}
				<p class="mt-1 text-xs text-fg-2">It goes into your Immich library first, then opens in the editor.</p>
			{/if}
		{/if}
	</div>
	{#if blocked && folder?.configured}
		<!-- The folder is full. Nothing is chosen, uploaded or deleted until this is answered: the point of
		     saying it here is that the photo hasn't been picked yet, so backing out costs nothing. -->
		<div role="alertdialog" aria-labelledby="full-title" class="mt-2 rounded border border-[#8d8d8d] bg-panel p-3 text-sm">
			<p id="full-title" class="font-medium">{folder.name ?? 'The editing folder'} is full — {folder.items?.length ?? 0} of {folder.limit}.</p>
			{#if folder.nextOut}
				<p class="mt-1 text-fg-2">
					Importing another photo will remove <span class="font-mono">{folder.nextOut}</span>, the oldest one
					you have already exported. The exported file is unaffected; what goes is the original in this folder,
					and its edits with it.
				</p>
				<div class="mt-2 flex flex-wrap gap-2">
					<button type="button" class="h-9 rounded bg-raised px-3" onclick={proceedAnyway}>Remove it and import</button>
					<button type="button" class="h-9 rounded px-3 text-fg-2 underline underline-offset-4" onclick={() => (blocked = false)}>Cancel</button>
				</div>
			{:else}
				<p class="mt-1 text-fg-2">
					Nothing in it has been exported yet, so there is no photo Sidecar is willing to remove for you —
					each one is the only copy of its own work. Export what you want to keep, or remove one below.
				</p>
				<div class="mt-2">
					<button type="button" class="h-9 rounded px-3 text-fg-2 underline underline-offset-4" onclick={() => (blocked = false)}>Close</button>
				</div>
			{/if}
		</div>
	{/if}
	{#if intakeError}<p class="mt-2 text-sm text-danger">{intakeError}</p>{/if}

	{#if folder?.configured}
		<!-- The imported photos, one row of thumbnails high. The folder is bounded, so this is also the only
		     place a photo can be taken out of it — behind More info, which is off by default. -->
		<section class="mt-6">
			<div class="mb-3 flex items-baseline gap-2">
				<h2 class="text-sm font-medium text-[#c6c6c6]">Imported</h2>
				<span class="text-xs text-fg-2">{folder.items?.length ?? 0} of {folder.limit}{folder.full ? ', full' : ''}</span>
				{#if folder.items?.length}
					<button type="button" aria-pressed={moreInfo} class="ml-auto text-xs text-fg-2 underline underline-offset-4" onclick={() => (moreInfo = !moreInfo)}>
						{moreInfo ? 'Less' : 'More'} info
					</button>
				{/if}
			</div>
			{#if folder.items?.length}
				<ul class="flex gap-3 overflow-x-auto pb-1">
					{#each folder.items as f (f.assetKey)}
						<li class="w-40 shrink-0 overflow-hidden rounded bg-[#262626] sm:w-[calc((100%-2.25rem)/4)]">
							<a href="/edit/{f.assetKey}" class="block hover:bg-[#393939]">
								<img src={ownThumb(f.assetKey)} alt="" class="aspect-[3/2] w-full bg-[#1f1f1f] object-cover" loading="lazy" />
								<span class="block truncate p-2 text-sm">{f.fileName}</span>
							</a>
							{#if moreInfo}
								<div class="flex items-center gap-2 px-2 pb-2 text-[0.6875rem]">
									<span class="text-fg-2">{mb(f.bytes)}</span>
									{#if f.exported}
										<span class="rounded-full bg-[#393939] px-1.5 py-0.5 text-fg-2" title="A render of this photo exists elsewhere, so it is the first one removed when room is needed">exported</span>
									{/if}
									<button
										type="button"
										class="ml-auto rounded-full px-2 py-0.5 {armed === f.assetKey ? 'bg-danger text-ground' : 'bg-[#393939] text-fg-2'}"
										disabled={folderBusy === f.assetKey}
										aria-label={armed === f.assetKey ? `Confirm removing ${f.fileName} from the folder` : `Remove ${f.fileName} from the folder`}
										onclick={() => (armed === f.assetKey ? remove(f) : arm(f.assetKey))}
									>{folderBusy === f.assetKey ? 'Removing…' : armed === f.assetKey ? 'Remove?' : 'Remove'}</button>
								</div>
							{/if}
						</li>
					{/each}
				</ul>
				{#if moreInfo}
					<p class="mt-2 text-xs text-fg-2">
						Removing takes two taps and deletes the original and its edits from this folder. Anything you have
						exported is already elsewhere and is not touched.
					</p>
				{/if}
			{:else}
				<p class="text-sm text-[#8d8d8d]">A photo imported into this folder appears here.</p>
			{/if}
		</section>
	{/if}

	<h2 class="mt-8 mb-3 text-sm font-medium text-[#c6c6c6]">Recent</h2>
	{#if recentElsewhere?.length}
		<ul class="grid grid-cols-2 gap-3 sm:grid-cols-4">
			{#each recentElsewhere as r (r.assetKey)}
				<li>
					<a href="/edit/{r.assetKey}" class="block overflow-hidden rounded bg-[#262626] hover:bg-[#393939]">
						<img
							src={r.immichAssetId ? thumbnailUrl(r.immichAssetId) : ownThumb(r.assetKey)}
							alt=""
							class="aspect-[3/2] w-full bg-[#1f1f1f] object-cover"
							loading="lazy"
							onerror={(e) => thumbFallback(e, r.assetKey)}
						/>
						<span class="block truncate p-2 text-sm">{r.fileName}</span>
					</a>
				</li>
			{/each}
		</ul>
	{:else if recent}
		<p class="text-sm text-[#8d8d8d]">{folder?.items?.length ? 'Photos you open from elsewhere appear here.' : 'Photos you open appear here.'}</p>
	{/if}
</main>
