// Delivers the device token from the OIDC login's custom-scheme deep link
// (`dev.foragebuddy.app://auth?token=<...>`) into the app. APP BUILD ONLY —
// the Tauri deep-link plugin is dynamically imported so the web build never
// touches Tauri APIs. Backend counterpart: the `?client=app` login flow
// (crates/server/src/auth/oidc.rs) mints a device token and redirects to
// `dev.foragebuddy.app://auth?token=<raw>`. The mobile crate registers the
// scheme (crates/mobile: tauri.conf.json `deep-link` + `tauri-plugin-deep-link`,
// capabilities `deep-link:default`).
import { setDeviceToken } from '$lib/api/deviceToken';

function tokenFromUrl(url: string): string | null {
	try {
		return new URL(url).searchParams.get('token');
	} catch {
		return null;
	}
}

/**
 * Wire up the deep-link auth handler. Processes a cold-start launch URL (the
 * app was opened by the deep link) AND listens for warm-start ones (app
 * already running). `onToken` runs after the token is stored, so the caller
 * can refresh the session and navigate. Returns an unlisten fn (a no-op if
 * setup failed).
 */
export async function initDeepLinkAuth(onToken: () => void | Promise<void>): Promise<() => void> {
	try {
		const { getCurrent, onOpenUrl } = await import('@tauri-apps/plugin-deep-link');

		const handle = (urls: string[] | null | undefined): boolean => {
			for (const url of urls ?? []) {
				const token = tokenFromUrl(url);
				if (token) {
					setDeviceToken(token);
					void onToken();
					return true;
				}
			}
			return false;
		};

		// Cold start: the deep link launched the app.
		try {
			handle(await getCurrent());
		} catch {
			// getCurrent unsupported/unavailable — the listener below still fires.
		}

		// Warm start: app already open when the link arrives.
		return await onOpenUrl((urls) => {
			handle(urls);
		});
	} catch (err) {
		console.error('Deep-link auth setup failed', err);
		return () => {};
	}
}
