<script lang="ts">
	import { confidencePct } from '$lib/format';

	let { confidence }: { confidence: number } = $props();

	const pct = $derived(confidencePct(confidence));
</script>

<div class="confidence-bar" role="img" aria-label={`Confidence ${pct}%`}>
	<div class="track">
		<div class="fill" style={`width: ${pct}%`}></div>
	</div>
	<span class="pct">{pct}%</span>
</div>

<style>
	.confidence-bar {
		display: flex;
		align-items: center;
		gap: 8px;
	}
	.track {
		flex: 1;
		height: 8px;
		border-radius: 999px;
		background: var(--tag-bg);
		overflow: hidden;
	}
	.fill {
		height: 100%;
		background: var(--moss);
		border-radius: 999px;
	}
	.pct {
		font-size: 12.5px;
		font-weight: 700;
		color: var(--muted);
		min-width: 36px;
		text-align: right;
	}
</style>
