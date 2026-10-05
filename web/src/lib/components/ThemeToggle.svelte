<script lang="ts">
	// Small reachable-from-every-screen control (lives in the layout's topbar)
	// that cycles system -> light -> dark -> system. Shows an icon for the
	// *effective* theme (sun/moon) plus a small "A" badge while following the
	// OS setting, so the current state is always legible at a glance.
	import { theme, type ThemePreference } from '$lib/theme.svelte';

	const labels: Record<ThemePreference, string> = {
		system: 'Theme: matching your system',
		light: 'Theme: light',
		dark: 'Theme: dark'
	};
</script>

<button
	type="button"
	class="theme-toggle"
	onclick={() => theme.cycle()}
	title={`${labels[theme.preference]} — tap to change`}
	aria-label={`${labels[theme.preference]}. Tap to change theme.`}
>
	{#if theme.effective === 'dark'}
		<span aria-hidden="true">🌙</span>
	{:else}
		<span aria-hidden="true">☀️</span>
	{/if}
	{#if theme.preference === 'system'}
		<span class="auto-badge" aria-hidden="true">A</span>
	{/if}
</button>

<style>
	.theme-toggle {
		position: relative;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 40px;
		height: 40px;
		min-height: 0;
		padding: 0;
		border-radius: 999px;
		border: 1px solid var(--line);
		background: var(--card);
		font-size: 18px;
		line-height: 1;
	}
	.theme-toggle:hover {
		background: var(--tag-bg);
	}
	.auto-badge {
		position: absolute;
		bottom: -2px;
		right: -2px;
		width: 16px;
		height: 16px;
		border-radius: 999px;
		background: var(--forest);
		color: var(--btn-ink);
		font-size: 9.5px;
		font-weight: 800;
		display: flex;
		align-items: center;
		justify-content: center;
		border: 1px solid var(--card);
	}
</style>
