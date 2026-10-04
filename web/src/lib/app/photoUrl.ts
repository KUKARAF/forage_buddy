// Resolving a displayable <img> source for `GET /api/photos/{id}/file`.
//
// That endpoint is auth-gated (session cookie OR device-token bearer). On the
// web build, the cookie rides along with a plain `<img src="...">` for free.
// On the Tauri build, auth is a bearer token that a bare `<img src>` can't
// attach — so there we fetch the bytes ourselves and hand back an object URL.
// Both paths are exposed behind one function so components don't need to
// branch on build mode themselves.
import { API_BASE_URL, fetchPhotoBlob } from '$lib/api/client';
import { IS_APP } from '$lib/api/deviceToken';

export function photoFileUrl(photoId: string): string {
	return `${API_BASE_URL}/api/photos/${encodeURIComponent(photoId)}/file`;
}

/**
 * Resolve a usable `<img src>` for a photo. In Tauri/bearer mode this fetches
 * the bytes and returns a `blob:` object URL — callers MUST revoke it
 * (`URL.revokeObjectURL`) once done (e.g. in `onDestroy`) to avoid leaking
 * memory. In web/cookie mode it returns the direct URL and there is nothing
 * to revoke.
 */
export async function resolvePhotoSrc(photoId: string): Promise<string> {
	if (!IS_APP) return photoFileUrl(photoId);
	const blob = await fetchPhotoBlob(photoId);
	return URL.createObjectURL(blob);
}
