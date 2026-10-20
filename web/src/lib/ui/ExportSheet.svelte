<!-- SPDX-License-Identifier: Apache-2.0
Export: saves the edit first, creates a job, and follows it over Server-Sent Events. The render runs on the
server whether or not this page stays open; reopening the sheet later shows the result. -->
<script lang="ts">
	type Destination = 'immich' | 'device' | 'both';
	type Settings = { format: 'jpeg' | 'tiff'; quality: number; longEdge: number | null; colorSpace: 'srgb' | 'display-p3'; destination: Destination };
	type Props = { assetKey: string; colorSpace: Settings['colorSpace']; prepare: () => Promise<string | null>; onclose: () => void };
	let { assetKey, colorSpace, prepare, onclose }: Props = $props();

	const SIZES: [string, number | null][] = [['Full size', null], ['4096 px', 4096], ['2048 px', 2048], ['1024 px', 1024]];
	const STAGES: Record<string, string> = { decode: 'Reading the RAW', demosaic: 'Demosaicing', develop: 'Developing', resample: 'Resizing', encode: 'Encoding', metadata: 'Copying metadata', upload: 'Uploading to Immich', stack: 'Stacking with the original' };
	const DESTINATIONS: [Destination, string][] = [['immich', 'Immich'], ['device', 'This device'], ['both', 'Both']];

	let settings = $state<Settings>(load());
	let phase = $state<'idle' | 'running' | 'done' | 'failed'>('idle');
	let stage = $state('');
	let overall = $state(0);
	let result = $state<{ file: string; immichAssetId?: string | null; download?: string | null; existing?: boolean } | null>(null);
	let error = $state('');
	let warning = $state('');
	let saving = $state(false);
	let immich = $state(true);
	// Set when the render was started to put the file on this device: the share sheet opens as soon as it
	// finishes, instead of asking for a second tap on a button that has just been replaced by itself.
	let thenSave = $state(false);
	let source: EventSource | null = null;

	// Without an Immich library there is nowhere to upload to, so the device is the only destination and the
	// picker would be a choice of one.
	$effect(() => {
		fetch('/api/session')
			.then((r) => (r.ok ? r.json() : null))
			.then((s) => {
				if (s && s.immich === false) {
					immich = false;
					settings.destination = 'device';
				}
			})
			.catch(() => {});
	});

	// The space comes from the editor (the settings page choice), so the export matches what was on screen;
	// the picker below still overrides it for one export.
	function load(): Settings {
		const base: Settings = { format: 'jpeg', quality: 92, longEdge: null, colorSpace, destination: 'immich' };
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
			if (thenSave) {
				thenSave = false;
				saveToDevice();
			}
		});
		// The render and any upload succeeded, but something after them didn't — recording the version in the
		// sidecar, most often. Worth saying, not worth throwing the export away over.
		source.addEventListener('warning', (e) => {
			warning = JSON.parse((e as MessageEvent).data).message;
		});
		source.addEventListener('failed', (e) => {
			error = JSON.parse((e as MessageEvent).data).message;
			phase = 'failed';
			source?.close();
		});
	}

	/**
	 * Export with the sheet's settings. `destination` overrides the picker for this one render without
	 * becoming the remembered choice: it is how a file already in one place is sent to the other.
	 */
	async function start(destination: Destination = settings.destination, save = false) {
		try {
			localStorage.setItem('sidecar.export', JSON.stringify(settings));
		} catch {}
		thenSave = save;
		phase = 'running';
		stage = 'Saving your edit';
		overall = 0;
		error = '';
		warning = '';
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
			body: JSON.stringify({ assetKey, editEtag: etag, ...settings, destination })
		});
		if (r.status === 200) {
			const { existing } = await r.json();
			result = { ...existing, existing: true };
			thenSave = false;
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

	/**
	 * Put the finished file on the device this page is running on. The share sheet is the one that ends in
	 * "Save Image" on a phone, so it's tried first; a browser without it — or one that refuses because the
	 * fetch cost us the user gesture — falls back to an ordinary download of the same URL.
	 */
	async function saveToDevice() {
		if (!result?.download) return;
		saving = true;
		error = '';
		try {
			const r = await fetch(result.download);
			if (!r.ok) throw new Error((await r.text()) || r.statusText);
			const blob = await r.blob();
			const file = new File([blob], result.file, { type: blob.type || 'application/octet-stream' });
			if (navigator.canShare?.({ files: [file] })) {
				await navigator.share({ files: [file], title: result.file });
				return;
			}
			const url = URL.createObjectURL(blob);
			const a = document.createElement('a');
			a.href = url;
			a.download = result.file;
			a.click();
			setTimeout(() => URL.revokeObjectURL(url), 30_000);
		} catch (e) {
			// Dismissing the share sheet is a choice, not a failure.
			if (e instanceof DOMException && e.name === 'AbortError') return;
			if (e instanceof DOMException && e.name === 'NotAllowedError') {
				window.location.href = result.download;
				return;
			}
			error = e instanceof Error ? e.message : String(e);
		} finally {
			saving = false;
		}
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
				{#if immich}
					<fieldset class="grid gap-2">
						<legend class="mb-1 text-xs text-fg-2">Save to</legend>
						<div class="grid grid-cols-3 gap-2">
							{#each DESTINATIONS as [value, label] (value)}
								<button type="button" aria-pressed={settings.destination === value} class="h-10 rounded text-xs {settings.destination === value ? 'bg-fg text-ground' : 'bg-raised'}" onclick={() => (settings.destination = value)}>{label}</button>
							{/each}
						</div>
					</fieldset>
				{/if}
				{#if phase === 'failed'}<p class="text-danger">{error}</p>{/if}
				<button type="button" class="h-11 rounded bg-fg font-medium text-ground" onclick={() => start()}>{phase === 'failed' ? 'Try again' : 'Export'}</button>
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
					{#if result.existing}Already in Immich as <span class="font-mono">{result.file}</span>.
					{:else if result.immichAssetId}Saved to Immich as <span class="font-mono">{result.file}</span>, stacked with the original.
					{:else}Exported <span class="font-mono">{result.file}</span>.{/if}
				</p>
				{#if warning}<p class="text-warning">{warning}</p>{/if}
				{#if error}<p class="text-danger">{error}</p>{/if}
				{#if result.download}
					<button type="button" class="h-11 rounded bg-fg font-medium text-ground disabled:opacity-60" disabled={saving} onclick={saveToDevice}>
						{saving ? 'Preparing…' : 'Save to this device'}
					</button>
				{:else if result.existing}
					<!-- In Immich, with the local copy dropped: it has to be rendered again before it can be
					     handed over, so the button says what it does for the person, not what that takes. -->
					<button type="button" class="h-11 rounded bg-fg font-medium text-ground" onclick={() => start('both', true)}>Save to this device</button>
				{/if}
				{#if immich && !result.immichAssetId}
					<!-- The same version, from the same edit, under the same name: one more place, not one more
					     version. -->
					<button type="button" class="h-11 rounded bg-raised font-medium" onclick={() => start('both')}>Also save to Immich</button>
				{/if}
			</div>
		{/if}
	</div>
</div>
