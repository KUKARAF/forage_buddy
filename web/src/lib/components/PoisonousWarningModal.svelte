<script lang="ts">
	// Separate, additional alert from SafetyAckModal: fires once per sighting
	// view, specifically when a complete identification result has at least
	// one candidate with `poisonous === true`. Caller is responsible for the
	// "once, not every poll tick" gating (see the `poisonousWarningSeen` flag
	// in the sighting detail page) — this component just renders the alert.
	let { ondismiss }: { ondismiss: () => void } = $props();
</script>

<div class="modal-overlay" role="alertdialog" aria-modal="true" aria-label="Poisonous warning">
	<div class="modal-card">
		<p class="warning-text">⚠️ There's a chance this specimen is poisonous! Be very careful!</p>
		<button class="btn danger" onclick={ondismiss}>OK</button>
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
		z-index: 90;
	}
	.modal-card {
		background: var(--card);
		border: 1px solid var(--red);
		border-radius: var(--radius-card);
		box-shadow: var(--shadow-strong);
		padding: 20px;
		max-width: 380px;
		width: 100%;
		text-align: center;
	}
	.warning-text {
		margin: 0 0 14px;
		font-size: 15px;
		font-weight: 700;
		color: var(--red);
		line-height: 1.5;
	}
	.modal-card .btn {
		width: 100%;
	}
</style>
