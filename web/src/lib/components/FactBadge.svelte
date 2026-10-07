<script lang="ts" module>
	export type Fact = 'edible' | 'medicinal' | 'psychoactive' | 'poisonous';
</script>

<script lang="ts">
	// The small bool→icon/color chip for one of the 4 identification facts
	// (edible/medicinal/psychoactive/poisonous). Tri-state: `null` renders as
	// a neutral "unknown" chip, never as if it were `false`. Deliberately a
	// single short label — never a sentence — per the product owner's "no
	// walls of text" complaint about the old deep-dive UI.
	let { fact, value }: { fact: Fact; value: boolean | null } = $props();

	// `trueTone`: the color used when this fact is true. Facts that are
	// "good to know you can do this" (edible, medicinal) read positive in
	// green; facts that are warnings (psychoactive, poisonous) read as
	// caution/danger even when merely "true" is the accurate answer.
	const CONFIG: Record<Fact, { icon: string; label: string; trueTone: 'green' | 'amber' | 'red' }> =
		{
			edible: { icon: '🍽', label: 'Edible', trueTone: 'green' },
			medicinal: { icon: '💊', label: 'Medicinal', trueTone: 'green' },
			psychoactive: { icon: '🌀', label: 'Psychoactive', trueTone: 'amber' },
			poisonous: { icon: '☠', label: 'Poisonous', trueTone: 'red' }
		};

	const cfg = $derived(CONFIG[fact]);
	const tone = $derived(value === null ? 'grey' : value ? cfg.trueTone : 'grey');
	const icon = $derived(value === null ? '—' : cfg.icon);
	const text = $derived(
		value === null ? 'Unknown' : value ? cfg.label : `Not ${cfg.label.toLowerCase()}`
	);
</script>

<span class="fact-badge tone-{tone}">
	<span aria-hidden="true">{icon}</span>
	{text}
</span>

<style>
	.fact-badge {
		display: inline-flex;
		align-items: center;
		gap: 5px;
		font-weight: 700;
		font-size: 12.5px;
		letter-spacing: 0.01em;
		padding: 4px 10px;
		border-radius: 999px;
		white-space: nowrap;
	}
	.tone-grey {
		background: var(--grey-bg);
		color: var(--grey);
	}
	.tone-green {
		background: var(--green-bg);
		color: var(--green);
	}
	.tone-amber {
		background: var(--amber-bg);
		color: var(--amber);
	}
	.tone-red {
		background: var(--red-bg);
		color: var(--red);
	}
</style>
