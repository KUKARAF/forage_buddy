// Session-scoped "I understand the AI disclaimer" flag for SafetyAckModal.
// Frontend-only UX gate — there is deliberately no backend token/endpoint
// behind this; it just controls whether the modal needs to show again this
// session. sessionStorage (not localStorage) so it re-prompts on a fresh
// session/tab, matching the "once per session" requirement.
const STORAGE_KEY = 'forage-buddy:identification-safety-ack';

export function hasAcknowledgedSafety(): boolean {
	try {
		return sessionStorage.getItem(STORAGE_KEY) === '1';
	} catch {
		// Storage disabled (private mode, etc.) — fail open to "not yet
		// acknowledged" so the modal just shows again; never crash the page.
		return false;
	}
}

export function acknowledgeSafety(): void {
	try {
		sessionStorage.setItem(STORAGE_KEY, '1');
	} catch {
		// Ignore — see hasAcknowledgedSafety above.
	}
}
