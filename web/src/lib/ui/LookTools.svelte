<!-- SPDX-License-Identifier: Apache-2.0
A photo's look, taken elsewhere (DESIGN.md §15, "Presets, copy and paste, sync"): chosen sections copied and pasted
onto another photo, and kept as presets — applied here, exported as files, imported from them. -->
<script lang="ts">
	import { copyLook, describeSections, lastSections, pasteLook, readLookClipboard, SECTIONS, type LookClipboard } from '$lib/state/look-clipboard';
	import * as presets from '$lib/state/presets';
	import type { Preset } from '$lib/state/presets';
	import type { EditParams } from '$lib/types';

	let {
		edit,
		defaults,
		fileName,
		readOnly,
		onchange,
		oncommit
	}: { edit: () => EditParams; defaults: () => EditParams; fileName: string; readOnly: boolean; onchange: (fn: (e: EditParams) => void) => void; oncommit: () => void } = $props();

	let clipboard = $state<LookClipboard | null>(readLookClipboard());
	let list = $state<Preset[]>([]);
	let error = $state('');
	/** What the section chooser is open for, if anything, and what it has ticked. */
	let choosing = $state<'copy' | 'save' | null>(null);
	let chosen = $state<string[]>(lastSections());
	let name = $state('');
	let armed = $state<string | null>(null);
	let importer: HTMLInputElement | undefined = $state();

	$effect(() => {
		presets.list().then((l) => (list = l)).catch(() => (list = []));
	});

	function open(what: 'copy' | 'save') {
		(choosing = choosing === what ? null : what), (error = ''), (chosen = lastSections());
	}

	function copy() {
		copyLook(edit(), fileName, chosen);
		clipboard = readLookClipboard();
		choosing = null;
	}

	function paste() {
		onchange((e) => pasteLook(e));
		oncommit();
	}

	async function save() {
		try {
			const p = await presets.create(name, chosen, edit(), defaults());
			list = [...list, p].sort((a, b) => a.name.localeCompare(b.name));
			(choosing = null), (name = '');
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
	}

	function applyPreset(p: Preset) {
		// A plain copy: the list is reactive state, whose proxies the sections' `structuredClone` can't take.
		const plain = $state.snapshot(p);
		onchange((e) => presets.apply(e, plain));
		oncommit();
	}

	async function removePreset(p: Preset) {
		if (armed !== p.id) return (armed = p.id);
		await presets.remove(p.id).catch(() => {});
		list = list.filter((x) => x.id !== p.id);
		armed = null;
	}

	async function imported() {
		const file = importer?.files?.[0];
		if (!file) return;
		try {
			const p = await presets.importFile(file, defaults());
			list = [...list, p].sort((a, b) => a.name.localeCompare(b.name));
			error = '';
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			importer!.value = '';
		}
	}

	const pill = 'rounded-full bg-raised px-2.5 py-1 text-fg disabled:opacity-40';
</script>

{#snippet chooser()}
	<fieldset class="grid grid-cols-2 gap-x-3 gap-y-1 rounded-md bg-raised/50 p-2">
		<legend class="sr-only">Sections</legend>
		{#each SECTIONS as s (s.id)}
			<label class="flex items-center gap-1.5 text-fg-2">
				<input type="checkbox" checked={chosen.includes(s.id)} onchange={(ev) => (chosen = (ev.currentTarget as HTMLInputElement).checked ? [...chosen, s.id] : chosen.filter((x) => x !== s.id))} />
				{s.label}
			</label>
		{/each}
	</fieldset>
{/snippet}

<div class="mb-3 grid gap-1.5 text-xs">
	<div class="flex flex-wrap items-center gap-1.5">
		<span class="text-fg-2">Look</span>
		<button type="button" class={pill} aria-expanded={choosing === 'copy'} onclick={() => open('copy')}>Copy…</button>
		<button type="button" class={pill} disabled={!clipboard || readOnly} title={clipboard ? `Paste ${describeSections(clipboard.sections)}` : undefined} onclick={paste}>Paste</button>
		{#if clipboard}<span class="min-w-0 truncate text-fg-2" title={describeSections(clipboard.sections)}>from {clipboard.from}</span>{/if}
	</div>
	{#if choosing === 'copy'}
		{@render chooser()}
		<div class="flex gap-1.5">
			<button type="button" class={pill} disabled={!chosen.length} onclick={copy}>Copy these</button>
			<button type="button" class="px-2 text-fg-2" onclick={() => (choosing = null)}>Cancel</button>
		</div>
	{/if}

	<div class="flex flex-wrap items-center gap-1.5 pt-1">
		<span class="text-fg-2">Presets</span>
		<button type="button" class={pill} aria-expanded={choosing === 'save'} onclick={() => open('save')}>Save as preset…</button>
		<button type="button" class={pill} onclick={() => importer?.click()}>Import…</button>
		<input bind:this={importer} type="file" accept=".json,application/json" class="hidden" aria-label="Import a preset file" onchange={imported} />
	</div>
	{#if choosing === 'save'}
		<label class="sr-only" for="preset-name">Preset name</label>
		<input id="preset-name" class="rounded-md bg-raised px-2 py-1 text-fg" placeholder="Name" bind:value={name} />
		{@render chooser()}
		<div class="flex gap-1.5">
			<button type="button" class={pill} disabled={!chosen.length || !name.trim()} onclick={save}>Save preset</button>
			<button type="button" class="px-2 text-fg-2" onclick={() => (choosing = null)}>Cancel</button>
		</div>
	{/if}
	{#if error}<p class="text-warning">{error}</p>{/if}
	{#if list.length}
		<ul class="grid gap-1">
			{#each list as p (p.id)}
				<li class="flex items-center gap-1.5">
					<button type="button" class="min-w-0 flex-1 truncate rounded-md bg-raised px-2 py-1 text-left text-fg disabled:opacity-40" disabled={readOnly} title="Apply: {describeSections(p.sections)}" onclick={() => applyPreset(p)}>{p.name}</button>
					<button type="button" class="px-1.5 text-fg-2" title="Download as a file" aria-label="Export {p.name}" onclick={() => presets.exportFile(p)}>Export</button>
					<button type="button" class="px-1.5 {armed === p.id ? 'text-danger' : 'text-fg-2'}" aria-label={armed === p.id ? `Confirm deleting ${p.name}` : `Delete ${p.name}`} onclick={() => removePreset(p)}>{armed === p.id ? 'Delete?' : 'Delete'}</button>
				</li>
			{/each}
		</ul>
	{/if}
</div>
