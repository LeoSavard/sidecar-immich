<script lang="ts">
	import './layout.css';
	import favicon from '$lib/assets/favicon.svg';
	import { preventPageZoom } from '$lib/page-zoom';

	let { children } = $props();
	$effect(preventPageZoom);
</script>

<svelte:head><link rel="icon" href={favicon} /></svelte:head>
{@render children()}
