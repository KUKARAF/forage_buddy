<script lang="ts">
	import { onDestroy } from 'svelte';
	import { page } from '$app/state';
	import {
		getSightingDetail,
		uploadPhoto,
		runTriage,
		triggerDeepDive,
		ApiError,
		type SightingDetail
	} from '$lib/api/client';
	import { relativeTime, placeOrCoords } from '$lib/format';
	import SafetyBanner from '$lib/components/SafetyBanner.svelte';
	import DangerBadge from '$lib/components/DangerBadge.svelte';
	import ConfidenceBar from '$lib/components/ConfidenceBar.svelte';
	import PhotoImg from '$lib/components/PhotoImg.svelte';
	import CameraCapture from '$lib/components/CameraCapture.svelte';
	import { IS_APP } from '$lib/api/deviceToken';

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

	let rerunningTriage = $state(false);
	let triageError = $state<string | null>(null);

	let deepDiveLoading = $state(false);
	let deepDiveError = $state<string | null>(null);

	let lightboxPhotoId = $state<string | null>(null);

	// Local-only checklist state for the deep-dive confusant features: ticking
	// a box helps the user work through the list while re-examining their
	// specimen, but is never sent to the backend.
	let checklist = $state<Record<string, boolean>>({});

	function checklistKey(confusantIdx: number, featureIdx: number): string {
		return `${confusantIdx}:${featureIdx}`;
	}

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

	// Triage, and then deep dive once triage lands on a real candidate, both
	// run automatically server-side after a photo upload (see
	// photos::on_photo_uploaded) — the look-alike/danger checklist is the
	// entire safety point of this app, not an optional extra behind a
	// button. Poll silently in the background (no full-page "Loading…"
	// flash) while either is still expected, so results just appear.
	let pollTimer: ReturnType<typeof setInterval> | undefined;
	let polling = $state(false);
	const POLL_INTERVAL_MS = 3000;
	const POLL_GIVE_UP_MS = 150_000; // a bit above the backend's own 120s request budget

	function awaitingAutoResults(d: SightingDetail | null): boolean {
		if (!d) return false;
		if (!d.triage) return true;
		return d.triage.status !== 'insufficient' && !d.deepdive;
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
		if (awaitingAutoResults(detail)) {
			polling = true;
			if (!pollTimer) {
				const startedAt = Date.now();
				pollTimer = setInterval(() => {
					if (Date.now() - startedAt > POLL_GIVE_UP_MS) {
						clearInterval(pollTimer);
						pollTimer = undefined;
						polling = false;
						return;
					}
					void refreshSilently();
				}, POLL_INTERVAL_MS);
			}
		} else if (pollTimer) {
			clearInterval(pollTimer);
			pollTimer = undefined;
			polling = false;
		}
	});

	onDestroy(() => {
		if (pollTimer) clearInterval(pollTimer);
	});

	$effect(() => {
		void load();
	});

	/** Uploads every file sequentially (one `uploadPhoto` call after another —
	 * each upload triggers a full triage LLM call server-side, so sequential
	 * keeps load predictable), then refreshes the detail view exactly once at
	 * the end rather than once per file. */
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

	async function rerunTriage() {
		if (!detail) return;
		rerunningTriage = true;
		triageError = null;
		try {
			detail.triage = await runTriage(detail.sighting.id);
		} catch (err) {
			triageError = err instanceof ApiError ? err.message : 'Could not re-run triage.';
		} finally {
			rerunningTriage = false;
		}
	}

	async function runDeepDive() {
		if (!detail) return;
		deepDiveLoading = true;
		deepDiveError = null;
		try {
			detail.deepdive = await triggerDeepDive(detail.sighting.id);
			checklist = {};
		} catch (err) {
			deepDiveError = err instanceof ApiError ? err.message : 'Deep dive failed. Please try again.';
		} finally {
			deepDiveLoading = false;
		}
	}

	const canRunDeepDive = $derived(
		detail?.triage !== null &&
			detail?.triage !== undefined &&
			detail.triage.status !== 'insufficient'
	);
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
		<SafetyBanner />

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

		<section class="card">
			<div class="card-head">
				<h2>Triage</h2>
				<button class="btn secondary small-btn" disabled={rerunningTriage} onclick={rerunTriage}>
					{rerunningTriage ? 'Re-running…' : 'Re-run triage'}
				</button>
			</div>

			{#if !detail.triage}
				<p class="muted">
					No triage result yet — it runs automatically right after a photo upload. Try "Re-run
					triage" if it's been a moment.
				</p>
			{:else if detail.triage.status === 'insufficient'}
				<div class="missing-info">
					<p class="missing-head">📸 We need a bit more to go on:</p>
					<ul>
						{#each detail.triage.missing_info as ask (ask)}
							<li>{ask}</li>
						{/each}
					</ul>
				</div>
				<p class="muted small reasoning">{detail.triage.reasoning}</p>
			{:else}
				<ul class="candidates">
					{#each detail.triage.candidate_species as candidate (candidate.species)}
						<li>
							<div class="candidate-name">
								<strong>{candidate.species}</strong>
								{#if candidate.common_name}<span class="muted">({candidate.common_name})</span>{/if}
							</div>
							<ConfidenceBar confidence={candidate.confidence} />
						</li>
					{/each}
				</ul>
				<p class="muted small reasoning">{detail.triage.reasoning}</p>
			{/if}

			{#if triageError}
				<p class="error-text">{triageError}</p>
			{/if}

			<div class="deepdive-trigger">
				{#if !canRunDeepDive}
					<button
						class="btn"
						disabled
						title="Needs at least a genus candidate before a deep dive makes sense"
					>
						Run deep dive
					</button>
				{:else if polling && !detail.deepdive && !deepDiveLoading}
					<p class="muted small">🔎 Checking Wikipedia and known look-alikes automatically…</p>
					<button class="btn secondary small-btn" onclick={runDeepDive}>Run now instead</button>
				{:else}
					<button class="btn" disabled={deepDiveLoading} onclick={runDeepDive}>
						{deepDiveLoading
							? 'Running deep dive…'
							: detail.deepdive
								? 'Re-run deep dive'
								: 'Run deep dive'}
					</button>
				{/if}
			</div>
		</section>

		{#if deepDiveLoading}
			<section class="card deepdive-loading">
				<div class="spinner" aria-hidden="true"></div>
				<p>Checking Wikipedia and known look-alikes… this can take a little while.</p>
			</section>
		{:else if deepDiveError}
			<section class="card error">
				<p>{deepDiveError}</p>
			</section>
		{:else if detail.deepdive}
			{@const dd = detail.deepdive}
			<section class="card deepdive">
				<h2>Deep dive</h2>
				<SafetyBanner />

				<div class="best-match">
					<strong>{dd.best_match_species}</strong>
					<ConfidenceBar confidence={dd.confidence} />
				</div>

				{#if dd.wikipedia_extract}
					<div class="wiki">
						<p>{dd.wikipedia_extract}</p>
						{#if dd.wikipedia_url}
							<a href={dd.wikipedia_url} target="_blank" rel="noopener noreferrer external"
								>Read more on Wikipedia →</a
							>
						{/if}
					</div>
				{/if}

				{#if dd.confusants.length > 0}
					<h3>⚠️ Could be confused with</h3>
					<ul class="confusants">
						{#each dd.confusants as confusant, ci (confusant.species)}
							<li
								class="confusant level-{confusant.danger_level}"
								class:alarming={confusant.danger_level === 'deadly_toxic'}
							>
								<div class="confusant-head">
									<div>
										<strong>{confusant.species}</strong>
										{#if confusant.common_name}<span class="muted">
												({confusant.common_name})</span
											>{/if}
									</div>
									<DangerBadge level={confusant.danger_level} />
								</div>
								{#if confusant.notes}
									<p class="confusant-notes">{confusant.notes}</p>
								{/if}
								{#if confusant.distinguishing_features.length > 0}
									<p class="checklist-label muted small">Check to rule out:</p>
									<ul class="checklist">
										{#each confusant.distinguishing_features as feature, fi (feature)}
											<li>
												<label>
													<input
														type="checkbox"
														checked={checklist[checklistKey(ci, fi)] ?? false}
														onchange={(e) =>
															(checklist[checklistKey(ci, fi)] = (
																e.currentTarget as HTMLInputElement
															).checked)}
													/>
													{feature}
												</label>
											</li>
										{/each}
									</ul>
								{/if}
							</li>
						{/each}
					</ul>
				{/if}

				<p class="safety-notes muted small">{dd.safety_notes}</p>
			</section>
		{/if}

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
	.reasoning {
		margin-top: 10px;
	}

	.candidates {
		list-style: none;
		margin: 0 0 10px;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 12px;
	}
	.candidate-name {
		margin-bottom: 4px;
	}

	.deepdive-trigger {
		margin-top: 14px;
	}

	.deepdive-loading {
		display: flex;
		align-items: center;
		gap: 14px;
	}
	.spinner {
		width: 28px;
		height: 28px;
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

	.deepdive h3 {
		margin-top: 18px;
	}
	.best-match {
		margin-bottom: 14px;
	}
	.wiki {
		margin-bottom: 14px;
	}
	.wiki a {
		color: var(--accent);
		font-weight: 600;
	}

	.confusants {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 10px;
	}
	.confusant {
		border-radius: var(--radius-btn);
		border: 1px solid var(--line);
		border-left: 6px solid var(--grey);
		padding: 12px 14px;
		background: var(--card);
	}
	.confusant.level-unknown {
		border-left-color: var(--grey);
	}
	.confusant.level-safe {
		border-left-color: var(--green);
	}
	.confusant.level-caution {
		border-left-color: var(--amber);
	}
	.confusant.level-toxic {
		border-left-color: var(--orange);
	}
	.confusant.level-deadly_toxic {
		border-left-color: var(--red);
	}
	.confusant.alarming {
		background: var(--red-bg);
		box-shadow: 0 0 0 1px var(--red) inset;
	}
	.confusant-head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 10px;
	}
	.confusant-notes {
		margin: 8px 0 4px;
	}
	.checklist-label {
		margin: 8px 0 4px;
	}
	.checklist {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 6px;
	}
	.checklist label {
		display: flex;
		align-items: flex-start;
		gap: 8px;
		font-weight: 400;
	}
	.checklist input {
		margin-top: 3px;
		width: 18px;
		height: 18px;
		flex: none;
	}

	.safety-notes {
		margin-top: 16px;
		border-top: 1px solid var(--line);
		padding-top: 10px;
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
