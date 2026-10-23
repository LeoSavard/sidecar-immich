<!-- SPDX-License-Identifier: Apache-2.0
The measurement under the histogram: how much of the rendering is at white or at black, and what the probed
place holds. A second column appears once the compared version has been seen, so flipping to it and back leaves
both renderings' numbers on screen (DESIGN.md §8). -->
<script lang="ts">
	import { asText, rows, type Reading } from '$lib/measure';

	type Props = {
		/** The rendering on screen, then the one last seen on the other side of the flip. */
		readings: Reading[];
		/** Armed: the next tap on the photo probes it. */
		probing: boolean;
		onprobe: (on: boolean) => void;
		/** Where the probe sits, so it can be cleared. */
		probed: boolean;
		onclear: () => void;
		title: string;
	};
	let { readings, probing, onprobe, probed, onclear, title }: Props = $props();

	let copied = $state(false);

	async function copy() {
		try {
			await navigator.clipboard.writeText(asText(title, readings));
			copied = true;
			setTimeout(() => (copied = false), 1500);
		} catch {
			copied = false;
		}
	}
</script>

<div class="grid gap-1 text-[0.6875rem]">
	<table class="w-full tabular-nums">
		<thead>
			<tr class="text-fg-2">
				<th class="w-1/2 text-left font-normal"></th>
				{#each readings as r (r.label)}
					<th class="text-right font-normal">{r.label}</th>
				{/each}
			</tr>
		</thead>
		<tbody>
			{#each rows(readings) as row (row.name)}
				<tr>
					<th scope="row" class="text-left font-normal text-fg-2">{row.name}</th>
					{#each row.values as v, i (i)}
						<td class="text-right">{v}</td>
					{/each}
				</tr>
			{/each}
		</tbody>
	</table>
	<div class="flex items-center gap-2">
		<button
			type="button"
			aria-pressed={probing}
			class="rounded-full px-2 py-0.5 {probing ? 'bg-fg text-ground' : 'bg-raised text-fg-2'}"
			title="Read one place in the picture: tap it on the photo"
			onclick={() => onprobe(!probing)}>Probe</button
		>
		{#if probed}
			<button type="button" class="rounded-full bg-raised px-2 py-0.5 text-fg-2" onclick={onclear}>Clear</button>
		{/if}
		<button type="button" class="ml-auto rounded-full bg-raised px-2 py-0.5 text-fg-2" onclick={copy}>{copied ? 'Copied' : 'Copy'}</button>
	</div>
</div>
