<script lang="ts">
	// An <img> that works for both auth modes: direct URL in web/cookie mode,
	// fetched-and-blobbed in Tauri/bearer mode (see $lib/app/photoUrl). Revokes
	// its object URL on destroy/photo change to avoid leaking memory.
	import { onDestroy } from 'svelte';
	import { resolvePhotoSrc } from '$lib/app/photoUrl';

	let {
		photoId,
		alt = '',
		class: className = ''
	}: { photoId: string; alt?: string; class?: string } = $props();

	let src = $state<string | null>(null);
	let failed = $state(false);
	let objectUrl: string | null = null;

	function revoke() {
		if (objectUrl) {
			URL.revokeObjectURL(objectUrl);
			objectUrl = null;
		}
	}

	async function load(id: string) {
		revoke();
		failed = false;
		src = null;
		try {
			const url = await resolvePhotoSrc(id);
			if (url.startsWith('blob:')) objectUrl = url;
			src = url;
		} catch {
			failed = true;
		}
	}

	$effect(() => {
		void load(photoId);
	});

	onDestroy(revoke);
</script>

{#if src}
	<img {src} {alt} class={className} loading="lazy" />
{:else if failed}
	<div class={`photo-fallback ${className}`} aria-label={alt || 'Photo unavailable'}>?</div>
{:else}
	<div class={`photo-fallback loading ${className}`} aria-label={alt || 'Loading photo'}></div>
{/if}

<style>
	.photo-fallback {
		display: flex;
		align-items: center;
		justify-content: center;
		background: var(--tag-bg);
		color: var(--muted);
		font-weight: 700;
	}
	.photo-fallback.loading {
		animation: pulse 1.4s ease-in-out infinite;
	}
	@keyframes pulse {
		0%,
		100% {
			opacity: 0.6;
		}
		50% {
			opacity: 1;
		}
	}
</style>
