import adapter from '@sveltejs/adapter-static';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

/** @type {import('@sveltejs/kit').Config} */
const config = {
	preprocess: vitePreprocess(),
	kit: {
		// Static adapter: this app is a pure client that talks to a remote backend
		// over HTTP. It's built once and served identically as plain static files
		// and from inside a Tauri-wrapped Android build, so there's no Node/edge
		// server. `fallback: '200.html'` makes it an SPA — any unknown path is
		// served the fallback and resolved client-side.
		// See https://svelte.dev/docs/kit/adapters for more information.
		adapter: adapter({
			pages: 'build',
			assets: 'build',
			fallback: '200.html',
			precompress: false,
			strict: true
		})
	}
};

export default config;
