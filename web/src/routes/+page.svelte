<!-- SPDX-License-Identifier: Apache-2.0 -->
<script lang="ts">
	import { api, thumbnailUrl } from '$lib/api';

	type Recent = { assetKey: string; fileName: string; immichAssetId: string | null; openedAt: string };
	let recent = $state<Recent[] | null>(null);
	let who = $state<{ name: string; authRequired: boolean } | null>(null);

	$effect(() => {
		(async () => {
			who = await (await api('/api/session')).json();
			recent = await (await api('/api/recent')).json();
		})().catch(() => {});
	});

	let link = $state('');
	let linkError = $state('');
	function openLink(e: SubmitEvent) {
		e.preventDefault();
		const id = link.match(/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/i)?.[0];
		if (!id) {
			linkError = "That doesn't look like an Immich photo link. Copy it from the photo's page in Immich.";
			return;
		}
		location.href = `/open?immich=${id}`;
	}

	// ---------- a photo from this device: uploaded into the library, then opened like any other
	let intakeName = $state('');
	let intakeError = $state('');
	let dragging = $state(false);

	async function intake(file: File) {
		intakeName = file.name;
		intakeError = '';
		try {
			const body = new FormData();
			body.append('file', file, file.name);
			const r = await api('/api/intake', { method: 'POST', body });
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
	<p class="mt-2 text-sm text-fg-2">Share a photo from the Immich app and choose <em>Edit in Sidecar</em>, or paste a photo link from Immich.</p>
	<form class="mt-4 flex gap-2" onsubmit={openLink}>
		<label class="sr-only" for="immich-link">Immich photo link</label>
		<input id="immich-link" bind:value={link} placeholder="https://immich…/photos/…" class="h-11 min-w-0 flex-1 rounded bg-panel px-3 text-sm outline-none focus:ring-2 focus:ring-fg" />
		<button class="h-11 rounded bg-raised px-4 text-sm">Open</button>
	</form>
	{#if linkError}<p class="mt-2 text-sm text-danger">{linkError}</p>{/if}

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
			<p aria-live="polite">Adding <span class="font-mono">{intakeName}</span> to your library…</p>
		{:else}
			<label class="cursor-pointer">
				<input type="file" class="sr-only" accept="image/*,.arw,.nef,.cr2,.cr3,.raf,.rw2,.orf,.dng,.pef,.srw" onchange={chosen} />
				<span class="underline underline-offset-4">Choose a photo on this device</span>
			</label>
			<p class="mt-1 text-xs text-fg-2">It goes into your Immich library first, then opens in the editor.</p>
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
