<script lang="ts">
	// The list screen's per-row status badge. Priority, per ARCHITECTURE.md:
	// if a deep-dive exists, its highest-danger confusant's color is the
	// safety-forward signal (even if triage itself says "species_candidate");
	// otherwise fall back to the plain triage-status badge:
	//   insufficient      -> grey  "Needs more photos"
	//   genus_candidate   -> amber "Genus: {genus}"
	//   species_candidate -> green "Species: {name}"
	import type { SightingListItem } from '$lib/api/client';
	import DangerBadge from './DangerBadge.svelte';

	let { item }: { item: SightingListItem } = $props();

	const dangerLevel = $derived(item.latest_deepdive_danger_level ?? null);
</script>

{#if dangerLevel}
	<DangerBadge
		level={dangerLevel}
		label={item.latest_deepdive_best_match_species
			? `Look-alike risk: ${item.latest_deepdive_best_match_species}`
			: undefined}
	/>
{:else if item.latest_triage_status === 'insufficient'}
	<span class="status-badge grey">Needs more photos</span>
{:else if item.latest_triage_status === 'genus_candidate'}
	<span class="status-badge amber">Genus: {item.latest_triage_genus ?? '?'}</span>
{:else if item.latest_triage_status === 'species_candidate'}
	<span class="status-badge green"
		>Species: {item.latest_triage_species ?? item.latest_triage_genus ?? '?'}</span
	>
{:else}
	<span class="status-badge grey">No triage yet</span>
{/if}

<style>
	.status-badge {
		display: inline-flex;
		align-items: center;
		font-weight: 700;
		font-size: 12.5px;
		padding: 4px 10px;
		border-radius: 999px;
		white-space: nowrap;
	}
	.grey {
		background: var(--grey-bg);
		color: var(--grey);
	}
	.amber {
		background: var(--amber-bg);
		color: var(--amber);
	}
	.green {
		background: var(--green-bg);
		color: var(--green);
	}
</style>
