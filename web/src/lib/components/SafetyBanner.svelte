<script lang="ts">
	// Persistent safety banner. Dismissible for the current in-memory session
	// (plain component state, nothing persisted) but reappears on a full
	// reload — the point is to make it easy to tuck away while reading a
	// screen, never to let it be silenced for good. Rendered on every screen
	// that shows (or can show) a species guess: the sighting list, the detail
	// screen's triage/deep-dive cards.
	import { SAFETY_DISCLAIMER } from '$lib/disclaimer';

	let dismissed = $state(false);
</script>

{#if !dismissed}
	<div class="safety-banner" role="note" aria-label="Safety disclaimer">
		<span class="icon" aria-hidden="true">⚠️</span>
		<p>{SAFETY_DISCLAIMER}</p>
		<button
			class="dismiss"
			onclick={() => (dismissed = true)}
			aria-label="Dismiss safety disclaimer for now"
		>
			✕
		</button>
	</div>
{/if}

<style>
	.safety-banner {
		display: flex;
		align-items: flex-start;
		gap: 10px;
		background: var(--amber-bg);
		border: 1px solid var(--amber);
		color: var(--ink);
		border-radius: var(--radius-card);
		padding: 12px 14px;
		margin-bottom: 18px;
	}
	.icon {
		font-size: 18px;
		line-height: 1.4;
		flex: none;
	}
	.safety-banner p {
		margin: 0;
		font-size: 13.5px;
		line-height: 1.5;
		flex: 1;
	}
	.dismiss {
		flex: none;
		background: transparent;
		border: none;
		color: var(--ink);
		font-size: 16px;
		min-width: 32px;
		min-height: 32px;
		border-radius: 8px;
	}
	.dismiss:hover {
		background: var(--hover-overlay);
	}
</style>
