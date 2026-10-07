<script lang="ts">
	import { onDestroy } from 'svelte';
	import { page } from '$app/state';
	import {
		getSightingDetail,
		uploadPhoto,
		runIdentification,
		ApiError,
		type SightingDetail
	} from '$lib/api/client';
	import { relativeTime, placeOrCoords } from '$lib/format';
	import IdentificationCard from '$lib/components/IdentificationCard.svelte';
	import SafetyAckModal from '$lib/components/SafetyAckModal.svelte';
	import PoisonousWarningModal from '$lib/components/PoisonousWarningModal.svelte';
	import PhotoImg from '$lib/components/PhotoImg.svelte';
	import CameraCapture from '$lib/components/CameraCapture.svelte';
	import { IS_APP } from '$lib/api/deviceToken';
	import { hasAcknowledgedSafety, acknowledgeSafety } from '$lib/identificationAck';

	const sightingId = $derived(page.params.id as string);

	let detail = $state<SightingDetail | null>(null);
	let loading = $state(true);
	let loadError = $state<string | null>(null);
	// Separate from loadError: if `getSightingDetail` succeeds but something
	// in rendering the result throws (an unexpected data shape, a bug in a
	// child component, ...), <svelte:boundary> below catches it here instead
	// of silently leaving the page frozen on its last-rendered state (which
	// is exactly what an unguarded render-time exception looks like — the
	// "Loading sighting…" text can be left on screen forever since `loading`
	// already flipped false but the branch swap that would remove it never
	// completed). This turns an invisible freeze into a visible, reportable
	// error with the actual message.
	let renderError = $state<string | null>(null);

	let galleryInput: HTMLInputElement | undefined = $state();
	let uploadingPhoto = $state(false);
	let uploadError = $state<string | null>(null);

	let cameraOpen = $state(false);
	let cameraUnavailableMessage = $state<string | null>(null);

	let reidentifying = $state(false);
	let identificationError = $state<string | null>(null);

	let lightboxPhotoId = $state<string | null>(null);

	// Per-session "I understand the AI disclaimer" gate (SafetyAckModal) —
	// frontend-only UX, no backend enforcement. Initialized from
	// sessionStorage so it doesn't re-prompt on every sighting viewed in the
	// same session, only the first time.
	let safetyAcknowledged = $state(hasAcknowledgedSafety());

	// Separate, additional warning (PoisonousWarningModal) — fires once per
	// sighting view the first time a complete result includes a poisonous
	// candidate, never again on later poll ticks/re-renders for this same
	// view.
	let poisonousWarningSeen = $state(false);
	let showPoisonousWarning = $state(false);

	async function load() {
		loading = true;
		loadError = null;
		try {
			detail = await getSightingDetail(sightingId);
		} catch (err) {
			loadError = err instanceof ApiError ? err.message : 'Could not load this sighting.';
		} finally {
			loading = false;
		}
	}

	// Identification runs automatically server-side after a photo upload (see
	// photos::on_photo_uploaded) — it is the entire safety point of this app,
	// not an optional extra behind a button. Poll silently in the background
	// (no full-page "Loading…" flash) while it's still in progress, so the
	// result just appears.
	let pollTimer: ReturnType<typeof setInterval> | undefined;
	const POLL_INTERVAL_MS = 3000;
	const POLL_GIVE_UP_MS = 150_000; // a bit above the backend's own 120s request budget

	function identificationInProgress(d: SightingDetail | null): boolean {
		if (!d) return false;
		const status = d.identification?.status;
		return status === undefined || status === 'pending' || status === 'partial';
	}

	async function refreshSilently() {
		try {
			detail = await getSightingDetail(sightingId);
		} catch {
			// Best-effort background refresh; the "Try again" button on a real
			// load failure (loadError) is the path for persistent errors, this
			// is just a missed poll tick.
		}
	}

	$effect(() => {
		if (identificationInProgress(detail)) {
			if (!pollTimer) {
				const startedAt = Date.now();
				pollTimer = setInterval(() => {
					if (Date.now() - startedAt > POLL_GIVE_UP_MS) {
						clearInterval(pollTimer);
						pollTimer = undefined;
						return;
					}
					void refreshSilently();
				}, POLL_INTERVAL_MS);
			}
		} else if (pollTimer) {
			clearInterval(pollTimer);
			pollTimer = undefined;
		}
	});

	// Fire the poisonous warning at most once per sighting view: as soon as
	// the identification result first lands in a `complete` state, decide
	// then and there whether to show it, and never re-evaluate again for
	// this view (even though `detail` keeps getting reassigned by polling/
	// re-identify).
	$effect(() => {
		const ident = detail?.identification;
		if (ident?.status === 'complete' && !poisonousWarningSeen) {
			poisonousWarningSeen = true;
			if (ident.candidates.some((c) => c.poisonous === true)) {
				showPoisonousWarning = true;
			}
		}
	});

	onDestroy(() => {
		if (pollTimer) clearInterval(pollTimer);
	});

	$effect(() => {
		void load();
	});

	/** Uploads every file sequentially (one `uploadPhoto` call after another —
	 * each upload triggers a full identification LLM call server-side, so
	 * sequential keeps load predictable), then refreshes the detail view
	 * exactly once at the end rather than once per file. */
	async function uploadFiles(files: File[]) {
		if (files.length === 0 || !detail) return;
		uploadingPhoto = true;
		uploadError = null;
		try {
			for (const file of files) {
				await uploadPhoto(detail.sighting.id, file);
			}
			await load();
		} catch (err) {
			uploadError = err instanceof ApiError ? err.message : 'Could not upload that photo.';
		} finally {
			uploadingPhoto = false;
		}
	}

	async function onAddPhoto(e: Event) {
		const input = e.currentTarget as HTMLInputElement;
		const files = input.files ? Array.from(input.files) : [];
		input.value = '';
		await uploadFiles(files);
	}

	// See the matching comment in sightings/new/+page.svelte: there is no
	// reliable in-app camera on Android today (the WebView's file chooser
	// doesn't honor <input capture>, and getUserMedia can't get a real
	// permission grant either), so the app build only offers the gallery
	// picker — no button that promises a camera it can't deliver.
	function openCamera() {
		cameraUnavailableMessage = null;
		cameraOpen = true;
	}

	async function onCameraDone(files: File[]) {
		cameraOpen = false;
		await uploadFiles(files);
	}

	function onCameraCancel() {
		cameraOpen = false;
	}

	function onCameraUnavailable() {
		cameraOpen = false;
		cameraUnavailableMessage = 'Camera not available — choose a photo instead.';
		galleryInput?.click();
	}

	async function reidentify() {
		if (!detail) return;
		reidentifying = true;
		identificationError = null;
		try {
			detail.identification = await runIdentification(detail.sighting.id);
			// A fresh result deserves a fresh evaluation of the poisonous alert.
			poisonousWarningSeen = false;
		} catch (err) {
			identificationError =
				err instanceof ApiError ? err.message : 'Could not re-run identification.';
		} finally {
			reidentifying = false;
		}
	}

	const canReidentify = $derived(
		detail?.identification?.status === 'complete' ||
			detail?.identification?.status === 'insufficient'
	);

	// Gate seeing the actual result (candidates, or the missing-info list)
	// behind the per-session SafetyAckModal — never gates viewing the
	// sighting/photos themselves.
	const needsSafetyAck = $derived(!safetyAcknowledged && canReidentify);
</script>

<svelte:head>
	<title>Sighting — Forage Buddy</title>
</svelte:head>

{#if loading}
	<p class="muted">Loading sighting…</p>
{:else if loadError}
	<div class="card error">
		<p>{loadError}</p>
		<button class="btn secondary" onclick={load}>Try again</button>
	</div>
{:else if detail}
	<svelte:boundary
		onerror={(err) => {
			renderError = err instanceof Error ? err.message : String(err);
			console.error('Error rendering sighting detail:', err);
		}}
	>
		{#snippet failed(error, reset)}
			<div class="card error">
				<p>Something went wrong showing this sighting: {renderError ?? String(error)}</p>
				<button
					class="btn secondary"
					onclick={() => {
						renderError = null;
						reset();
					}}>Try again</button
				>
			</div>
		{/snippet}

		<section class="card">
			<h2>Photos</h2>
			<div class="gallery">
				{#each detail.photos as photo (photo.id)}
					<button class="thumb-btn" onclick={() => (lightboxPhotoId = photo.id)}>
						<PhotoImg photoId={photo.id} alt="Specimen photo" class="thumb-img" />
					</button>
				{/each}
			</div>
			<!-- Gallery/file picker. Also the web build's fallback when
			     getUserMedia is unavailable/denied. -->
			<input
				bind:this={galleryInput}
				type="file"
				accept="image/*"
				multiple
				class="visually-hidden"
				onchange={onAddPhoto}
			/>
			<div class="capture-actions">
				{#if !IS_APP}
					<button class="btn" disabled={uploadingPhoto} onclick={openCamera}>
						📷 Take a photo
					</button>
					<button
						class="btn secondary"
						disabled={uploadingPhoto}
						onclick={() => galleryInput?.click()}
					>
						{uploadingPhoto ? 'Uploading…' : '🖼️ Choose photo(s)'}
					</button>
				{:else}
					<!-- No in-app camera on Android today — one honest action: take the
					     photo with your camera app, then pick it here. -->
					<button class="btn" disabled={uploadingPhoto} onclick={() => galleryInput?.click()}>
						{uploadingPhoto ? 'Uploading…' : '📷 Add photo(s)'}
					</button>
				{/if}
			</div>
			{#if cameraUnavailableMessage}
				<p class="muted small">{cameraUnavailableMessage}</p>
			{/if}
			{#if uploadError}
				<p class="error-text">{uploadError}</p>
			{/if}
		</section>

		<div class="header-row">
			<div>
				<h1>Sighting</h1>
				<p class="muted">
					{relativeTime(detail.sighting.observed_at)}
					{#if placeOrCoords(detail.sighting.place_label, detail.sighting.lat, detail.sighting.lon)}
						· {placeOrCoords(detail.sighting.place_label, detail.sighting.lat, detail.sighting.lon)}
					{/if}
				</p>
			</div>
		</div>

		{#if detail.sighting.notes}
			<p class="notes">{detail.sighting.notes}</p>
		{/if}

		<section class="card identification">
			<div class="card-head">
				<h2>Identification</h2>
				{#if canReidentify}
					<button class="btn secondary small-btn" disabled={reidentifying} onclick={reidentify}>
						{reidentifying ? 'Re-identifying…' : 'Re-identify'}
					</button>
				{/if}
			</div>

			{#if identificationInProgress(detail)}
				<div class="identifying-row">
					<span class="spinner" aria-hidden="true"></span>
					<p class="muted">🔎 Identifying…</p>
				</div>
			{:else if detail.identification}
				{@const ident = detail.identification}
				{#if ident.status === 'failed'}
					<p class="error-text">Identification failed.</p>
					<button class="btn secondary small-btn" disabled={reidentifying} onclick={reidentify}>
						{reidentifying ? 'Trying again…' : 'Try again'}
					</button>
				{:else if ident.status === 'insufficient'}
					{#if needsSafetyAck}
						<p class="muted">Confirm the safety note to see what's missing.</p>
					{:else}
						<div class="missing-info">
							<p class="missing-head">📸 We need a bit more to go on:</p>
							<ul>
								{#each ident.missing_info ?? [] as ask (ask)}
									<li>{ask}</li>
								{/each}
							</ul>
						</div>
					{/if}
				{:else if ident.status === 'complete'}
					{#if needsSafetyAck}
						<p class="muted">Confirm the safety note to view your results.</p>
					{:else}
						{#each ident.candidates.slice(0, 3) as candidate (candidate.species)}
							<IdentificationCard {candidate} />
						{/each}
					{/if}
				{/if}
			{/if}

			{#if identificationError}
				<p class="error-text">{identificationError}</p>
			{/if}
		</section>

		{#if lightboxPhotoId}
			<div
				class="lightbox"
				role="button"
				tabindex="0"
				aria-label="Close full-size photo"
				onclick={() => (lightboxPhotoId = null)}
				onkeydown={(e) => e.key === 'Escape' && (lightboxPhotoId = null)}
			>
				<PhotoImg photoId={lightboxPhotoId} alt="Full-size specimen photo" class="lightbox-img" />
			</div>
		{/if}

		{#if cameraOpen}
			<CameraCapture
				ondone={onCameraDone}
				oncancel={onCameraCancel}
				onunavailable={onCameraUnavailable}
			/>
		{/if}

		{#if needsSafetyAck}
			<SafetyAckModal
				onacknowledge={() => {
					acknowledgeSafety();
					safetyAcknowledged = true;
				}}
			/>
		{/if}

		{#if showPoisonousWarning}
			<PoisonousWarningModal ondismiss={() => (showPoisonousWarning = false)} />
		{/if}
	</svelte:boundary>
{/if}

<style>
	.header-row {
		margin-bottom: 8px;
	}
	.notes {
		background: var(--tag-bg);
		border-radius: var(--radius-btn);
		padding: 10px 12px;
		margin-bottom: 16px;
	}

	.card {
		margin-bottom: 18px;
	}
	.card-head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 10px;
	}
	.card.error {
		border-color: var(--red);
		background: var(--red-bg);
	}

	.gallery {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(84px, 1fr));
		gap: 8px;
		margin-bottom: 14px;
	}
	.thumb-btn {
		border: none;
		background: var(--tag-bg);
		border-radius: 10px;
		overflow: hidden;
		aspect-ratio: 1;
		padding: 0;
	}
	.thumb-btn :global(.thumb-img) {
		width: 100%;
		height: 100%;
		object-fit: cover;
	}

	.visually-hidden {
		position: absolute;
		width: 1px;
		height: 1px;
		overflow: hidden;
		clip: rect(0 0 0 0);
	}

	.capture-actions {
		display: flex;
		gap: 10px;
		flex-wrap: wrap;
	}

	.small-btn {
		min-height: 36px;
		padding: 6px 12px;
		font-size: 13px;
	}
	.small {
		font-size: 12.5px;
	}

	.missing-info {
		background: var(--grey-bg);
		border-radius: var(--radius-btn);
		padding: 12px 14px;
	}
	.missing-head {
		font-weight: 700;
		margin-bottom: 6px;
	}
	.missing-info ul {
		margin: 0;
		padding-left: 20px;
	}

	.identifying-row {
		display: flex;
		align-items: center;
		gap: 12px;
	}
	.spinner {
		width: 22px;
		height: 22px;
		flex: none;
		border-radius: 999px;
		border: 3px solid var(--line);
		border-top-color: var(--moss);
		animation: spin 0.8s linear infinite;
	}
	@keyframes spin {
		to {
			transform: rotate(360deg);
		}
	}

	.error-text {
		color: var(--red);
		font-size: 13.5px;
		margin: 6px 0;
	}

	.lightbox {
		position: fixed;
		inset: 0;
		background: var(--overlay);
		display: flex;
		align-items: center;
		justify-content: center;
		padding: 24px;
		z-index: 50;
	}
	.lightbox :global(.lightbox-img) {
		max-width: 100%;
		max-height: 100%;
		object-fit: contain;
		border-radius: 8px;
	}
</style>
