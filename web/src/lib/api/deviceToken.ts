// Device-token auth for the Tauri app build.
//
// The website relies on the backend's session cookie, but the Android app's
// webview loads the UI from `http://tauri.localhost` while the backend lives
// on a remote origin — third-party cookie rules make cookie auth unreliable
// there. Instead, the app build runs the OIDC flow and the backend hands back
// a long-lived *device token*, which is stored here and sent as
// `Authorization: Bearer <token>` on every request (see `client.ts`).
//
// On the website build none of this is active: `IS_APP` is false and no
// token is ever stored, so the Authorization header is simply never added.

/**
 * True when this bundle was built for the Tauri app (`PUBLIC_APP_MODE=tauri`
 * set at build time). The website build leaves it unset ("web").
 */
export const IS_APP: boolean = (import.meta.env.PUBLIC_APP_MODE as string | undefined) === 'tauri';

const STORAGE_KEY = 'foragebuddy_device_token';

/** The stored device token, or `null` when absent (website build / logged out). */
export function getDeviceToken(): string | null {
	// Guard for SSR / environments without `window` — SvelteKit runs
	// module/top-level code during prerender where `localStorage` doesn't exist.
	if (typeof window === 'undefined') return null;
	try {
		return localStorage.getItem(STORAGE_KEY);
	} catch {
		// localStorage unavailable (privacy mode etc.) — behave as logged out
		return null;
	}
}

export function setDeviceToken(token: string): void {
	if (typeof window === 'undefined') return;
	try {
		localStorage.setItem(STORAGE_KEY, token);
	} catch {
		// best-effort only
	}
}

export function clearDeviceToken(): void {
	if (typeof window === 'undefined') return;
	try {
		localStorage.removeItem(STORAGE_KEY);
	} catch {
		// best-effort only
	}
}
