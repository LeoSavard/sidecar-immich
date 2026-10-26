<script lang="ts">
	import './layout.css';
	import { preventPageZoom } from '$lib/page-zoom';

	let { children } = $props();
	$effect(preventPageZoom);
</script>

{@render children()}
