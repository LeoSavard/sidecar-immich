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

	const SPACES: { value: DisplaySpace; blurb: string }[] = [
		{ value: 'srgb', blurb: 'The safe choice. Every screen and app shows it the same way.' },
		{ value: 'display-p3', blurb: 'Wider reds and greens. Apple screens and Immich show it correctly; older apps may not.' }
	];

	onMount(async () => {
		current = await settings();
		status = 'idle';
		wideDisplay = matchMedia('(color-gamut: p3)').matches;
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

	<p class="mt-6 h-5 text-xs text-fg-2" role="status">
		{#if status === 'saving'}Saving…{:else if status === 'saved'}Saved{:else if status === 'failed'}Couldn't save: {error}{/if}
	</p>
</main>
