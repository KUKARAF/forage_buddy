// Shared, reactive theme state (Svelte 5 runes in a .svelte.ts module), same
// pattern as `authState.svelte.ts`.
//
// Three user-facing states: 'system' (default, follows the OS/browser
// `prefers-color-scheme`), 'light', 'dark'. The *actual* applied theme is
// `effective` ('light' | 'dark'), derived from `preference` plus the OS
// setting when `preference === 'system'`.
//
// A tiny inline script in `app.html` applies the right class to <html>
// before first paint (see the comment there for why) using the exact same
// localStorage key and resolution rule as this module, so there's no flash
// of the wrong theme on load and no visible flicker once Svelte takes over.
const STORAGE_KEY = 'forage-buddy-theme';

export type ThemePreference = 'system' | 'light' | 'dark';
export type EffectiveTheme = 'light' | 'dark';

function isThemePreference(value: unknown): value is ThemePreference {
	return value === 'system' || value === 'light' || value === 'dark';
}

function prefersDark(): boolean {
	return typeof window !== 'undefined' && window.matchMedia('(prefers-color-scheme: dark)').matches;
}

function readStoredPreference(): ThemePreference {
	if (typeof localStorage === 'undefined') return 'system';
	try {
		const stored = localStorage.getItem(STORAGE_KEY);
		return isThemePreference(stored) ? stored : 'system';
	} catch {
		return 'system';
	}
}

class ThemeState {
	preference = $state<ThemePreference>(readStoredPreference());
	/** Mirrors the OS preference; kept live via a matchMedia listener so
	 *  'system' mode reacts to the OS theme changing without a reload. */
	private systemPrefersDark = $state(prefersDark());
	private initialized = false;

	get effective(): EffectiveTheme {
		if (this.preference === 'light') return 'light';
		if (this.preference === 'dark') return 'dark';
		return this.systemPrefersDark ? 'dark' : 'light';
	}

	/** Call once from the root layout's onMount. Wires the OS-theme listener
	 * and an $effect that keeps <html class="dark"> and localStorage in sync
	 * with `preference`/`effective` from then on. */
	init(): () => void {
		if (this.initialized) return () => {};
		this.initialized = true;

		const media = window.matchMedia('(prefers-color-scheme: dark)');
		const onChange = (e: MediaQueryListEvent) => {
			this.systemPrefersDark = e.matches;
		};
		media.addEventListener('change', onChange);

		const unsubscribe = $effect.root(() => {
			$effect(() => {
				document.documentElement.classList.toggle('dark', this.effective === 'dark');
				try {
					localStorage.setItem(STORAGE_KEY, this.preference);
				} catch {
					// Storage unavailable (private mode, etc.) — theme just won't
					// persist across sessions; not worth surfacing to the user.
				}
			});
		});

		return () => {
			media.removeEventListener('change', onChange);
			unsubscribe();
		};
	}

	setPreference(next: ThemePreference): void {
		this.preference = next;
	}

	/** Cycles system -> light -> dark -> system, for a single toggle button. */
	cycle(): void {
		const order: ThemePreference[] = ['system', 'light', 'dark'];
		const next = order[(order.indexOf(this.preference) + 1) % order.length];
		this.preference = next;
	}
}

export const theme = new ThemeState();
