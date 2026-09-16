<!-- SPDX-License-Identifier: Apache-2.0 -->
<script lang="ts">
	import { page } from '$app/state';

	let apiKey = $state('');
	let error = $state('');
	let busy = $state(false);

	async function submit(e: SubmitEvent) {
		e.preventDefault();
		busy = true;
		error = '';
		const r = await fetch('/api/session', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ apiKey }) });
		busy = false;
		if (!r.ok) {
			error = (await r.text()) || 'Sign-in failed.';
			return;
		}
		const next = page.url.searchParams.get('next') ?? '/';
		location.href = next.startsWith('/') ? next : '/';
	}
</script>

<svelte:head><title>Sign in · Sidecar</title></svelte:head>

<main class="mx-auto flex min-h-dvh max-w-md flex-col justify-center gap-6 bg-[#161616] px-6 py-10 text-[#f4f4f4]">
	<div>
		<h1 class="text-xl">Sign in to Sidecar</h1>
		<p class="mt-2 text-sm text-[#c6c6c6]">Sidecar uses your Immich account. Paste an Immich API key; it's stored encrypted on your server and never kept in this browser.</p>
	</div>
	<form class="grid gap-3" onsubmit={submit}>
		<label class="grid gap-1 text-sm text-[#c6c6c6]" for="api-key">
			Immich API key
			<input id="api-key" bind:value={apiKey} autocomplete="off" autocapitalize="off" spellcheck="false" class="h-11 rounded bg-[#262626] px-3 font-mono text-[#f4f4f4] outline-none focus:ring-2 focus:ring-[#f4f4f4]" required />
		</label>
		{#if error}<p class="text-sm text-[#ff8389]">{error}</p>{/if}
		<button disabled={busy || !apiKey} class="h-11 rounded bg-[#f4f4f4] font-medium text-[#161616] disabled:opacity-40">{busy ? 'Checking…' : 'Sign in'}</button>
	</form>
	<details class="text-sm text-[#c6c6c6]">
		<summary class="cursor-pointer text-[#f4f4f4]">Create a key in Immich</summary>
		<p class="mt-2">In Immich, open <em>Account settings → API keys → New API key</em> and give it these permissions:</p>
		<ul class="mt-2 list-disc pl-5 font-mono text-xs leading-6">
			<li>user.read</li><li>asset.read</li><li>asset.view</li><li>asset.upload</li><li>asset.update</li><li>stack.create</li>
		</ul>
	</details>
</main>
