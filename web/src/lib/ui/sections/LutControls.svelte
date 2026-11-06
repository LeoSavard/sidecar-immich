<!-- SPDX-License-Identifier: Apache-2.0
A 3D LUT as a look (DESIGN.md §15, "3D LUTs"): one of the server's, or a .cube file uploaded now; how much of it;
and the space it was built in, which the file itself doesn't say. -->
<script lang="ts">
	import Slider from '$lib/ui/Slider.svelte';
	import { decimals } from '$lib/ui/format';
	import { isMissing, list, upload, type LutInfo } from '$lib/luts';
	import type { LutSpace } from '$lib/generated/edit/LutSpace';
	import type { SectionProps } from './section';

	let { edit, onchange, oncommit }: SectionProps = $props();

	const SPACES: { id: LutSpace; label: string; note: string }[] = [
		{ id: 'srgb', label: 'sRGB', note: 'Photoshop and most photo tools build LUTs in sRGB.' },
		{ id: 'rec709', label: 'Rec.709 video', note: 'Video grading tools build for Rec.709 with a γ 2.4 display.' },
		{ id: 'display-p3', label: 'Display P3', note: 'P3 primaries with the sRGB curve, as Apple’s displays.' }
	];

	let luts = $state<LutInfo[]>([]);
	let error = $state('');
	let busy = $state(false);
	let input: HTMLInputElement | undefined = $state();

	$effect(() => {
		list().then((l) => (luts = l)).catch(() => (luts = []));
	});

	function choose(info: LutInfo | null) {
		onchange((e) => {
			if (!info) delete e.lut;
			else e.lut = { id: info.id, name: info.name, amount: e.lut?.amount ?? 100, space: e.lut?.space ?? 'srgb' };
		});
		oncommit();
	}

	async function picked() {
		const file = input?.files?.[0];
		if (!file) return;
		(busy = true), (error = '');
		try {
			const info = await upload(file);
			luts = [...luts.filter((l) => l.id !== info.id), info].sort((a, b) => a.name.localeCompare(b.name));
			choose(info);
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			busy = false;
			input!.value = '';
		}
	}

	const choice = (on: boolean) => `rounded-full px-3 py-1 ${on ? 'bg-fg text-ground' : 'bg-raised text-fg-2'}`;
	const space = $derived(SPACES.find((s) => s.id === (edit.lut?.space ?? 'srgb'))!);
</script>

<p class="pt-1 text-[11px] font-medium tracking-wide text-muted uppercase">LUT</p>
<div class="flex flex-wrap items-center gap-1.5 text-xs">
	<label class="sr-only" for="lut-choice">LUT</label>
	<select id="lut-choice" class="min-w-0 flex-1 rounded-md bg-raised px-2 py-1 text-fg" value={edit.lut?.id ?? ''} onchange={(ev) => choose(luts.find((l) => l.id === (ev.currentTarget as HTMLSelectElement).value) ?? null)}>
		<option value="">None</option>
		{#each luts as l (l.id)}<option value={l.id}>{l.name}</option>{/each}
		{#if edit.lut && !luts.some((l) => l.id === edit.lut?.id)}<option value={edit.lut.id}>{edit.lut.name} (not on this server)</option>{/if}
	</select>
	<button type="button" class="rounded-full bg-raised px-3 py-1 text-fg-2" disabled={busy} onclick={() => input?.click()}>{busy ? 'Uploading…' : 'Upload .cube'}</button>
	<input bind:this={input} type="file" accept=".cube" class="hidden" aria-label="Upload a .cube LUT" onchange={picked} />
</div>
{#if error}<p class="text-xs text-warning">{error}</p>{/if}
{#if edit.lut}
	{#if isMissing(edit.lut.id)}<p class="text-xs text-warning">This server doesn’t have “{edit.lut.name}”, so the picture shows without it and exports will fail until it is uploaded again.</p>{/if}
	<Slider id="lut-amount" label="Amount" value={edit.lut.amount} min={0} max={100} defaultValue={100} format={(v) => `${decimals(v)} %`} onchange={(v) => onchange((e) => { if (e.lut) e.lut.amount = v; })} {oncommit} />
	<div class="flex flex-wrap items-center gap-1.5 text-xs" role="radiogroup" aria-label="Built for">
		{#each SPACES as s (s.id)}
			<button type="button" role="radio" aria-checked={space.id === s.id} class={choice(space.id === s.id)} onclick={() => { onchange((e) => { if (e.lut) e.lut.space = s.id; }); oncommit(); }}>{s.label}</button>
		{/each}
	</div>
	<p class="text-xs text-muted">{space.note} A LUT applies after everything else, to the picture as displayed; pick the space it was built in, which the file doesn’t say.</p>
{/if}
