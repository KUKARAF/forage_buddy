// A single "get my current position" call that works in both build modes:
//  - Tauri (PUBLIC_APP_MODE === 'tauri'): native geolocation via
//    `@tauri-apps/plugin-geolocation` (dynamically imported — the web build
//    never touches Tauri APIs).
//  - Plain browser (web build, or the Tauri webview falling back): the
//    standard `navigator.geolocation` API.
//
// Both return the same shape (`coords.latitude/longitude/accuracy`), so the
// caller doesn't need to branch on the result.

export interface GeoResult {
	lat: number;
	lon: number;
	accuracyM: number | null;
}

export class GeolocationError extends Error {}

const IS_TAURI_MODE = (import.meta.env.PUBLIC_APP_MODE as string | undefined) === 'tauri';

/** Turn whatever the plugin rejected with into a readable message. Both
 * `checkPermissions`/`requestPermissions` and `getCurrentPosition` reject
 * outright (not just resolve with a "denied" status) when the device's
 * system-wide Location Services/GPS toggle is off — a different switch than
 * this app's own permission, and the single most likely reason "the app has
 * Location permission granted but still can't get a position" happens. */
function describeTauriGeoFailure(cause: unknown): string {
	const raw = cause instanceof Error ? cause.message : String(cause);
	if (/location.{0,20}(disabled|off|service)/i.test(raw) || /service.{0,20}disabled/i.test(raw)) {
		return 'Location services are turned off on this device — enable Location in your phone’s quick settings (not just this app’s permission), then try again.';
	}
	return raw || 'Could not get your location.';
}

async function getPositionTauri(): Promise<GeoResult> {
	const { checkPermissions, requestPermissions, getCurrentPosition } =
		await import('@tauri-apps/plugin-geolocation');

	let status: Awaited<ReturnType<typeof checkPermissions>>;
	try {
		status = await checkPermissions();
	} catch (cause) {
		throw new GeolocationError(describeTauriGeoFailure(cause));
	}
	if (status.location !== 'granted' && status.coarseLocation !== 'granted') {
		try {
			status = await requestPermissions(['location']);
		} catch (cause) {
			throw new GeolocationError(describeTauriGeoFailure(cause));
		}
		if (status.location !== 'granted' && status.coarseLocation !== 'granted') {
			throw new GeolocationError(
				'Location permission was denied. Enable it for Forage Buddy in your phone’s app settings.'
			);
		}
	}

	try {
		const pos = await getCurrentPosition({
			enableHighAccuracy: true,
			timeout: 15000,
			maximumAge: 0
		});
		return {
			lat: pos.coords.latitude,
			lon: pos.coords.longitude,
			accuracyM: pos.coords.accuracy ?? null
		};
	} catch (cause) {
		throw new GeolocationError(describeTauriGeoFailure(cause));
	}
}

function getPositionBrowser(): Promise<GeoResult> {
	return new Promise((resolve, reject) => {
		if (typeof navigator === 'undefined' || !navigator.geolocation) {
			reject(new GeolocationError('Geolocation is not available in this browser.'));
			return;
		}
		navigator.geolocation.getCurrentPosition(
			(pos) => {
				resolve({
					lat: pos.coords.latitude,
					lon: pos.coords.longitude,
					accuracyM: pos.coords.accuracy ?? null
				});
			},
			(err) => {
				reject(new GeolocationError(err.message || 'Could not get your location.'));
			},
			{ enableHighAccuracy: true, timeout: 15000, maximumAge: 0 }
		);
	});
}

/** Get the device's current position, trying the native plugin in the Tauri build. */
export async function getCurrentLocation(): Promise<GeoResult> {
	try {
		return await (IS_TAURI_MODE ? getPositionTauri() : getPositionBrowser());
	} catch (err) {
		if (err instanceof GeolocationError) throw err;
		throw new GeolocationError(err instanceof Error ? err.message : 'Could not get your location.');
	}
}
