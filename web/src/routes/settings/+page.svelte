<!-- SPDX-License-Identifier: Apache-2.0
Settings: one choice of output space drives the viewport, the loupe and the export default, so what you
judge on screen is what the exported file holds. -->
<script lang="ts">
	import { onMount } from 'svelte';
	import { DEFAULTS, saveSettings, settings, SPACE_LABELS } from '$lib/settings';
	import type { DisplaySpace, Settings } from '$lib/types';

	let current = $state<Settings>({ ...DEFAULTS });
	let status = $state<'loading' | 'idle' | 'saving' | 'saved' | 'failed'>('loading');
	let error = $state('');
	let wideDisplay = $state(false);

	/** Mirrors `sidecar_worker::models::Status`. */
	type ModelStatus = { id: string; name: string; enables: string; licence: string; restrictions: string | null; delivery: 'shipped' | 'on-demand'; state: 'installed' | 'not-in-this-image' | 'not-downloaded' | 'needs-update' };
	let models = $state<ModelStatus[] | null>(null);
	const STATE_LABELS: Record<ModelStatus['state'], string> = {
		installed: 'On this server',
		'not-in-this-image': 'Not in this image (the -ml image has it)',
		'not-downloaded': 'Not downloaded',
		'needs-update': 'Needs updating for this version'
	};

	const SPACES: { value: DisplaySpace; blurb: string }[] = [
		{ value: 'srgb', blurb: 'The safe choice. Every screen and app shows it the same way.' },
		{ value: 'display-p3', blurb: 'Wider reds and greens. Apple screens and Immich show it correctly; older apps may not.' }
	];

	onMount(async () => {
		current = await settings();
		status = 'idle';
		wideDisplay = matchMedia('(color-gamut: p3)').matches;
		models = await fetch('/api/models')
			.then((r) => (r.ok ? r.json() : null))
			.catch(() => null);
	});

	async function update(change: Partial<Settings>) {
		const next = { ...current, ...change };
		current = next;
		status = 'saving';
		try {
			current = await saveSettings(next);
			status = 'saved';
			setTimeout(() => status === 'saved' && (status = 'idle'), 1500);
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
			status = 'failed';
		}
	}
</script>

<svelte:head><title>Settings · Sidecar</title></svelte:head>

<main class="mx-auto min-h-dvh max-w-xl bg-ground px-5 pt-[max(1.5rem,env(safe-area-inset-top))] pb-[max(2rem,env(safe-area-inset-bottom))] text-fg">
	<a href="/" class="text-sm text-fg-2">← Sidecar</a>
	<h1 class="mt-3 text-xl">Settings</h1>

	<section class="mt-6">
		<h2 class="text-sm font-medium">Output space</h2>
		<p class="mt-1 text-sm text-fg-2">
			The viewport, the loupe and new exports all use this. Editing in the space you export to means the colours you see are the colours you get —
			pick a wider space and the screen can show reds the file can't hold.
		</p>
		<div class="mt-3 grid gap-2">
			{#each SPACES as space (space.value)}
				<button
					type="button"
					aria-pressed={current.outputSpace === space.value}
					class="rounded p-3 text-left {current.outputSpace === space.value ? 'bg-fg text-ground' : 'bg-raised text-fg'}"
					onclick={() => update({ outputSpace: space.value })}
				>
					<span class="block text-sm font-medium">{SPACE_LABELS[space.value]}</span>
					<span class="mt-0.5 block text-xs {current.outputSpace === space.value ? 'text-ground/70' : 'text-fg-2'}">{space.blurb}</span>
				</button>
			{/each}
		</div>
		{#if !wideDisplay && current.outputSpace === 'display-p3'}
			<p class="mt-2 text-xs text-fg-2">This screen reports sRGB only, so it will show an approximation of Display P3.</p>
		{/if}
	</section>

	<section class="mt-8">
		<h2 class="text-sm font-medium">Clipping warnings</h2>
		<p class="mt-1 text-sm text-fg-2">
			Mark the pixels that have no detail left in the output space — red where they reach white, blue where they reach black. You can still toggle
			them per photo from the histogram.
		</p>
		<div class="mt-3 grid gap-2">
			{#each [['clipHighlights', 'Start with highlight warnings on'], ['clipShadows', 'Start with shadow warnings on']] as [key, label] (key)}
				<label class="flex items-center justify-between rounded bg-raised p-3 text-sm">
					<span>{label}</span>
					<input
						type="checkbox"
						class="size-5 accent-fg"
						checked={current[key as 'clipHighlights' | 'clipShadows']}
						onchange={(e) => update({ [key]: e.currentTarget.checked } as Partial<Settings>)}
					/>
				</label>
			{/each}
		</div>
	</section>

	<section class="mt-8" aria-labelledby="models-heading">
		<h2 id="models-heading" class="text-sm font-medium">Models</h2>
		<p class="mt-1 text-sm text-fg-2">
			The machine-learning models Sidecar can run, what each makes possible, and the licence its weights come under. They run on this
			server's GPU; nothing is sent elsewhere.
		</p>
		{#if models}
			<ul class="mt-3 grid gap-2">
				{#each models as m (m.id)}
					<li class="rounded bg-raised p-3 text-sm">
						<div class="flex items-baseline justify-between gap-2">
							<span class="font-medium">{m.name}</span>
							<span class="text-xs {m.state === 'installed' ? 'text-fg' : 'text-warning'}">{STATE_LABELS[m.state]}</span>
						</div>
						<p class="mt-0.5 text-xs text-fg-2">{m.enables} · {m.licence}{m.delivery === 'on-demand' ? ' · downloaded, never shipped' : ''}</p>
						{#if m.restrictions}<p class="mt-0.5 text-xs text-warning">{m.restrictions}</p>{/if}
					</li>
				{/each}
			</ul>
		{:else}
			<p class="mt-3 text-sm text-fg-2">Loading…</p>
		{/if}
	</section>

	<p class="mt-6 h-5 text-xs text-fg-2" role="status">
		{#if status === 'saving'}Saving…{:else if status === 'saved'}Saved{:else if status === 'failed'}Couldn't save: {error}{/if}
	</p>
</main>
