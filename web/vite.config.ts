import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
	// Expose PUBLIC_-prefixed env vars on `import.meta.env` so they are
	// statically inlined at build time (client.ts reads PUBLIC_API_BASE_URL,
	// deviceToken.ts reads PUBLIC_APP_MODE). Vite's default prefix is only
	// VITE_ — without this, `import.meta.env.PUBLIC_*` is silently undefined in
	// the built bundle and the Android app build ends up identical to the
	// website build (wrong API origin).
	envPrefix: ['VITE_', 'PUBLIC_'],
	plugins: [sveltekit()]
});
