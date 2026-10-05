<script lang="ts">
	import { onDestroy } from 'svelte';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { createSighting, uploadPhoto, ApiError } from '$lib/api/client';
	import { IS_APP } from '$lib/api/deviceToken';
	import { getCurrentLocation, GeolocationError, type GeoResult } from '$lib/app/geolocation';
	import { datetimeLocalToRfc3339, nowAsDatetimeLocal } from '$lib/format';
	import CameraCapture from '$lib/components/CameraCapture.svelte';

	interface PendingPhoto {
		id: string;
		file: File;
		url: string;
	}

	// Queued photos, to be uploaded one-by-one (sequentially, not in parallel —
	// each upload triggers a full triage LLM call server-side; sequential keeps
	// load predictable) right after the sighting itself is created.
	let pendingPhotos = $state<PendingPhoto[]>([]);
	let location = $state<GeoResult | null>(null);
	let locating = $state(false);
	let locationError = $state<string | null>(null);
	let observedAt = $state(nowAsDatetimeLocal());
	let notes = $state('');
	let submitting = $state(false);
	let submitError = $state<string | null>(null);

	let cameraOpen = $state(false);
	let cameraUnavailableMessage = $state<string | null>(null);
	let fallbackInput: HTMLInputElement | undefined = $state();
	let galleryInput: HTMLInputElement | undefined = $state();

	function addFiles(files: File[]) {
		for (const file of files) {
			pendingPhotos.push({ id: crypto.randomUUID(), file, url: URL.createObjectURL(file) });
		}
	}

	function removePendingPhoto(id: string) {
		const idx = pendingPhotos.findIndex((p) => p.id === id);
		if (idx === -1) return;
		URL.revokeObjectURL(pendingPhotos[idx].url);
		pendingPhotos.splice(idx, 1);
	}

	// In the Android app, `getUserMedia` never actually works (the Tauri
	// Android WebView doesn't bridge its permission prompt to a real Android
	// runtime grant — see docs/ARCHITECTURE.md / the camera-UX follow-up
	// notes), so trying it first and falling back to the file input only
	// AFTER an async rejection is worse than useless there: by the time the
	// rejection lands, the click that opened this is no longer a "trusted"
	// synchronous user gesture, and some WebViews silently refuse to honor a
	// programmatic `.click()` on a file input outside that window — which is
	// exactly what "the camera button does nothing" looks like. So in the
	// app build, skip the live-camera attempt entirely and go straight to
	// the proven-reliable `<input capture>` system-camera handoff, triggered
	// synchronously from the real tap. The web build still gets the nicer
	// in-app live camera, where getUserMedia works normally.
	function openCamera() {
		if (IS_APP) {
			fallbackInput?.click();
			return;
		}
		cameraUnavailableMessage = null;
		cameraOpen = true;
	}

	function onCameraDone(files: File[]) {
		cameraOpen = false;
		addFiles(files);
	}

	function onCameraCancel() {
		cameraOpen = false;
	}

	function onCameraUnavailable() {
		cameraOpen = false;
		cameraUnavailableMessage = 'Camera not available — choose or take a photo instead.';
		fallbackInput?.click();
	}

	function onFallbackChange(e: Event) {
		const input = e.currentTarget as HTMLInputElement;
		const files = input.files ? Array.from(input.files) : [];
		input.value = '';
		addFiles(files);
	}

	onDestroy(() => {
		for (const photo of pendingPhotos) URL.revokeObjectURL(photo.url);
	});

	async function useMyLocation() {
		locating = true;
		locationError = null;
		try {
			location = await getCurrentLocation();
		} catch (err) {
			locationError =
				err instanceof GeolocationError ? err.message : 'Could not get your location.';
		} finally {
			locating = false;
		}
	}

	function clearLocation() {
		location = null;
		locationError = null;
	}

	async function submit() {
		if (pendingPhotos.length === 0) {
			submitError = 'Add a photo to continue — triage needs at least one.';
			return;
		}
		submitting = true;
		submitError = null;
		try {
			const observedAtRfc3339 = datetimeLocalToRfc3339(observedAt);
			const sighting = await createSighting({
				lat: location?.lat,
				lon: location?.lon,
				location_accuracy_m: location?.accuracyM ?? undefined,
				observed_at: observedAtRfc3339,
				notes: notes.trim() ? notes.trim() : undefined
			});
			// Sequential on purpose — see the comment on `pendingPhotos` above.
			for (const photo of pendingPhotos) {
				await uploadPhoto(sighting.id, photo.file, { takenAt: observedAtRfc3339 });
			}
			await goto(resolve('/sightings/[id]', { id: sighting.id }));
		} catch (err) {
			submitError = err instanceof ApiError ? err.message : 'Could not create the sighting.';
			submitting = false;
		}
	}
</script>

<svelte:head>
	<title>New sighting — Forage Buddy</title>
</svelte:head>

<h1>New sighting</h1>
<p class="muted">
	Snap a clear photo, note where and when, and we'll give you a fast genus guess — or tell you what
	other photo would help.
</p>

<form
	onsubmit={(e) => {
		e.preventDefault();
		void submit();
	}}
>
	<div class="field">
		<span class="field-label">Photos</span>

		{#if pendingPhotos.length > 0}
			<div class="photo-grid">
				{#each pendingPhotos as photo (photo.id)}
					<div class="photo-thumb">
						<img src={photo.url} alt="Queued specimen" />
						<button
							type="button"
							class="thumb-remove"
							onclick={() => removePendingPhoto(photo.id)}
							aria-label="Remove this photo"
						>
							✕
						</button>
					</div>
				{/each}
			</div>
		{/if}

		<div class="capture-actions">
			<button type="button" class="btn" onclick={openCamera}>📷 Take a photo</button>
			<button type="button" class="btn secondary" onclick={() => galleryInput?.click()}>
				🖼️ Choose photo(s)
			</button>
		</div>

		<!-- Camera-capture input: the Android app's primary capture path
		     (triggered directly, synchronously, from openCamera above), and the
		     web build's automatic fallback when getUserMedia is unavailable/denied.
		     Deliberately NOT `multiple` — Chrome/Android silently drops the
		     `capture` hint (falling back to a generic file/gallery picker)
		     when `multiple` is also set on the same input, which is exactly
		     what broke "Take a photo" before. One shot per tap; tap again for
		     more, same as any camera app. -->
		<input
			bind:this={fallbackInput}
			class="visually-hidden"
			type="file"
			accept="image/*"
			capture="environment"
			onchange={onFallbackChange}
		/>
		<!-- Gallery/file picker, no camera hint — lets "Choose photo(s)" mean
		     what it says instead of also launching the camera. -->
		<input
			bind:this={galleryInput}
			class="visually-hidden"
			type="file"
			accept="image/*"
			multiple
			onchange={onFallbackChange}
		/>

		{#if cameraUnavailableMessage}
			<p class="muted small">{cameraUnavailableMessage}</p>
		{/if}
	</div>

	<div class="field">
		<label for="observed-at">When</label>
		<input id="observed-at" type="datetime-local" bind:value={observedAt} />
	</div>

	<div class="field">
		<span class="field-label">Where (optional)</span>
		{#if location}
			<div class="location-chip">
				<span>{location.lat.toFixed(5)}, {location.lon.toFixed(5)}</span>
				{#if location.accuracyM !== null}
					<span class="muted small">±{Math.round(location.accuracyM)} m</span>
				{/if}
				<button type="button" class="btn secondary small-btn" onclick={clearLocation}>Clear</button>
			</div>
		{:else}
			<button type="button" class="btn secondary" onclick={useMyLocation} disabled={locating}>
				{locating ? 'Getting location…' : '📍 Use my location'}
			</button>
			{#if locationError}
				<p class="error-text">{locationError}</p>
			{/if}
		{/if}
	</div>

	<div class="field">
		<label for="notes">Notes (optional)</label>
		<textarea
			id="notes"
			bind:value={notes}
			placeholder="Habitat, smell, texture, anything that might help…"></textarea>
	</div>

	{#if submitError}
		<p class="error-text">{submitError}</p>
	{/if}

	<button class="btn" type="submit" disabled={submitting}>
		{submitting ? 'Creating…' : 'Create sighting'}
	</button>
</form>

{#if cameraOpen}
	<CameraCapture
		ondone={onCameraDone}
		oncancel={onCameraCancel}
		onunavailable={onCameraUnavailable}
	/>
{/if}

<style>
	.field-label {
		display: block;
		font-weight: 600;
		font-size: 14px;
		margin-bottom: 6px;
	}

	.photo-grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(96px, 1fr));
		gap: 10px;
		margin-bottom: 12px;
	}
	.photo-thumb {
		position: relative;
		aspect-ratio: 1;
		border-radius: var(--radius-card);
		overflow: hidden;
		border: 1px solid var(--line);
		background: var(--tag-bg);
	}
	.photo-thumb img {
		width: 100%;
		height: 100%;
		object-fit: cover;
		display: block;
	}
	.thumb-remove {
		position: absolute;
		top: 6px;
		right: 6px;
		width: 26px;
		height: 26px;
		min-height: 0;
		border-radius: 999px;
		border: none;
		background: var(--red);
		color: var(--btn-ink);
		font-size: 13px;
		line-height: 1;
		display: flex;
		align-items: center;
		justify-content: center;
		padding: 0;
	}

	.capture-actions {
		display: flex;
		gap: 10px;
		flex-wrap: wrap;
	}

	.visually-hidden {
		position: absolute;
		width: 1px;
		height: 1px;
		overflow: hidden;
		clip: rect(0 0 0 0);
	}

	.location-chip {
		display: flex;
		align-items: center;
		gap: 10px;
		background: var(--tag-bg);
		border: 1px solid var(--line);
		border-radius: var(--radius-btn);
		padding: 10px 12px;
		flex-wrap: wrap;
	}
	.small-btn {
		min-height: 36px;
		padding: 6px 12px;
	}
	.small {
		font-size: 12.5px;
	}

	.error-text {
		color: var(--red);
		font-size: 13.5px;
		margin: 6px 0;
	}
</style>
