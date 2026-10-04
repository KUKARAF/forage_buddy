// Shared, reactive auth state (Svelte 5 runes in a .svelte.ts module).
//
// The root layout calls `ensureLoaded()` once on mount. A 401 (or network
// error) resolves `me` to `null` ("guest") rather than throwing — in
// FORAGEBUDDY_DEV_MODE the backend's /auth/me always succeeds as the implicit
// `admin` user, so this same bootstrap code runs unchanged in dev.
import { getMe, type Me } from './api/client';

class AuthState {
	me = $state<Me | null>(null);
	/** True once the initial /auth/me fetch has settled (success or not). */
	loaded = $state(false);
	/** Dedupes concurrent load() calls so /auth/me is fetched exactly once. */
	private loadPromise: Promise<void> | null = null;

	get isGuest(): boolean {
		return this.me === null;
	}

	/** Resolve the initial auth load exactly once, sharing a single in-flight
	 * promise across all callers. */
	ensureLoaded(): Promise<void> {
		if (this.loadPromise === null) this.loadPromise = this.load();
		return this.loadPromise;
	}

	/** Force a fresh auth load, discarding the memoized promise. Used after a
	 * sign-in completes (e.g. the app's device-token deep link arrives) so the
	 * UI re-fetches /auth/me and flips out of the guest state. */
	reload(): Promise<void> {
		this.loadPromise = this.load();
		return this.loadPromise;
	}

	private async load(): Promise<void> {
		try {
			this.me = await getMe();
		} catch {
			this.me = null;
		}
		this.loaded = true;
	}
}

export const auth = new AuthState();
