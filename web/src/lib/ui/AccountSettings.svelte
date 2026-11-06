<!-- SPDX-License-Identifier: Apache-2.0
Local accounts in Settings (crates/api/src/accounts.rs): your own password, and for an administrator the
household's accounts — add someone, set a forgotten password, make or unmake an administrator, remove. -->
<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '$lib/api';

	let { isAdmin, name, me }: { isAdmin: boolean; name: string; me: string } = $props();

	type Person = { id: string; username: string; name: string; isAdmin: boolean };
	let people = $state<Person[]>([]);
	let message = $state('');
	let failed = $state(false);

	let current = $state('');
	let fresh = $state('');
	let freshAgain = $state('');

	let adding = $state(false);
	let newUser = $state({ username: '', name: '', password: '', isAdmin: false });
	let resetting = $state<string | null>(null);
	let resetTo = $state('');

	const field = 'h-10 w-full rounded bg-panel px-3 text-sm text-fg outline-none focus:ring-2 focus:ring-fg';
	const button = 'h-9 rounded bg-raised px-3 text-sm text-fg disabled:opacity-40';
	const inRow = 'h-9 rounded bg-panel px-3 text-sm text-fg disabled:opacity-40';
	const primary = 'h-9 rounded bg-fg px-3 text-sm font-medium text-ground disabled:opacity-40';

	async function load() {
		if (isAdmin) people = await (await api('/api/accounts')).json();
	}
	onMount(load);

	async function act(what: () => Promise<unknown>, done: string) {
		message = '';
		failed = false;
		try {
			await what();
			message = done;
			await load();
		} catch (e) {
			failed = true;
			message = e instanceof Error ? e.message : String(e);
		}
	}

	const json = (method: string, body: unknown): RequestInit => ({ method, headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) });

	function changeOwn(e: SubmitEvent) {
		e.preventDefault();
		if (fresh !== freshAgain) {
			failed = true;
			message = 'The two new passwords differ.';
			return;
		}
		act(async () => {
			await api('/api/account/password', json('PUT', { current, password: fresh }));
			current = fresh = freshAgain = '';
		}, 'Password changed. Other browsers signed in as you have been signed out.');
	}

	function add(e: SubmitEvent) {
		e.preventDefault();
		act(async () => {
			await api('/api/accounts', json('POST', newUser));
			newUser = { username: '', name: '', password: '', isAdmin: false };
			adding = false;
		}, 'Account added. Tell them their username and password; they can change the password in Settings.');
	}

	function reset(p: Person, e: SubmitEvent) {
		e.preventDefault();
		act(async () => {
			await api(`/api/accounts/${p.id}`, json('PUT', { password: resetTo }));
			resetting = null;
			resetTo = '';
		}, `${p.name}'s password is set, and they are signed out everywhere.`);
	}

	const toggleAdmin = (p: Person) =>
		act(() => api(`/api/accounts/${p.id}`, json('PUT', { isAdmin: !p.isAdmin })), p.isAdmin ? `${p.name} is no longer an administrator.` : `${p.name} is now an administrator.`);

	function remove(p: Person) {
		if (!confirm(`Remove ${p.name}'s account? Their photos and edits stay; their Recent list goes.`)) return;
		act(() => api(`/api/accounts/${p.id}`, { method: 'DELETE' }), `${p.name}'s account is removed.`);
	}
</script>

<section class="mt-8">
	<h2 class="text-sm font-medium">Your account</h2>
	<p class="mt-1 text-sm text-fg-2">Signed in as {name}.</p>
	<form class="mt-3 grid gap-2" onsubmit={changeOwn}>
		<input type="password" class={field} placeholder="Current password" autocomplete="current-password" bind:value={current} required />
		<input type="password" class={field} placeholder="New password (8 characters or more)" autocomplete="new-password" minlength="8" bind:value={fresh} required />
		<input type="password" class={field} placeholder="New password again" autocomplete="new-password" bind:value={freshAgain} required />
		<div><button class={primary} disabled={!current || !fresh}>Change password</button></div>
	</form>
</section>

{#if isAdmin}
	<section class="mt-8">
		<h2 class="text-sm font-medium">People</h2>
		<p class="mt-1 text-sm text-fg-2">Everyone who can sign in to this Sidecar. An administrator can add people and set a forgotten password.</p>
		<ul class="mt-3 grid gap-2">
			{#each people as p (p.id)}
				<li class="rounded bg-raised p-3 text-sm">
					<div class="flex flex-wrap items-baseline justify-between gap-2">
						<span><span class="font-medium">{p.name}</span> <span class="text-fg-2">{p.username}{p.isAdmin ? ' · administrator' : ''}</span></span>
						<span class="flex gap-2">
							<button class={inRow} onclick={() => ((resetting = resetting === p.id ? null : p.id), (resetTo = ''))}>Set password</button>
							<button class={inRow} onclick={() => toggleAdmin(p)}>{p.isAdmin ? 'Not admin' : 'Make admin'}</button>
							{#if p.id !== me}<button class={inRow} onclick={() => remove(p)}>Remove</button>{/if}
						</span>
					</div>
					{#if resetting === p.id}
						<form class="mt-2 flex gap-2" onsubmit={(e) => reset(p, e)}>
							<input type="password" class={field} placeholder="New password for {p.name}" autocomplete="new-password" minlength="8" bind:value={resetTo} required />
							<button class={primary} disabled={!resetTo}>Set</button>
						</form>
					{/if}
				</li>
			{/each}
		</ul>
		{#if adding}
			<form class="mt-3 grid gap-2 rounded bg-raised p-3" onsubmit={add}>
				<input class={field} placeholder="Name" autocomplete="off" bind:value={newUser.name} />
				<input class={field} placeholder="Username" autocomplete="off" autocapitalize="off" spellcheck="false" bind:value={newUser.username} required />
				<input type="password" class={field} placeholder="Password (8 characters or more)" autocomplete="new-password" minlength="8" bind:value={newUser.password} required />
				<label class="flex items-center gap-2 text-sm"><input type="checkbox" class="size-4 accent-fg" bind:checked={newUser.isAdmin} /> Administrator</label>
				<div class="flex gap-2">
					<button class={primary} disabled={!newUser.username || !newUser.password}>Add</button>
					<button type="button" class={inRow} onclick={() => (adding = false)}>Cancel</button>
				</div>
			</form>
		{:else}
			<button class="{button} mt-3" onclick={() => (adding = true)}>Add someone</button>
		{/if}
	</section>
{/if}

{#if message}
	<p class="mt-3 text-sm {failed ? 'text-danger' : 'text-fg-2'}" role="status">{message}</p>
{/if}
