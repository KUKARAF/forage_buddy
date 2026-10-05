<script lang="ts">
	// In-app live camera capture: the PRIMARY way to add photos to a sighting
	// (see docs/ARCHITECTURE.md's capture flow + the follow-up UX request this
	// component implements). Requests `getUserMedia` (a genuine OS/browser
	// camera permission prompt, unlike `<input capture>` which silently hands
	// off to a separate system app), shows the live stream, and lets the user
	// take several shots in one session before handing them all back at once.
	//
	// Callers MUST treat `onunavailable` as "fall back to a plain file input" —
	// it fires both when `getUserMedia` doesn't exist at all and when the
	// permission prompt is denied/fails, so this component degrades to a no-op
	// (it never renders a broken camera UI) and the caller's existing
	// `<input type="file" capture="environment">` takes over.
	import { onDestroy, onMount } from 'svelte';

	interface Shot {
		id: string;
		file: File;
		url: string;
	}

	let {
		ondone,
		oncancel,
		onunavailable
	}: {
		/** Called once, when the user taps "Done", with every shot taken this
		 * session (in capture order). The camera stream is already stopped by
		 * the time this fires. Never called with an empty array — the "Done"
		 * button is disabled until at least one shot exists. */
		ondone: (files: File[]) => void;
		/** User tapped "Cancel" before taking/keeping any shots they want to use. */
		oncancel: () => void;
		/** `getUserMedia` is unsupported, or the permission request failed/was
		 * denied. The caller should fall back to its file-input button. */
		onunavailable: () => void;
	} = $props();

	let videoEl: HTMLVideoElement | undefined = $state();
	let stream: MediaStream | null = null;
	let shots = $state<Shot[]>([]);
	let starting = $state(true);
	let settled = false;

	function stopStream() {
		if (stream) {
			for (const track of stream.getTracks()) track.stop();
			stream = null;
		}
	}

	async function start() {
		if (typeof navigator === 'undefined' || !navigator.mediaDevices?.getUserMedia) {
			settled = true;
			onunavailable();
			return;
		}
		try {
			stream = await navigator.mediaDevices.getUserMedia({
				video: { facingMode: 'environment' }
			});
		} catch (err) {
			console.warn('Camera permission denied or unavailable:', err);
			settled = true;
			onunavailable();
			return;
		}
		starting = false;
		if (videoEl) {
			videoEl.srcObject = stream;
			await videoEl.play().catch(() => {});
		}
	}

	// Re-attach the stream if the <video> element mounts (or remounts) after
	// the getUserMedia promise already resolved.
	$effect(() => {
		if (videoEl && stream && videoEl.srcObject !== stream) {
			videoEl.srcObject = stream;
			void videoEl.play().catch(() => {});
		}
	});

	onMount(() => {
		void start();
	});

	function takeShot() {
		if (!videoEl || videoEl.videoWidth === 0) return;
		const canvas = document.createElement('canvas');
		canvas.width = videoEl.videoWidth;
		canvas.height = videoEl.videoHeight;
		const ctx = canvas.getContext('2d');
		if (!ctx) return;
		ctx.drawImage(videoEl, 0, 0, canvas.width, canvas.height);
		canvas.toBlob(
			(blob) => {
				if (!blob) return;
				const file = new File([blob], `photo-${Date.now()}.jpg`, { type: 'image/jpeg' });
				shots.push({ id: crypto.randomUUID(), file, url: URL.createObjectURL(file) });
			},
			'image/jpeg',
			0.9
		);
	}

	function removeShot(id: string) {
		const idx = shots.findIndex((s) => s.id === id);
		if (idx === -1) return;
		URL.revokeObjectURL(shots[idx].url);
		shots.splice(idx, 1);
	}

	function finish() {
		if (shots.length === 0) return;
		settled = true;
		stopStream();
		ondone(shots.map((s) => s.file));
	}

	function cancel() {
		settled = true;
		stopStream();
		for (const s of shots) URL.revokeObjectURL(s.url);
		oncancel();
	}

	// Belt-and-suspenders: if the component is torn down some other way
	// (parent navigates away, etc.) without finish()/cancel() running, never
	// leave the camera stream open.
	onDestroy(() => {
		if (!settled) stopStream();
	});
</script>

<div class="camera-overlay">
	{#if starting}
		<p class="status-text">Starting camera…</p>
	{/if}

	<!-- Always mounted (just hidden while starting) so `videoEl` is bound and
	     ready the instant the stream resolves. -->
	<video bind:this={videoEl} class="camera-video" class:hidden={starting} autoplay playsinline muted
	></video>

	{#if shots.length > 0}
		<div class="shot-strip">
			{#each shots as shot (shot.id)}
				<div class="shot-thumb">
					<img src={shot.url} alt="Captured specimen" />
					<button
						type="button"
						class="shot-remove"
						onclick={() => removeShot(shot.id)}
						aria-label="Remove this photo"
					>
						✕
					</button>
				</div>
			{/each}
		</div>
	{/if}

	<div class="camera-controls">
		<button type="button" class="btn secondary" onclick={cancel}>Cancel</button>
		<button
			type="button"
			class="shutter"
			disabled={starting}
			onclick={takeShot}
			aria-label="Take photo"
		></button>
		<button type="button" class="btn" disabled={shots.length === 0} onclick={finish}>
			Done{shots.length > 0 ? ` (${shots.length})` : ''}
		</button>
	</div>
</div>

<style>
	/* A full-screen viewfinder is, like the photo lightbox elsewhere in this
	   app, intentionally always-dark regardless of the light/dark theme
	   toggle — it's camera chrome, not page content, so it doesn't use the
	   --bg/--ink tokens. */
	.camera-overlay {
		position: fixed;
		inset: 0;
		background: #000;
		display: flex;
		flex-direction: column;
		z-index: 100;
		padding: calc(12px + var(--safe-top)) calc(12px + var(--safe-right))
			calc(12px + var(--safe-bottom)) calc(12px + var(--safe-left));
	}
	.status-text {
		color: #fff;
		margin: auto;
	}
	.camera-video {
		flex: 1;
		width: 100%;
		min-height: 0;
		object-fit: cover;
		border-radius: var(--radius-card);
		background: #000;
	}
	.camera-video.hidden {
		display: none;
	}
	.shot-strip {
		display: flex;
		gap: 8px;
		overflow-x: auto;
		padding: 10px 0;
	}
	.shot-thumb {
		position: relative;
		flex: none;
		width: 64px;
		height: 64px;
		border-radius: 8px;
		overflow: hidden;
		border: 2px solid #fff;
	}
	.shot-thumb img {
		width: 100%;
		height: 100%;
		object-fit: cover;
		display: block;
	}
	.shot-remove {
		position: absolute;
		top: -2px;
		right: -2px;
		width: 20px;
		height: 20px;
		min-height: 0;
		border-radius: 999px;
		border: none;
		background: var(--red);
		color: var(--btn-ink);
		font-size: 11px;
		line-height: 1;
		display: flex;
		align-items: center;
		justify-content: center;
		padding: 0;
	}
	.camera-controls {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 12px;
		padding-top: 10px;
	}
	.shutter {
		width: 64px;
		height: 64px;
		min-height: 0;
		border-radius: 999px;
		border: 4px solid #fff;
		background: var(--red);
		flex: none;
	}
	.shutter:disabled {
		opacity: 0.5;
	}
</style>
