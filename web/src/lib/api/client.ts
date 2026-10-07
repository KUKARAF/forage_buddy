// Minimal fetch wrapper + typed helpers for talking to the Forage Buddy
// backend. See docs/ARCHITECTURE.md ("REST API surface") for the pinned
// contract this file builds against.
//
// The base URL can be overridden at build time via `PUBLIC_API_BASE_URL` (see
// Vite's `import.meta.env`) — set in `.env.development` so `npm run dev` keeps
// talking to the separately-running backend on :8080. When unset (the
// production build: the backend serves these static assets itself, same
// origin, no separate dev server), it falls back to `window.location.origin`
// so the built bundle isn't hardcoded to any one domain. This app is
// `ssr = false`, so `window` is always available by the time these run.
export const API_BASE_URL: string =
	(import.meta.env.PUBLIC_API_BASE_URL as string | undefined) ??
	(typeof window !== 'undefined' ? window.location.origin : '');

import { getDeviceToken } from './deviceToken';

/** Thrown by the request helpers below on any non-2xx response or network failure. */
export class ApiError extends Error {
	/** HTTP status code, or 0 if the request never reached the server (network error). */
	readonly status: number;
	/** Parsed JSON error body, if the response had one and was JSON. */
	readonly body: unknown;

	constructor(message: string, status: number, body?: unknown) {
		super(message);
		this.name = 'ApiError';
		this.status = status;
		this.body = body;
	}
}

export interface RequestOptions {
	/** Extra headers to merge into the request. */
	headers?: Record<string, string>;
	/** AbortSignal for cancellation (combined with the default timeout below, not a replacement for it). */
	signal?: AbortSignal;
	/** Override the default request timeout (ms). */
	timeoutMs?: number;
}

function authHeaders(): Record<string, string> {
	const deviceToken = getDeviceToken();
	return deviceToken !== null ? { Authorization: `Bearer ${deviceToken}` } : {};
}

// A field app with no client-side request timeout is a trap: on a flaky
// outdoor connection, a stalled `fetch()` just hangs forever with no error,
// which is exactly what it looks like to be stuck on "Loading sighting…"
// indefinitely — the app's existing error + "Try again" UI never gets a
// chance to show because nothing ever rejects. Every request below gets a
// default timeout so a stall always eventually surfaces as a retryable
// error instead. `AbortSignal.timeout`/`AbortSignal.any` are both
// well-supported in current browsers and Android WebView.
const DEFAULT_TIMEOUT_MS = 20_000;
const UPLOAD_TIMEOUT_MS = 60_000;
// Identification calls out to an LLM and can legitimately take tens of
// seconds; the backend's own request timeout (routes.rs) is 120s, so the
// client timeout is set a bit above that rather than racing it.
const LONG_RUNNING_TIMEOUT_MS = 130_000;

function timeoutSignal(callerSignal: AbortSignal | undefined, ms: number): AbortSignal {
	const timeout = AbortSignal.timeout(ms);
	return callerSignal ? AbortSignal.any([callerSignal, timeout]) : timeout;
}

/** `AbortSignal.timeout()` rejects with a `TimeoutError` DOMException — give
 * that case a clear, actionable message instead of the generic network-error
 * text, since "the connection stalled" and "you're offline" read very
 * differently to someone standing in the woods with one signal bar. */
function describeFetchFailure(method: string, url: string, cause: unknown): string {
	if (cause instanceof DOMException && cause.name === 'TimeoutError') {
		return 'Request timed out — check your connection and try again.';
	}
	return `Network error while requesting ${method} ${url}`;
}

export async function request<TResponse>(
	method: 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE',
	path: string,
	body?: unknown,
	options: RequestOptions = {}
): Promise<TResponse> {
	const url = path.startsWith('http') ? path : `${API_BASE_URL}${path}`;

	let response: Response;
	try {
		response = await fetch(url, {
			method,
			// Session auth is cookie-based (set by the backend after OIDC login),
			// so credentials must be included on every request, including
			// cross-origin ones (e.g. Tauri webview talking to a remote host).
			credentials: 'include',
			headers: {
				...(body !== undefined ? { 'Content-Type': 'application/json' } : {}),
				Accept: 'application/json',
				// App build: authenticate with the stored device token (cookies
				// don't reliably cross the tauri.localhost <-> backend origin
				// split). Spread before options.headers so callers can override.
				...authHeaders(),
				...options.headers
			},
			body: body !== undefined ? JSON.stringify(body) : undefined,
			signal: timeoutSignal(options.signal, options.timeoutMs ?? DEFAULT_TIMEOUT_MS)
		});
	} catch (cause) {
		throw new ApiError(
			describeFetchFailure(method, url, cause),
			0,
			cause instanceof Error ? cause.message : cause
		);
	}

	if (response.status === 204) {
		return undefined as TResponse;
	}

	const contentType = response.headers.get('content-type') ?? '';
	const isJson = contentType.includes('application/json');
	const payload = isJson ? await response.json().catch(() => undefined) : undefined;

	if (!response.ok) {
		// Every error response is {"message": string} (docs/ARCHITECTURE.md).
		const message =
			(isJson && payload && typeof payload === 'object' && 'message' in payload
				? String((payload as Record<string, unknown>).message)
				: undefined) ?? `Request failed with status ${response.status}`;
		throw new ApiError(message, response.status, payload);
	}

	return payload as TResponse;
}

// --- Domain types (mirror docs/ARCHITECTURE.md + crates/core/src/domain.rs) --

export type SightingStatus = 'open' | 'archived';
/**
 * Status of the single automatic identification result for a sighting
 * (replaces the old separate triage/deep-dive flow). `pending`/`partial` are
 * both "still working" states the UI treats identically (show a spinner,
 * keep polling); `insufficient` means the backend needs more photos/info;
 * `failed` means the run errored and a manual retry (`runIdentification`) is
 * offered; `complete` means `candidates` is ready to render.
 */
export type IdentificationStatus = 'pending' | 'partial' | 'complete' | 'insufficient' | 'failed';
/** Confusant danger scale, per the new identification contract. */
export type DangerLevel = 'unknown' | 'mild' | 'toxic' | 'deadly_toxic';

/** The authenticated user (GET /auth/me). In FORAGEBUDDY_DEV_MODE this is
 * always the implicit "admin" user, so the same bootstrap code works
 * unchanged in dev. */
export interface Me {
	id: string;
	email: string | null;
	display_name: string | null;
}

export interface Sighting {
	id: string;
	status: SightingStatus;
	lat: number | null;
	lon: number | null;
	location_accuracy_m: number | null;
	place_label: string | null;
	observed_at: string;
	notes: string | null;
	created_at: string;
	updated_at: string;
}

/**
 * GET /api/sightings row shape: the sighting plus a cheap summary of its
 * latest identification attempt (status, top-confidence candidate's raw
 * species name, and the single highest-severity danger signal across that
 * attempt's candidates/confusants), confirmed against
 * `crates/server/src/sightings/mod.rs`'s `SightingSummary`.
 */
export interface SightingListItem {
	id: string;
	created_at: string;
	observed_at: string;
	place_label: string | null;
	lat: number | null;
	lon: number | null;
	status: SightingStatus;
	photo_count: number;
	thumbnail_photo_id: string | null;
	latest_identification_status?: IdentificationStatus | null;
	/** Raw `species` name of the top-confidence candidate (not common-name-resolved). */
	latest_identification_species?: string | null;
	latest_identification_danger_level?: DangerLevel | null;
}

export interface Photo {
	id: string;
	sighting_id: string;
	content_type: string;
	width: number | null;
	height: number | null;
	taken_at: string;
	sort_order: number;
	created_at: string;
}

/** A look-alike species listed on an identification candidate — rendered as
 * a single short inline chip, never a bulleted checklist of distinguishing
 * features (that pattern is exactly the "wall of text" this replaced). */
export interface Confusant {
	species: string;
	danger_level: DangerLevel;
	/** Short phrase, may be empty. */
	note: string;
	wikipedia_url?: string | null;
}

/** One possible species for a sighting's identification result. `edible`/
 * `medicinal`/`psychoactive`/`poisonous` are tri-state: `null` means
 * "unknown", not "false" — render a neutral state for it, never treat it as
 * a negative answer. */
export interface IdentificationCandidate {
	species: string;
	common_name: string | null;
	confidence: number;
	edible: boolean | null;
	medicinal: boolean | null;
	psychoactive: boolean | null;
	poisonous: boolean | null;
	wikipedia_url?: string | null;
	/** Short phrase, may be empty — never a paragraph. */
	risk_note: string;
	confusants: Confusant[];
}

/** The one automatic identification result for a sighting (replaces the old
 * separate triage + manually-triggered deep-dive results). */
export interface IdentificationResult {
	status: IdentificationStatus;
	created_at: string;
	candidates: IdentificationCandidate[];
	/** Only present/non-empty when `status` is `'insufficient'`. */
	missing_info?: string[];
}

/** GET /api/sightings/{id} — the one-request aggregate view for the detail screen. */
export interface SightingDetail {
	sighting: Sighting;
	photos: Photo[];
	identification: IdentificationResult | null;
}

// --- Typed API helpers --------------------------------------------------------

/**
 * Absolute URL of the backend's OIDC login entry point. Used for the
 * "Sign in" / login redirect — a full-page navigation, not a fetch.
 */
export function loginUrl(): string {
	return `${API_BASE_URL}/auth/login`;
}

/**
 * GET /auth/me — the current user, or throws `ApiError` with status 401 when
 * not signed in. Callers treat 401 as "guest". Always succeeds as the
 * implicit `admin` user when the backend runs with FORAGEBUDDY_DEV_MODE=true.
 */
export function getMe(options?: RequestOptions): Promise<Me> {
	return request<Me>('GET', '/auth/me', undefined, options);
}

export interface CreateSightingInput {
	lat?: number;
	lon?: number;
	location_accuracy_m?: number;
	place_label?: string;
	observed_at: string;
	notes?: string;
}

/** POST /api/sightings — create a sighting (no photo yet; upload separately). */
export function createSighting(
	input: CreateSightingInput,
	options?: RequestOptions
): Promise<Sighting> {
	return request<Sighting>('POST', '/api/sightings', input, options);
}

/** GET /api/sightings — list the caller's sightings, newest first. */
export function listSightings(options?: RequestOptions): Promise<SightingListItem[]> {
	return request<SightingListItem[]>('GET', '/api/sightings', undefined, options);
}

/**
 * GET /api/sightings/{id} — the aggregate view (sighting + photos + latest
 * identification result) the detail screen renders from in one request.
 */
export function getSightingDetail(
	sightingId: string,
	options?: RequestOptions
): Promise<SightingDetail> {
	return request<SightingDetail>(
		'GET',
		`/api/sightings/${encodeURIComponent(sightingId)}`,
		undefined,
		options
	);
}

export interface PatchSightingInput {
	notes?: string;
	status?: SightingStatus;
}

/** PATCH /api/sightings/{id} — update mutable sighting fields. */
export function patchSighting(
	sightingId: string,
	input: PatchSightingInput,
	options?: RequestOptions
): Promise<Sighting> {
	return request<Sighting>(
		'PATCH',
		`/api/sightings/${encodeURIComponent(sightingId)}`,
		input,
		options
	);
}

/**
 * POST /api/sightings/{id}/photos — multipart upload, field name `photo`.
 * `takenAt` is an optional RFC3339 string (defaults server-side to upload
 * time); `lat`/`lon` optionally override the sighting's own location if the
 * user moved between shots. Triggers identification server-side after
 * commit (fire-and-forget on the backend) — callers should refetch the
 * sighting detail after a short delay (or let the user retrigger) to see
 * the result.
 */
export async function uploadPhoto(
	sightingId: string,
	file: File | Blob,
	opts: { takenAt?: string; lat?: number; lon?: number } = {},
	options: RequestOptions = {}
): Promise<Photo> {
	const url = `${API_BASE_URL}/api/sightings/${encodeURIComponent(sightingId)}/photos`;
	const form = new FormData();
	form.append('photo', file, file instanceof File ? file.name : 'photo.jpg');
	if (opts.takenAt !== undefined) form.append('taken_at', opts.takenAt);
	if (opts.lat !== undefined) form.append('lat', String(opts.lat));
	if (opts.lon !== undefined) form.append('lon', String(opts.lon));

	let response: Response;
	try {
		response = await fetch(url, {
			method: 'POST',
			credentials: 'include',
			headers: {
				Accept: 'application/json',
				...authHeaders(),
				...options.headers
			},
			body: form,
			signal: timeoutSignal(options.signal, options.timeoutMs ?? UPLOAD_TIMEOUT_MS)
		});
	} catch (cause) {
		throw new ApiError(
			describeFetchFailure('POST', url, cause),
			0,
			cause instanceof Error ? cause.message : cause
		);
	}

	const contentType = response.headers.get('content-type') ?? '';
	const isJson = contentType.includes('application/json');
	const payload = isJson ? await response.json().catch(() => undefined) : undefined;

	if (!response.ok) {
		const message =
			(isJson && payload && typeof payload === 'object' && 'message' in payload
				? String((payload as Record<string, unknown>).message)
				: undefined) ?? `Request failed with status ${response.status}`;
		throw new ApiError(message, response.status, payload);
	}

	return payload as Photo;
}

/**
 * GET /api/photos/{id}/file — fetch a photo's raw bytes as a Blob.
 *
 * Authenticated (cookie + device-token bearer), so an `<img src>` pointed
 * straight at this path wouldn't carry the bearer in the app build. Used by
 * `$lib/app/photoUrl`'s `resolvePhotoSrc` in Tauri/bearer mode; the web/cookie
 * mode links directly instead (`photoFileUrl`).
 */
export async function fetchPhotoBlob(photoId: string, options: RequestOptions = {}): Promise<Blob> {
	const url = `${API_BASE_URL}/api/photos/${encodeURIComponent(photoId)}/file`;
	let response: Response;
	try {
		response = await fetch(url, {
			method: 'GET',
			credentials: 'include',
			headers: { ...authHeaders(), ...options.headers },
			signal: timeoutSignal(options.signal, options.timeoutMs ?? DEFAULT_TIMEOUT_MS)
		});
	} catch (cause) {
		throw new ApiError(
			describeFetchFailure('GET', url, cause),
			0,
			cause instanceof Error ? cause.message : cause
		);
	}
	if (!response.ok) {
		throw new ApiError(`Request failed with status ${response.status}`, response.status);
	}
	return response.blob();
}

/**
 * POST /api/sightings/{id}/identification — manual re-run/retry. Covers both
 * "re-run after adding a note" and "the automatic run failed, try again".
 * Can take tens of seconds (LLM call); callers should show a clear loading
 * state while this is in flight.
 */
export function runIdentification(
	sightingId: string,
	options?: RequestOptions
): Promise<IdentificationResult> {
	return request<IdentificationResult>(
		'POST',
		`/api/sightings/${encodeURIComponent(sightingId)}/identification`,
		{},
		{ timeoutMs: LONG_RUNNING_TIMEOUT_MS, ...options }
	);
}
