<!-- SPDX-License-Identifier: Apache-2.0
The phone's way in is the home page, as the desktop's is (owner's call, 2026-10-26): it imports a photo (into the
editing folder on a phone) and opens the editor, which develops it on the phone. The module stays for what only it
provides — LibRaw for the browser at /m/libraw/, and the installed app that starts here — so this page just goes
there, keeping whatever it was asked (`?inspect`, `?device`) for the editor. -->
<script lang="ts">
	import { onMount } from 'svelte';

	onMount(() => location.replace(`/${location.search}`));
</script>

<svelte:head><title>Sidecar</title></svelte:head>

<main class="grid min-h-dvh place-items-center bg-ground text-sm text-fg-2"><a href="/" class="underline underline-offset-4">Open Sidecar</a></main>
