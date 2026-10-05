<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { listSightings, ApiError, type SightingListItem } from '$lib/api/client';
	import { relativeTime, placeOrCoords } from '$lib/format';
	import SafetyBanner from '$lib/components/SafetyBanner.svelte';
	import StatusBadge from '$lib/components/StatusBadge.svelte';
	import PhotoImg from '$lib/components/PhotoImg.svelte';

	let sightings = $state<SightingListItem[]>([]);
	let loading = $state(true);
	let error = $state<string | null>(null);

	async function load() {
		loading = true;
		error = null;
		try {
			sightings = await listSightings();
		} catch (err) {
			error = err instanceof ApiError ? err.message : 'Could not load your sightings.';
		} finally {
			loading = false;
		}
	}

	onMount(load);
</script>

<svelte:head>
	<title>Forage Buddy</title>
</svelte:head>

<SafetyBanner />

<div class="header-row">
	<h1>Your sightings</h1>
</div>

{#if loading}
	<p class="muted">Loading your sightings…</p>
{:else if error}
	<div class="card error">
		<p>{error}</p>
		<button class="btn secondary" onclick={load}>Try again</button>
	</div>
{:else if sightings.length === 0}
	<div class="card empty">
		<p>No sightings yet. Photograph something you've found to get started.</p>
	</div>
{:else}
	<ul class="list">
		{#each sightings as item (item.id)}
			<li>
				<a class="row" href={resolve('/sightings/[id]', { id: item.id })}>
					<div class="thumb">
						{#if item.thumbnail_photo_id}
							<PhotoImg photoId={item.thumbnail_photo_id} alt="" class="thumb-img" />
						{:else}
							<div class="thumb-placeholder" aria-hidden="true">🍄</div>
						{/if}
					</div>
					<div class="meta">
						<div class="top-line">
							<span class="time">{relativeTime(item.created_at)}</span>
							{#if placeOrCoords(item.place_label, item.lat, item.lon)}
								<span class="place muted"
									>· {placeOrCoords(item.place_label, item.lat, item.lon)}</span
								>
							{/if}
						</div>
						<div class="bottom-line">
							<StatusBadge {item} />
							<span class="muted small"
								>{item.photo_count} photo{item.photo_count === 1 ? '' : 's'}</span
							>
						</div>
					</div>
				</a>
			</li>
		{/each}
	</ul>
{/if}

<button class="btn fab" onclick={() => goto(resolve('/sightings/new'))} aria-label="New sighting">
	<span aria-hidden="true">+</span> New sighting
</button>

<style>
	.header-row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		margin-bottom: 12px;
	}

	.card.error {
		border-color: var(--red);
		background: var(--red-bg);
	}

	.card.empty {
		text-align: center;
		color: var(--muted);
	}

	.list {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 10px;
	}

	.row {
		display: flex;
		align-items: center;
		gap: 14px;
		background: var(--card);
		border: 1px solid var(--line);
		border-radius: var(--radius-card);
		padding: 12px;
		box-shadow: var(--shadow);
	}
	.row:hover {
		border-color: var(--moss);
	}

	.thumb {
		flex: none;
		width: 64px;
		height: 64px;
		border-radius: 10px;
		overflow: hidden;
	}
	.thumb :global(.thumb-img) {
		width: 100%;
		height: 100%;
		object-fit: cover;
	}
	.thumb-placeholder {
		width: 100%;
		height: 100%;
		display: flex;
		align-items: center;
		justify-content: center;
		font-size: 28px;
		background: var(--tag-bg);
	}

	.meta {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: 6px;
	}
	.top-line {
		font-size: 14px;
		font-weight: 600;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.place {
		font-weight: 400;
	}
	.bottom-line {
		display: flex;
		align-items: center;
		gap: 10px;
	}
	.small {
		font-size: 12.5px;
	}

	.fab {
		position: fixed;
		bottom: calc(24px + var(--safe-bottom));
		right: calc(24px + var(--safe-right));
		border-radius: 999px;
		padding: 14px 20px;
		box-shadow: var(--shadow-strong);
		font-size: 15px;
	}
</style>
