<!-- SPDX-License-Identifier: Apache-2.0
Export: saves the edit first, creates a job, and follows it over Server-Sent Events. The render runs on the
server whether or not this page stays open; reopening the sheet later shows the result. -->
<script lang="ts">
	type Settings = { format: 'jpeg' | 'tiff'; quality: number; longEdge: number | null; colorSpace: 'srgb' | 'display-p3' };
	type Props = { assetKey: string; colorSpace: Settings['colorSpace']; prepare: () => Promise<string | null>; onclose: () => void };
	let { assetKey, colorSpace, prepare, onclose }: Props = $props();

	const SIZES: [string, number | null][] = [['Full size', null], ['4096 px', 4096], ['2048 px', 2048], ['1024 px', 1024]];
	const STAGES: Record<string, string> = { decode: 'Reading the RAW', demosaic: 'Demosaicing', develop: 'Developing', resample: 'Resizing', encode: 'Encoding', metadata: 'Copying metadata', upload: 'Uploading to Immich', stack: 'Stacking with the original' };

	let settings = $state<Settings>(load());
	let phase = $state<'idle' | 'running' | 'done' | 'failed'>('idle');
	let stage = $state('');
	let overall = $state(0);
	let result = $state<{ file: string; immichAssetId?: string | null; download?: string | null; existing?: boolean } | null>(null);
	let error = $state('');
	let source: EventSource | null = null;

	// The space comes from the editor (the settings page choice), so the export matches what was on screen;
	// the picker below still overrides it for one export.
	function load(): Settings {
		const base: Settings = { format: 'jpeg', quality: 92, longEdge: null, colorSpace };
		try {
			const s = JSON.parse(localStorage.getItem('sidecar.export') ?? '');
			if (s && typeof s === 'object') return { ...base, ...s, colorSpace };
		} catch {}
		return base;
	}

	function follow(jobId: string) {
		source?.close();
		source = new EventSource(`/api/exports/${jobId}/events`);
		source.addEventListener('progress', (e) => {
			const d = JSON.parse((e as MessageEvent).data);
			stage = STAGES[d.stage] ?? d.stage;
			overall = d.overall;
		});
		source.addEventListener('done', (e) => {
			result = JSON.parse((e as MessageEvent).data);
			overall = 1;
			phase = 'done';
			source?.close();
		});
		source.addEventListener('failed', (e) => {
			error = JSON.parse((e as MessageEvent).data).message;
			phase = 'failed';
			source?.close();
		});
	}

	async function start() {
		try {
			localStorage.setItem('sidecar.export', JSON.stringify(settings));
		} catch {}
		phase = 'running';
		stage = 'Saving your edit';
		overall = 0;
		error = '';
		result = null;
		const etag = await prepare();
		if (!etag) {
			phase = 'failed';
			error = "The edit couldn't be saved, so there's nothing to export yet.";
			return;
		}
		const r = await fetch('/api/exports', {
			method: 'POST',
			headers: { 'content-type': 'application/json', 'idempotency-key': crypto.randomUUID() },
			body: JSON.stringify({ assetKey, editEtag: etag, ...settings })
		});
		if (r.status === 200) {
			const { existing } = await r.json();
			result = { ...existing, existing: true };
			phase = 'done';
			return;
		}
		if (!r.ok) {
			phase = 'failed';
			error = (await r.text()) || r.statusText;
			return;
		}
		const { jobId } = await r.json();
		follow(jobId);
	}

	$effect(() => () => source?.close());
</script>

<div class="fixed inset-0 z-20 flex items-end justify-center bg-ground/60 lg:items-center" role="presentation" onclick={(e) => e.target === e.currentTarget && phase !== 'running' && onclose()}>
	<div class="w-full max-w-md rounded-t-xl bg-panel p-5 pb-[max(1.25rem,env(safe-area-inset-bottom))] lg:rounded-xl" role="dialog" aria-modal="true" aria-labelledby="export-title">
		<div class="mb-4 flex items-center justify-between">
			<h2 id="export-title" class="text-base font-medium">Export</h2>
			<button type="button" class="rounded px-2 py-1 text-sm text-fg-2 hover:bg-raised" onclick={onclose}>{phase === 'running' ? 'Hide' : 'Close'}</button>
		</div>

		{#if phase === 'idle' || phase === 'failed'}
			<div class="grid gap-4 text-sm">
				<fieldset class="grid gap-2">
					<legend class="mb-1 text-xs text-fg-2">Format</legend>
					<div class="grid grid-cols-2 gap-2">
						{#each [['jpeg', 'JPEG'], ['tiff', 'TIFF, 16-bit']] as [value, label] (value)}
							<button type="button" aria-pressed={settings.format === value} class="h-10 rounded {settings.format === value ? 'bg-fg text-ground' : 'bg-raised'}" onclick={() => (settings.format = value as Settings['format'])}>{label}</button>
						{/each}
					</div>
				</fieldset>
				{#if settings.format === 'jpeg'}
					<label class="grid gap-1" for="export-quality">
						<span class="flex justify-between text-xs text-fg-2"><span>Quality</span><span class="text-fg tabular-nums">{settings.quality}</span></span>
						<input id="export-quality" type="range" min="60" max="100" bind:value={settings.quality} class="h-8 accent-[#c6c6c6]" />
					</label>
				{/if}
				<fieldset class="grid gap-2">
					<legend class="mb-1 text-xs text-fg-2">Size</legend>
					<div class="grid grid-cols-4 gap-2">
						{#each SIZES as [label, value] (label)}
							<button type="button" aria-pressed={settings.longEdge === value} class="h-10 rounded text-xs {settings.longEdge === value ? 'bg-fg text-ground' : 'bg-raised'}" onclick={() => (settings.longEdge = value)}>{label}</button>
						{/each}
					</div>
				</fieldset>
				<fieldset class="grid gap-2">
					<legend class="mb-1 text-xs text-fg-2">Colour</legend>
					<div class="grid grid-cols-2 gap-2">
						{#each [['srgb', 'sRGB'], ['display-p3', 'Display P3']] as [value, label] (value)}
							<button type="button" aria-pressed={settings.colorSpace === value} class="h-10 rounded {settings.colorSpace === value ? 'bg-fg text-ground' : 'bg-raised'}" onclick={() => (settings.colorSpace = value as Settings['colorSpace'])}>{label}</button>
						{/each}
					</div>
				</fieldset>
				{#if phase === 'failed'}<p class="text-danger">{error}</p>{/if}
				<button type="button" class="h-11 rounded bg-fg font-medium text-ground" onclick={start}>{phase === 'failed' ? 'Try again' : 'Export'}</button>
			</div>
		{:else if phase === 'running'}
			<div class="grid gap-2 text-sm" aria-live="polite">
				<p>{stage}…</p>
				<div class="h-1.5 overflow-hidden rounded-full bg-raised" role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow={Math.round(overall * 100)}>
					<div class="h-full bg-fg transition-[width]" style:width="{Math.round(overall * 100)}%"></div>
				</div>
				<p class="text-xs text-fg-2">The render continues on your server if you leave this page.</p>
			</div>
		{:else if result}
			<div class="grid gap-3 text-sm" aria-live="polite">
				<p>
					{#if result.existing}Already exported as <span class="font-mono">{result.file}</span>.
					{:else if result.immichAssetId}Saved to Immich as <span class="font-mono">{result.file}</span>, stacked with the original.
					{:else}Exported <span class="font-mono">{result.file}</span>.{/if}
				</p>
				{#if result.download}<a class="grid h-11 place-items-center rounded bg-fg font-medium text-ground" href={result.download} download>Download</a>{/if}
			</div>
		{/if}
	</div>
</div>
