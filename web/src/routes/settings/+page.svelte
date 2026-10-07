<script lang="ts">
	import { onMount } from 'svelte';
	import { getSettings, updateSettings, ApiError, type Settings } from '$lib/api/client';

	type ModelField = 'candidate_model' | 'facts_model' | 'risk_model' | 'visual_match_model';

	// One row per runtime-editable model slot. Kept short on purpose — no
	// walls of text, just enough to tell the 4 slots apart.
	const SLOTS: { field: ModelField; label: string; hint: string }[] = [
		{
			field: 'candidate_model',
			label: 'Candidate identification',
			hint: 'Photo → species guess.'
		},
		{
			field: 'facts_model',
			label: 'Species facts',
			hint: 'Edible / medicinal / psychoactive / poisonous lookup.'
		},
		{
			field: 'risk_model',
			label: 'Look-alike risk lookup',
			hint: 'Finds dangerous look-alikes for the top candidate.'
		},
		{
			field: 'visual_match_model',
			label: 'Visual match check',
			hint: 'Compares your photo to a Wikipedia reference photo.'
		}
	];

	let loading = $state(true);
	let loadError = $state<string | null>(null);
	let availableModels = $state<string[]>([]);
	let values = $state<Record<ModelField, string>>({
		candidate_model: '',
		facts_model: '',
		risk_model: '',
		visual_match_model: ''
	});

	let saving = $state(false);
	let saveMessage = $state<string | null>(null);
	let saveError = $state<string | null>(null);

	function applySettings(settings: Settings) {
		availableModels = settings.available_models;
		values = {
			candidate_model: settings.candidate_model,
			facts_model: settings.facts_model,
			risk_model: settings.risk_model,
			visual_match_model: settings.visual_match_model
		};
	}

	async function load() {
		loading = true;
		loadError = null;
		try {
			applySettings(await getSettings());
		} catch (err) {
			loadError = err instanceof ApiError ? err.message : 'Could not load settings.';
		} finally {
			loading = false;
		}
	}

	async function save() {
		saving = true;
		saveMessage = null;
		saveError = null;
		try {
			const updated = await updateSettings({ ...values });
			applySettings(updated);
			saveMessage = 'Saved.';
		} catch (err) {
			saveError = err instanceof ApiError ? err.message : 'Could not save settings.';
		} finally {
			saving = false;
		}
	}

	onMount(load);
</script>

<svelte:head>
	<title>Settings — Forage Buddy</title>
</svelte:head>

<h1>Settings</h1>

{#if loading}
	<p class="muted">Loading settings…</p>
{:else if loadError}
	<div class="card error">
		<p>{loadError}</p>
		<button class="btn secondary" onclick={load}>Try again</button>
	</div>
{:else}
	<form
		onsubmit={(e) => {
			e.preventDefault();
			void save();
		}}
	>
		{#each SLOTS as slot (slot.field)}
			<div class="field">
				<label for={slot.field}>{slot.label}</label>
				<select id={slot.field} bind:value={values[slot.field]}>
					{#each availableModels as model (model)}
						<option value={model}>{model}</option>
					{/each}
				</select>
				<p class="hint muted small">{slot.hint}</p>
			</div>
		{/each}

		{#if saveError}
			<p class="error-text">{saveError}</p>
		{/if}
		{#if saveMessage}
			<p class="success-text">{saveMessage}</p>
		{/if}

		<button class="btn" type="submit" disabled={saving}>
			{saving ? 'Saving…' : 'Save'}
		</button>
	</form>
{/if}

<style>
	.hint {
		margin: 6px 0 0;
	}

	.small {
		font-size: 12.5px;
	}

	select {
		width: 100%;
		min-height: 44px;
		padding: 10px 12px;
		border: 1px solid var(--line);
		border-radius: var(--radius-btn);
		background: var(--input-bg);
		color: var(--ink);
	}

	.card.error {
		border-color: var(--red);
		background: var(--red-bg);
	}

	.error-text {
		color: var(--red);
		font-size: 13.5px;
		margin: 6px 0;
	}

	.success-text {
		color: var(--green);
		font-size: 13.5px;
		margin: 6px 0;
	}
</style>
