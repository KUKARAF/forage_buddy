<script lang="ts">
	// Renders ONE identification candidate, compactly: name + confidence,
	// four fact chips, an optional one-line risk note, and confusants as
	// short inline chips. Never a paragraph, never a bulleted checklist —
	// that's the "wall of text" pattern this replaces (the old deep-dive
	// card's distinguishing-features checklist).
	import type { IdentificationCandidate } from '$lib/api/client';
	import { confidencePct } from '$lib/format';
	import FactBadge from './FactBadge.svelte';

	let { candidate }: { candidate: IdentificationCandidate } = $props();

	const displayName = $derived(candidate.common_name || candidate.species);
	const pct = $derived(confidencePct(candidate.confidence));

	const DANGER_LABEL: Record<string, string> = {
		unknown: 'unknown risk',
		mild: 'mild',
		toxic: 'toxic',
		deadly_toxic: 'deadly toxic'
	};
</script>

<div class="id-card">
	<div class="id-head">
		<div class="id-name">
			{#if candidate.wikipedia_url}
				<a
					class="species-link"
					href={candidate.wikipedia_url}
					target="_blank"
					rel="noopener noreferrer external"
				>
					<strong>{displayName}</strong>
				</a>
			{:else}
				<strong>{displayName}</strong>
			{/if}
			{#if candidate.common_name}
				<span class="muted small">({candidate.species})</span>
			{/if}
		</div>
		<span class="confidence-badge">{pct}%</span>
	</div>

	<div class="facts">
		<FactBadge fact="edible" value={candidate.edible} />
		<FactBadge fact="medicinal" value={candidate.medicinal} />
		<FactBadge fact="psychoactive" value={candidate.psychoactive} />
		<FactBadge fact="poisonous" value={candidate.poisonous} />
	</div>

	{#if candidate.risk_note}
		<p class="risk-note">{candidate.risk_note}</p>
	{/if}

	{#if candidate.wikipedia_url}
		<a
			class="wiki-link"
			href={candidate.wikipedia_url}
			target="_blank"
			rel="noopener noreferrer external"
		>
			Wikipedia ↗
		</a>
	{/if}

	{#if candidate.confusants.length > 0}
		<div class="confusants">
			{#each candidate.confusants as confusant (confusant.species)}
				{@const phrase = `⚠ looks like ${confusant.species} (${confusant.note || DANGER_LABEL[confusant.danger_level]})`}
				{#if confusant.wikipedia_url}
					<a
						class="confusant-chip level-{confusant.danger_level}"
						href={confusant.wikipedia_url}
						target="_blank"
						rel="noopener noreferrer external"
						title={confusant.note}
					>
						{phrase}
					</a>
				{:else}
					<span class="confusant-chip level-{confusant.danger_level}" title={confusant.note}>
						{phrase}
					</span>
				{/if}
			{/each}
		</div>
	{/if}
</div>

<style>
	.id-card {
		border: 1px solid var(--line);
		border-radius: var(--radius-card);
		background: var(--card);
		padding: 14px 16px;
	}
	.id-card + .id-card {
		margin-top: 12px;
	}
	.id-head {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: 10px;
		margin-bottom: 10px;
	}
	.id-name {
		min-width: 0;
	}
	.species-link {
		color: inherit;
		text-decoration: none;
	}
	.species-link:hover {
		text-decoration: underline;
	}
	.confidence-badge {
		flex: none;
		font-weight: 700;
		font-size: 12.5px;
		color: var(--muted);
		background: var(--tag-bg);
		border-radius: 999px;
		padding: 4px 10px;
	}
	.small {
		font-size: 12.5px;
	}

	.facts {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
		margin-bottom: 8px;
	}

	.risk-note {
		margin: 0 0 8px;
		font-size: 13.5px;
		color: var(--ink);
	}

	.wiki-link {
		display: inline-block;
		font-size: 13px;
		font-weight: 600;
		color: var(--accent);
		margin-bottom: 8px;
	}

	.confusants {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
		margin-top: 4px;
	}
	.confusant-chip {
		display: inline-flex;
		align-items: center;
		font-size: 12.5px;
		border-radius: 999px;
		border: 1px solid var(--line);
		border-left: 4px solid var(--grey);
		background: var(--tag-bg);
		color: inherit;
		text-decoration: none;
		padding: 4px 10px;
		white-space: nowrap;
	}
	a.confusant-chip:hover {
		background: var(--hover-overlay);
	}
	.confusant-chip.level-unknown {
		border-left-color: var(--grey);
	}
	.confusant-chip.level-mild {
		border-left-color: var(--amber);
	}
	.confusant-chip.level-toxic {
		border-left-color: var(--orange);
	}
	.confusant-chip.level-deadly_toxic {
		border-left-color: var(--red);
		background: var(--red-bg);
	}
</style>
