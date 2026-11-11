<!-- SPDX-License-Identifier: Apache-2.0
Soft proofing (DESIGN.md §15, "Soft proofing"): the output profile to proof through — one of the server's, or an .icc
uploaded now — the intent into it, black point compensation, the paper and ink simulated, and the gamut warning. -->
<script lang="ts">
	import { list, upload, type ProofInfo, type ProofIntent, type ProofOptions } from '$lib/proofs';

	let { value, onchange }: { value: ProofOptions | null; onchange: (next: ProofOptions | null) => void } = $props();

	const INTENTS: { id: ProofIntent; label: string; note: string }[] = [
		{ id: 'perceptual', label: 'Perceptual', note: 'Squeezes the whole picture into the print’s gamut, keeping how colours relate.' },
		{ id: 'relative', label: 'Relative', note: 'Keeps every colour the print can hold exactly and clips the rest to its edge.' },
		{ id: 'saturation', label: 'Saturation', note: 'Keeps colours vivid at the cost of their accuracy, as for graphics.' }
	];

	let profiles = $state<ProofInfo[]>([]);
	let error = $state('');
	let busy = $state(false);
	let input: HTMLInputElement | undefined = $state();

	$effect(() => {
		list().then((l) => (profiles = l)).catch(() => (profiles = []));
	});

	const current = $derived<ProofOptions>(value ?? { id: '', intent: 'perceptual', bpc: true, paper: false, warn: true });
	const set = (patch: Partial<ProofOptions>) => onchange({ ...current, ...patch });

	async function picked() {
		const file = input?.files?.[0];
		if (!file) return;
		(busy = true), (error = '');
		try {
			const info = await upload(file);
			profiles = [...profiles.filter((p) => p.id !== info.id), info].sort((a, b) => a.name.localeCompare(b.name));
			set({ id: info.id });
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			busy = false;
			input!.value = '';
		}
	}

	const choice = (on: boolean) => `rounded-full px-2 py-0.5 ${on ? 'bg-fg text-ground' : 'bg-raised text-fg-2'}`;
	const intent = $derived(INTENTS.find((i) => i.id === current.intent)!);
</script>

<div class="grid max-w-md gap-1.5 rounded-lg bg-ground/90 p-2 text-[0.6875rem]" aria-label="Soft proof">
	<div class="flex flex-wrap items-center gap-1.5">
		<label class="sr-only" for="proof-profile">Proof profile</label>
		<select id="proof-profile" class="min-w-0 flex-1 rounded-md bg-raised px-2 py-1 text-fg" value={current.id} onchange={(ev) => set({ id: (ev.currentTarget as HTMLSelectElement).value })}>
			<option value="">Choose a printer profile…</option>
			{#each profiles as p (p.id)}<option value={p.id} title={p.description ?? undefined}>{p.name}</option>{/each}
		</select>
		<button type="button" class="rounded-full bg-raised px-2.5 py-1 text-fg-2" disabled={busy} onclick={() => input?.click()}>{busy ? 'Uploading…' : 'Upload .icc'}</button>
		<input bind:this={input} type="file" accept=".icc,.icm" class="hidden" aria-label="Upload an ICC profile" onchange={picked} />
	</div>
	{#if error}<p class="text-warning">{error}</p>{/if}
	{#if current.id}
		<div class="flex flex-wrap items-center gap-1" role="radiogroup" aria-label="Intent">
			{#each INTENTS as i (i.id)}
				<button type="button" role="radio" aria-checked={current.intent === i.id} class={choice(current.intent === i.id)} onclick={() => set({ intent: i.id })}>{i.label}</button>
			{/each}
		</div>
		<p class="text-muted">{intent.note}</p>
		<div class="flex flex-wrap items-center gap-1">
			<button type="button" aria-pressed={current.bpc} class={choice(current.bpc)} title="Map the display's black to the print's darkest, so shadows keep their detail" onclick={() => set({ bpc: !current.bpc })}>Black point compensation</button>
			<button type="button" aria-pressed={current.paper} class={choice(current.paper)} title="Show the paper's own white and the ink's own black, rather than the screen's" onclick={() => set({ paper: !current.paper })}>Paper and ink</button>
			<button type="button" aria-pressed={current.warn} class={choice(current.warn)} title="Grey where the print can't hold the colour" onclick={() => set({ warn: !current.warn })}>Gamut warning</button>
		</div>
	{/if}
</div>
