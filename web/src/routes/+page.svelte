<!-- SPDX-License-Identifier: Apache-2.0 -->
<script lang="ts">
	import { api, thumbnailUrl } from '$lib/api';

	type Recent = { assetKey: string; fileName: string; immichAssetId: string | null; openedAt: string };
	let recent = $state<Recent[] | null>(null);
	let who = $state<{ name: string; authRequired: boolean; immich: boolean; intakeFolder: string | null } | null>(null);

	$effect(() => {
		(async () => {
			who = await (await api('/api/session')).json();
			recent = await (await api('/api/recent')).json();
		})().catch(() => {});
	});

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
			const body = new FormData();
			body.append('file', file, file.name);
			const r = await api(`/api/intake?to=${destination}`, { method: 'POST', body });
			location.href = `/edit/${(await r.json()).assetKey}`;
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

	function dropped(e: DragEvent) {
		e.preventDefault();
		dragging = false;
		const file = e.dataTransfer?.files?.[0];
		if (file) intake(file);
	}

	async function signOut() {
		await fetch('/api/session', { method: 'DELETE' });
		location.href = '/signin';
	}
</script>

<svelte:head><title>Sidecar</title></svelte:head>

<main class="mx-auto min-h-dvh max-w-3xl bg-[#161616] px-4 pt-[max(1.5rem,env(safe-area-inset-top))] pb-[max(1.5rem,env(safe-area-inset-bottom))] text-[#f4f4f4]">
	<header class="flex items-baseline justify-between">
		<h1 class="text-xl">Sidecar</h1>
		{#if who?.authRequired}<button class="text-sm text-[#c6c6c6]" onclick={signOut}>Sign out {who.name}</button>{/if}
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
			<label class="cursor-pointer">
				<input type="file" class="sr-only" accept="image/*,.arw,.nef,.cr2,.cr3,.raf,.rw2,.orf,.dng,.pef,.srw" onchange={chosen} />
				<span class="underline underline-offset-4">Choose a photo on this device</span>
			</label>
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
	{#if intakeError}<p class="mt-2 text-sm text-danger">{intakeError}</p>{/if}

	<div class="mt-3 flex flex-wrap gap-4 text-sm text-fg-2">
		<a href="/setup" class="underline underline-offset-4">Set up the iPhone shortcut</a>
		<a href="/settings" class="underline underline-offset-4">Settings</a>
	</div>
	<h2 class="mt-8 mb-3 text-sm font-medium text-[#c6c6c6]">Recent</h2>
	{#if recent?.length}
		<ul class="grid grid-cols-2 gap-3 sm:grid-cols-4">
			{#each recent as r (r.assetKey)}
				<li>
					<a href="/edit/{r.assetKey}" class="block overflow-hidden rounded bg-[#262626] hover:bg-[#393939]">
						{#if r.immichAssetId}<img src={thumbnailUrl(r.immichAssetId)} alt="" class="aspect-[3/2] w-full object-cover" loading="lazy" />{:else}<div class="aspect-[3/2] w-full bg-[#1f1f1f]"></div>{/if}
						<span class="block truncate p-2 text-sm">{r.fileName}</span>
					</a>
				</li>
			{/each}
		</ul>
	{:else if recent}
		<p class="text-sm text-[#8d8d8d]">Photos you open appear here.</p>
	{/if}
</main>
