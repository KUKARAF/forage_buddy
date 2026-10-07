<script lang="ts">
	// Color-coded badge for a DangerLevel: grey=unknown, amber=mild,
	// orange=toxic, red=deadly_toxic. `deadly_toxic` is rendered unmistakably
	// alarming (bold, bordered, exclamation) rather than a subtle tint — this
	// is the one case where "fits the design" must lose to "cannot be missed".
	import type { DangerLevel } from '$lib/api/client';

	let { level, label }: { level: DangerLevel; label?: string } = $props();

	const text: Record<DangerLevel, string> = {
		unknown: 'Unknown risk',
		mild: 'Mild risk',
		toxic: 'Toxic',
		deadly_toxic: 'DEADLY TOXIC'
	};
</script>

<span class="danger-badge level-{level}" class:alarming={level === 'deadly_toxic'}>
	{#if level === 'deadly_toxic'}<span aria-hidden="true">⚠️</span>{/if}
	{label ?? text[level]}
</span>

<style>
	.danger-badge {
		display: inline-flex;
		align-items: center;
		gap: 5px;
		font-weight: 700;
		font-size: 12.5px;
		letter-spacing: 0.01em;
		padding: 4px 10px;
		border-radius: 999px;
		border: 1px solid transparent;
		white-space: nowrap;
	}
	.level-unknown {
		background: var(--grey-bg);
		color: var(--grey);
	}
	.level-mild {
		background: var(--amber-bg);
		color: var(--amber);
	}
	.level-toxic {
		background: var(--orange-bg);
		color: var(--orange);
	}
	.level-deadly_toxic {
		background: var(--red-bg);
		color: var(--red);
	}
	.alarming {
		border-color: var(--red);
		font-size: 13.5px;
		box-shadow: 0 0 0 1px var(--red) inset;
	}
</style>
