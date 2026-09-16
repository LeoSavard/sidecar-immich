<!-- SPDX-License-Identifier: Apache-2.0 -->
<script lang="ts">
	const origin = typeof location !== 'undefined' ? location.origin : '';
	let copied = $state('');

	async function copy(text: string) {
		try {
			await navigator.clipboard.writeText(text);
			copied = text;
			setTimeout(() => copied === text && (copied = ''), 1500);
		} catch {
			copied = '';
		}
	}
</script>

<svelte:head><title>Set up the shortcut · Sidecar</title></svelte:head>

{#snippet chip(text: string)}
	<button type="button" class="inline-flex max-w-full items-center gap-2 rounded bg-raised px-2 py-1 font-mono text-xs break-all text-fg" onclick={() => copy(text)}>
		<span>{text}</span><span class="shrink-0 text-fg-2">{copied === text ? 'Copied' : 'Copy'}</span>
	</button>
{/snippet}

<main class="mx-auto min-h-dvh max-w-xl bg-ground px-5 pt-[max(1.5rem,env(safe-area-inset-top))] pb-[max(2rem,env(safe-area-inset-bottom))] text-fg">
	<a href="/" class="text-sm text-fg-2">← Sidecar</a>
	<h1 class="mt-3 text-xl">Edit in Sidecar — iPhone shortcut</h1>
	<p class="mt-2 text-sm text-fg-2">
		The shortcut reads the photo you share from Immich, sends its fingerprint, name and date to Sidecar, and opens it here. The photo itself isn't uploaded.
		Build it once in the Shortcuts app; it takes about five minutes.
	</p>

	<ol class="mt-6 grid gap-5 text-sm [counter-reset:step]">
		<li class="grid gap-2">
			<p><strong>New shortcut.</strong> In Shortcuts, tap <strong>+</strong>. Name it <em>Edit in Sidecar</em>. Tap the <strong>ⓘ</strong> (Details) button, turn on <strong>Show in Share Sheet</strong>, and set <strong>Receive</strong> to <em>Images</em> and <em>Files</em> only.</p>
		</li>
		<li class="grid gap-2">
			<p><strong>Date.</strong> Add <em>Get Details of Images</em> → choose <em>Date Taken</em> from <em>Shortcut Input</em>. Add <em>Format Date</em>: Date Format <em>ISO 8601</em>, turn on <em>Include ISO 8601 Time</em>.</p>
		</li>
		<li class="grid gap-2">
			<p><strong>Name.</strong> Add <em>Get Details of Files</em> → <em>Name</em> from <em>Shortcut Input</em>. Add <em>URL Encode</em> on that name.</p>
		</li>
		<li class="grid gap-2">
			<p><strong>Size.</strong> Add <em>Get Details of Files</em> → <em>File Size</em>. Add <em>Format File Size</em>: Format <em>Bytes</em>, turn off <em>Include Units</em>.</p>
		</li>
		<li class="grid gap-2">
			<p><strong>Fingerprint.</strong> Add <em>Generate Hash</em>: type <em>SHA1</em>, of <em>Shortcut Input</em>.</p>
		</li>
		<li class="grid gap-2">
			<p><strong>Link.</strong> Add <em>URL</em>. Type or paste these pieces in order, inserting the variable from each step after its piece (tap the variable bar above the keyboard):</p>
			<div class="grid gap-1.5 pl-1">
				<div class="flex flex-wrap items-center gap-2">{@render chip(`${origin}/open?sha1=`)} <span class="text-fg-2">then <em>Hash</em></span></div>
				<div class="flex flex-wrap items-center gap-2">{@render chip('&name=')} <span class="text-fg-2">then <em>URL Encoded Text</em></span></div>
				<div class="flex flex-wrap items-center gap-2">{@render chip('&taken=')} <span class="text-fg-2">then <em>Formatted Date</em></span></div>
				<div class="flex flex-wrap items-center gap-2">{@render chip('&size=')} <span class="text-fg-2">then <em>File Size</em></span></div>
			</div>
		</li>
		<li class="grid gap-2">
			<p><strong>Open.</strong> Add <em>Open URLs</em> with the <em>URL</em> from the previous step. Tap <strong>Done</strong>.</p>
		</li>
		<li class="grid gap-2">
			<p><strong>Try it.</strong> In Immich, open a RAW photo → Share → <em>Edit in Sidecar</em>. The first time, Shortcuts asks to allow sharing and opening the link; allow both. Sidecar opens in Safari, so stay signed in there.</p>
		</li>
	</ol>

	<p class="mt-6 rounded bg-panel p-3 text-xs text-fg-2">
		Tailscale has to be connected on the iPhone — Sidecar is only reachable inside your VPN. If the date can't be read, Sidecar still finds the photo by its name and fingerprint.
	</p>
</main>
