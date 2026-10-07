<script lang="ts">
	// The list screen's per-row status badge. Priority (best guess — see the
	// `SightingListItem` doc comment in $lib/api/client for the "not
	// confirmed against the backend rewrite" caveat on these field names):
	// if the latest identification result has a known danger level, that
	// color is the safety-forward signal; otherwise fall back to a plain
	// status badge for pending/partial/insufficient/complete/failed.
	import type { SightingListItem } from '$lib/api/client';
	import DangerBadge from './DangerBadge.svelte';

	let { item }: { item: SightingListItem } = $props();

	const dangerLevel = $derived(item.latest_identification_danger_level ?? null);
</script>

{#if dangerLevel}
	<DangerBadge
		level={dangerLevel}
		label={item.latest_identification_species
			? `Look-alike risk: ${item.latest_identification_species}`
			: undefined}
	/>
{:else if item.latest_identification_status === 'pending' || item.latest_identification_status === 'partial'}
	<span class="status-badge grey">Identifying…</span>
{:else if item.latest_identification_status === 'insufficient'}
	<span class="status-badge grey">Needs more photos</span>
{:else if item.latest_identification_status === 'complete'}
	<span class="status-badge green">Species: {item.latest_identification_species ?? '?'}</span>
{:else if item.latest_identification_status === 'failed'}
	<span class="status-badge amber">Identification failed</span>
{:else}
	<span class="status-badge grey">No identification yet</span>
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
