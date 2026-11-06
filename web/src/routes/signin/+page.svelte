<!-- SPDX-License-Identifier: Apache-2.0
Sign-in. With an Immich, an Immich API key; without one, a local account's username and password — and on a
fresh server, the form that makes the first account with the setup code from the server's log. -->
<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/state';

	type Method = 'immich' | 'password' | 'none';
	let method = $state<Method | null>(null);
	let setup = $state(false);

	let apiKey = $state('');
	let username = $state('');
	let password = $state('');
	let again = $state('');
	let name = $state('');
	let code = $state('');
	let error = $state('');
	let busy = $state(false);

	const field = 'h-11 rounded bg-panel px-3 text-fg outline-none focus:ring-2 focus:ring-fg';

	function done() {
		const next = page.url.searchParams.get('next') ?? '/';
		location.href = next.startsWith('/') && !next.startsWith('//') ? next : '/';
	}

	onMount(async () => {
		const r = await fetch('/api/sign-in');
		const m: { method: Method; setup: boolean } = await r.json();
		if (m.method === 'none') return done();
		method = m.method;
		setup = m.setup;
	});

	async function post(path: string, body: unknown) {
		busy = true;
		error = '';
		const r = await fetch(path, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) });
		busy = false;
		if (!r.ok) {
			error = (await r.text()) || 'Sign-in failed.';
			// Someone made the first account in the meantime: offer the ordinary form.
			if (r.status === 409) setup = false;
			return;
		}
		done();
	}

	function submit(e: SubmitEvent) {
		e.preventDefault();
		if (method === 'immich') return post('/api/session', { apiKey });
		if (!setup) return post('/api/session', { username, password });
		if (password !== again) {
			error = 'The two passwords differ.';
			return;
		}
		return post('/api/accounts/setup', { code, username, name, password });
	}
</script>

<svelte:head><title>Sign in · Sidecar</title></svelte:head>

<main class="mx-auto flex min-h-dvh max-w-md flex-col justify-center gap-6 bg-ground px-6 py-10 text-fg">
	{#if method === 'immich'}
		<div>
			<h1 class="text-xl">Sign in to Sidecar</h1>
			<p class="mt-2 text-sm text-fg-2">Sidecar uses your Immich account. Paste an Immich API key; it's stored encrypted on your server and never kept in this browser.</p>
		</div>
		<form class="grid gap-3" onsubmit={submit}>
			<label class="grid gap-1 text-sm text-fg-2" for="api-key">
				Immich API key
				<input id="api-key" bind:value={apiKey} autocomplete="off" autocapitalize="off" spellcheck="false" class="{field} font-mono" required />
			</label>
			{#if error}<p class="text-sm text-danger">{error}</p>{/if}
			<button disabled={busy || !apiKey} class="h-11 rounded bg-fg font-medium text-ground disabled:opacity-40">{busy ? 'Checking…' : 'Sign in'}</button>
		</form>
		<details class="text-sm text-fg-2">
			<summary class="cursor-pointer text-fg">Create a key in Immich</summary>
			<p class="mt-2">In Immich, open <em>Account settings → API keys → New API key</em> and give it these permissions:</p>
			<ul class="mt-2 list-disc pl-5 font-mono text-xs leading-6">
				<li>user.read</li><li>asset.read</li><li>asset.view</li><li>asset.upload</li><li>asset.update</li><li>stack.create</li>
			</ul>
		</details>
	{:else if method === 'password'}
		<div>
			<h1 class="text-xl">{setup ? 'Set up Sidecar' : 'Sign in to Sidecar'}</h1>
			{#if setup}
				<p class="mt-2 text-sm text-fg-2">
					Make the first account. It can add everyone else later, in Settings. The setup code is in the server's log, on the line that starts
					<em>No accounts yet</em> — <span class="font-mono text-xs">docker logs</span> on the Sidecar container shows it.
				</p>
			{/if}
		</div>
		<form class="grid gap-3" onsubmit={submit}>
			{#if setup}
				<label class="grid gap-1 text-sm text-fg-2">
					Setup code
					<input bind:value={code} autocomplete="off" autocapitalize="characters" spellcheck="false" class="{field} font-mono uppercase" required />
				</label>
				<label class="grid gap-1 text-sm text-fg-2">
					Your name
					<input bind:value={name} autocomplete="name" class={field} />
				</label>
			{/if}
			<label class="grid gap-1 text-sm text-fg-2">
				Username
				<input bind:value={username} autocomplete="username" autocapitalize="off" spellcheck="false" class={field} required />
			</label>
			<label class="grid gap-1 text-sm text-fg-2">
				Password
				<input type="password" bind:value={password} autocomplete={setup ? 'new-password' : 'current-password'} minlength={setup ? 8 : undefined} class={field} required />
			</label>
			{#if setup}
				<label class="grid gap-1 text-sm text-fg-2">
					Password again
					<input type="password" bind:value={again} autocomplete="new-password" class={field} required />
				</label>
				<p class="text-xs text-fg-2">At least 8 characters. A short sentence is easier to remember and harder to guess than a word with symbols.</p>
			{/if}
			{#if error}<p class="text-sm text-danger" role="alert">{error}</p>{/if}
			<button disabled={busy || !username || !password} class="h-11 rounded bg-fg font-medium text-ground disabled:opacity-40">
				{busy ? 'Checking…' : setup ? 'Create the account' : 'Sign in'}
			</button>
		</form>
		{#if !setup}
			<p class="text-sm text-fg-2">Forgotten your password? Someone with an administrator account can set a new one in Settings.</p>
		{/if}
	{/if}
</main>
