<script lang="ts">
	import '../app.css';
	import { onMount } from 'svelte';
	import { page } from '$app/state';
	import { resolve } from '$app/paths';
	import { auth } from '$lib/authState.svelte';
	import { IS_APP } from '$lib/api/deviceToken';
	import { startLogin } from '$lib/app/login';
	import { theme } from '$lib/theme.svelte';
	import { getCurrentLocation } from '$lib/app/geolocation';
	import ThemeToggle from '$lib/components/ThemeToggle.svelte';

	let { children } = $props();

	// Short commit SHA baked in at build time (unset in local dev). Shown in
	// a small footer so "are you sure you're on the latest build" is never a
	// guessing game when debugging a report.
	const buildSha = import.meta.env.PUBLIC_BUILD_SHA as string | undefined;

	// Best-effort, non-blocking permission priming: surfaces the OS/browser
	// prompts early (on app boot) rather than waiting for the user to first
	// hit "Use my location" / the camera on /sightings/new. Both fail silently
	// — the app already works fine requesting these on demand, so a denied or
	// unsupported prompt here must never break boot.
	async function primeGeolocationPermission() {
		try {
			await getCurrentLocation();
		} catch (err) {
			console.warn('Geolocation permission priming failed (non-fatal):', err);
		}
	}

	async function primeCameraPermission() {
		try {
			if (typeof navigator === 'undefined' || !navigator.mediaDevices?.getUserMedia) return;
			const stream = await navigator.mediaDevices.getUserMedia({ video: true });
			for (const track of stream.getTracks()) track.stop();
		} catch (err) {
			console.warn('Camera permission priming failed (non-fatal):', err);
		}
	}

	onMount(() => {
		const unlistenTheme = theme.init();
		let unlistenDeepLink: (() => void) | undefined;

		void primeGeolocationPermission();
		// getUserMedia never actually works in the Android app's WebView today
		// (see the capture-flow comments in sightings/new & sightings/[id]) —
		// priming it there would just open and immediately close the camera
		// hardware for nothing. Only worth doing in the web build.
		if (!IS_APP) void primeCameraPermission();

		void (async () => {
			// App build: the OIDC login returns the device token via the
			// `dev.foragebuddy.app://auth?token=<raw>` deep link. Register the
			// handler (and process any cold-start launch URL) before the initial
			// load so a launch via the login deep link lands signed in. When a
			// token arrives, re-fetch /auth/me so the guest state flips to
			// signed-in.
			if (IS_APP) {
				const { initDeepLinkAuth } = await import('$lib/app/deepLinkAuth');
				unlistenDeepLink = await initDeepLinkAuth(() => auth.reload());
			}
			await auth.ensureLoaded();
			if (auth.isGuest) {
				// Web build: full-page nav to the backend's OIDC login (cookie
				// flows back same-origin on return). App build: opens the OS
				// browser to the device-token flow and waits for the deep link
				// above — the webview itself never navigates away.
				await startLogin();
			}
		})();

		return () => {
			unlistenTheme();
			unlistenDeepLink?.();
		};
	});

	const home = resolve('/');
	const settings = resolve('/settings');
	const path = $derived(page.url.pathname);
</script>

{#if !auth.loaded}
	<div class="boot-screen">
		<p>Loading Forage Buddy…</p>
	</div>
{:else if auth.isGuest}
	<div class="boot-screen">
		<p>Redirecting you to sign in…</p>
		<button class="btn secondary" onclick={() => void startLogin()}>Open sign-in</button>
	</div>
{:else}
	<div class="shell">
		<header class="topbar">
			<a class="brand" href={home}>
				<span aria-hidden="true">🌿</span> Forage Buddy
			</a>
			<div class="topbar-right">
				{#if path !== home}
					<a class="back" href={home}>← All sightings</a>
				{/if}
				{#if path !== settings}
					<a class="settings-link" href={settings} aria-label="Settings">⚙ Settings</a>
				{/if}
				<ThemeToggle />
			</div>
		</header>
		<main class="content">
			{@render children()}
		</main>
		{#if buildSha}
			<footer class="build-footer">build {buildSha}</footer>
		{/if}
	</div>
{/if}

<style>
	.boot-screen {
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 16px;
		min-height: 100vh;
		padding: 24px;
		text-align: center;
	}

	.shell {
		min-height: 100vh;
		display: flex;
		flex-direction: column;
	}

	.topbar {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 12px;
		padding: calc(16px + var(--safe-top)) calc(20px + var(--safe-right)) 16px
			calc(20px + var(--safe-left));
		border-bottom: 1px solid var(--line);
		background: var(--card);
		position: sticky;
		top: 0;
		z-index: 10;
	}
	.brand {
		font-weight: 800;
		font-size: 18px;
		letter-spacing: -0.01em;
		color: var(--accent);
		display: inline-flex;
		align-items: center;
		gap: 8px;
	}
	.topbar-right {
		display: flex;
		align-items: center;
		gap: 14px;
	}
	.back {
		font-weight: 600;
		font-size: 14px;
		color: var(--muted);
	}
	.back:hover {
		color: var(--accent);
	}
	.settings-link {
		font-weight: 600;
		font-size: 14px;
		color: var(--muted);
	}
	.settings-link:hover {
		color: var(--accent);
	}

	.content {
		flex: 1;
		width: 100%;
		max-width: 760px;
		margin: 0 auto;
		padding: 20px calc(16px + var(--safe-right)) calc(48px + var(--safe-bottom))
			calc(16px + var(--safe-left));
	}

	.build-footer {
		text-align: center;
		font-size: 11px;
		color: var(--muted);
		opacity: 0.6;
		padding: 0 0 calc(10px + var(--safe-bottom));
	}
</style>
