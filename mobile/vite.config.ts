import tailwindcss from '@tailwindcss/vite';
import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

// `SIDECAR_API` points the dev server at a running Sidecar, as `dev/serve --web` does for the main app.
const api = (globalThis as { process?: { env: Record<string, string | undefined> } }).process?.env.SIDECAR_API;

export default defineConfig({
	plugins: [
		tailwindcss(),
		sveltekit({
			compilerOptions: {
				runes: ({ filename }) => (filename.split(/[/\\]/).includes('node_modules') ? undefined : true)
			},
			adapter: adapter({ fallback: 'index.html' }),
			// Served beside the main app, at /m, by an image built with a -mobile target (docs/MOBILE.md).
			paths: { base: '/m' },
			// The editor's engine, controls and api client are the main app's own `$lib`, used as they are:
			// the module owns the phone's flow, never a second copy of the pipeline.
			files: { lib: '../web/src/lib' }
		})
	],
	// The shared components import `svelte` from beside themselves, which is web/node_modules: one runtime, not
	// two, or a component from the main app and one from here would not share reactivity.
	resolve: { dedupe: ['svelte'] },
	server: {
		fs: { allow: ['..'] },
		proxy: api ? { '/api': api, '/open': api } : undefined
	}
});
