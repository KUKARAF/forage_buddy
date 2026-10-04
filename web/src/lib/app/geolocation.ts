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

async function getPositionTauri(): Promise<GeoResult> {
	const { getCurrentPosition } = await import('@tauri-apps/plugin-geolocation');
	const pos = await getCurrentPosition();
	return {
		lat: pos.coords.latitude,
		lon: pos.coords.longitude,
		accuracyM: pos.coords.accuracy ?? null
	};
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
