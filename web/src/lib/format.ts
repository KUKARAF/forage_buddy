// Small formatting helpers shared across the list/detail screens.

/** "3 hours ago", "2 days ago", etc. Falls back to a locale date for anything
 * more than ~a month old so the list doesn't show "3 months ago" forever. */
export function relativeTime(iso: string): string {
	const date = new Date(iso);
	if (Number.isNaN(date.getTime())) return iso;
	const diffMs = Date.now() - date.getTime();
	const diffSec = Math.round(diffMs / 1000);
	const abs = Math.abs(diffSec);

	if (abs < 45) return 'just now';
	if (abs < 90) return diffSec >= 0 ? 'a minute ago' : 'in a minute';

	const diffMin = Math.round(diffSec / 60);
	if (Math.abs(diffMin) < 45)
		return `${Math.abs(diffMin)} minute${Math.abs(diffMin) === 1 ? '' : 's'} ago`;

	const diffHour = Math.round(diffMin / 60);
	if (Math.abs(diffHour) < 22)
		return `${Math.abs(diffHour)} hour${Math.abs(diffHour) === 1 ? '' : 's'} ago`;

	const diffDay = Math.round(diffHour / 24);
	if (Math.abs(diffDay) < 26)
		return `${Math.abs(diffDay)} day${Math.abs(diffDay) === 1 ? '' : 's'} ago`;

	const diffMonth = Math.round(diffDay / 30);
	if (Math.abs(diffMonth) < 11)
		return `${Math.abs(diffMonth)} month${Math.abs(diffMonth) === 1 ? '' : 's'} ago`;

	return date.toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' });
}

/** "47.123, -122.456" (lat, lon rounded to 3 decimals) for sightings with no place_label. */
export function formatLatLon(lat: number, lon: number): string {
	return `${lat.toFixed(3)}, ${lon.toFixed(3)}`;
}

/** Best-effort place/location string for a list row: place_label if present,
 * else rounded lat/lon, else null (no location recorded). */
export function placeOrCoords(
	placeLabel: string | null,
	lat: number | null,
	lon: number | null
): string | null {
	if (placeLabel) return placeLabel;
	if (lat !== null && lon !== null) return formatLatLon(lat, lon);
	return null;
}

/** Convert a <input type="datetime-local"> value (local time, no timezone) to
 * an RFC3339 string with the local timezone offset, which is what the
 * backend's `observed_at`/`taken_at` fields expect. */
export function datetimeLocalToRfc3339(value: string): string {
	// new Date() on a "YYYY-MM-DDTHH:mm" string parses it as LOCAL time, and
	// .toISOString() always serializes in UTC — both are valid RFC3339, so no
	// manual offset math is needed.
	return new Date(value).toISOString();
}

/** A <input type="datetime-local"> value representing "now", for defaulting the form. */
export function nowAsDatetimeLocal(): string {
	const d = new Date();
	d.setSeconds(0, 0);
	const pad = (n: number) => String(n).padStart(2, '0');
	return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** Round a confidence (0..1) to a whole percentage for display. */
export function confidencePct(confidence: number): number {
	return Math.round(Math.max(0, Math.min(1, confidence)) * 100);
}
