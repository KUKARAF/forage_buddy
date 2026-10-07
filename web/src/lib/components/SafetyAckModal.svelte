<script lang="ts">
	// Gates seeing an identification result behind a typed acknowledgement,
	// once per session (see $lib/identificationAck). Frontend-only UX gate —
	// no corresponding backend enforcement, by design.
	const REQUIRED_PHRASE = 'never munch on a hunch';

	let { onacknowledge }: { onacknowledge: () => void } = $props();

	let typed = $state('');
	let inputEl: HTMLInputElement | undefined = $state();

	const canContinue = $derived(typed.trim().toLowerCase() === REQUIRED_PHRASE);

	function continueClick() {
		if (!canContinue) return;
		onacknowledge();
	}

	function onKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter') continueClick();
	}

	$effect(() => {
		inputEl?.focus();
	});
</script>

<div class="modal-overlay" role="dialog" aria-modal="true" aria-label="Safety disclaimer">
	<div class="modal-card">
		<p class="disclaimer-text">
			Results are fetched by AI and can therefore be wildly inaccurate. Say it with me: never munch
			on a hunch.
		</p>
		<label for="safety-ack-input" class="input-label"
			>Type "never munch on a hunch" to continue</label
		>
		<input
			id="safety-ack-input"
			bind:this={inputEl}
			type="text"
			autocomplete="off"
			bind:value={typed}
			onkeydown={onKeydown}
		/>
		<button class="btn" disabled={!canContinue} onclick={continueClick}>Continue</button>
	</div>
</div>

<style>
	.modal-overlay {
		position: fixed;
		inset: 0;
		background: var(--overlay);
		display: flex;
		align-items: center;
		justify-content: center;
		padding: 24px;
		z-index: 80;
	}
	.modal-card {
		background: var(--card);
		border-radius: var(--radius-card);
		box-shadow: var(--shadow-strong);
		padding: 20px;
		max-width: 420px;
		width: 100%;
	}
	.disclaimer-text {
		margin: 0 0 14px;
		font-size: 14.5px;
		line-height: 1.5;
	}
	.input-label {
		margin-bottom: 6px;
	}
	.modal-card .btn {
		width: 100%;
		margin-top: 12px;
	}
</style>
