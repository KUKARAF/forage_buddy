<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { createSighting, uploadPhoto, ApiError } from '$lib/api/client';
	import { getCurrentLocation, GeolocationError, type GeoResult } from '$lib/app/geolocation';
	import { datetimeLocalToRfc3339, nowAsDatetimeLocal } from '$lib/format';

	let photoFile = $state<File | null>(null);
	let photoPreviewUrl = $state<string | null>(null);
	let location = $state<GeoResult | null>(null);
	let locating = $state(false);
	let locationError = $state<string | null>(null);
	let observedAt = $state(nowAsDatetimeLocal());
	let notes = $state('');
	let submitting = $state(false);
	let submitError = $state<string | null>(null);

	let fileInput: HTMLInputElement | undefined = $state();

	function onPhotoChange(e: Event) {
		const input = e.currentTarget as HTMLInputElement;
		const file = input.files?.[0] ?? null;
		if (photoPreviewUrl) URL.revokeObjectURL(photoPreviewUrl);
		photoFile = file;
		photoPreviewUrl = file ? URL.createObjectURL(file) : null;
	}

	function clearPhoto() {
		if (photoPreviewUrl) URL.revokeObjectURL(photoPreviewUrl);
		photoFile = null;
		photoPreviewUrl = null;
		if (fileInput) fileInput.value = '';
	}

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
		if (!photoFile) {
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
			await uploadPhoto(sighting.id, photoFile, { takenAt: observedAtRfc3339 });
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
		<label for="photo">Photo</label>
		{#if photoPreviewUrl}
			<div class="preview">
				<img src={photoPreviewUrl} alt="Selected specimen" />
				<button type="button" class="btn secondary" onclick={clearPhoto}>Remove photo</button>
			</div>
		{:else}
			<input
				bind:this={fileInput}
				id="photo"
				type="file"
				accept="image/*"
				capture="environment"
				onchange={onPhotoChange}
			/>
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

<style>
	.field-label {
		display: block;
		font-weight: 600;
		font-size: 14px;
		margin-bottom: 6px;
	}

	.preview {
		display: flex;
		flex-direction: column;
		gap: 10px;
		align-items: flex-start;
	}
	.preview img {
		max-width: 100%;
		max-height: 320px;
		border-radius: var(--radius-card);
		border: 1px solid var(--line);
		object-fit: contain;
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
