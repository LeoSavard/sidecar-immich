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
