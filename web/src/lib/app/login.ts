// Start the OIDC login, shared by every "Sign in" affordance.
//
// On the WEB build a normal same-tab navigation to the backend's login entry
// point (the session cookie flows back same-origin). On the APP build (Tauri
// Android), the webview is served from `tauri.localhost` while the backend
// lives on a remote origin, so cookies can't flow back. Instead we open the
// OS browser (a Chrome Custom Tab, via `@tauri-apps/plugin-opener`) to the
// `?client=app` flow; the backend mints a long-lived device token and
// redirects back into the app via the `dev.foragebuddy.app://auth?token=<raw>`
// deep link, captured by `$lib/app/deepLinkAuth`.
//
// The `@tauri-apps/plugin-opener` import is DYNAMIC and guarded by `IS_APP` so
// the web build never loads any Tauri API (there is no Tauri runtime there).
import { API_BASE_URL, loginUrl } from '$lib/api/client';
import { IS_APP } from '$lib/api/deviceToken';

export async function startLogin(): Promise<void> {
	if (IS_APP) {
		const { openUrl } = await import('@tauri-apps/plugin-opener');
		await openUrl(`${API_BASE_URL}/auth/login?client=app`);
	} else {
		window.location.href = loginUrl();
	}
}
