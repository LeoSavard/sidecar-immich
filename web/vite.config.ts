import tailwindcss from '@tailwindcss/vite';
import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

// `dev/serve --web` sets this: Vite serves the app with live reload and hands the api's routes to the server.
const api = (globalThis as { process?: { env: Record<string, string | undefined> } }).process?.env.SIDECAR_API;

export default defineConfig({
	plugins: [
		tailwindcss(),
		sveltekit({
			compilerOptions: {
				// Force runes mode for the project, except for libraries. Can be removed in svelte 6.
				runes: ({ filename }) => filename.split(/[/\\]/).includes('node_modules') ? undefined : true
			},
			// Single-page app: the editor needs WebGPU and runs entirely in the browser.
			adapter: adapter({ fallback: 'index.html' })
		})
	],
	server: api ? { proxy: { '/api': api, '/open': api } } : undefined
});
