<script>
	// Inline SVG icons: 24x24, currentColor, Lucide/Feather style. Pick via `name`.
	// Play/stop are solid glyphs (fill) per DESIGN.md; the rest are stroked.
	let { name } = $props();

	const solid = $derived(name === 'play' || name === 'stop');
</script>

<svg
	class="icon"
	viewBox="0 0 24 24"
	fill={solid ? 'currentColor' : 'none'}
	stroke={solid ? 'none' : 'currentColor'}
	stroke-width="2"
	stroke-linecap="round"
	stroke-linejoin="round"
	aria-hidden="true"
>
	{#if name === 'play'}
		<path d="M8 5v14l11-7z" />
	{:else if name === 'stop'}
		<rect x="6" y="6" width="12" height="12" rx="1" />
	{:else if name === 'trash'}
		<polyline points="3 6 5 6 21 6" />
		<path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
		<line x1="10" y1="11" x2="10" y2="17" />
		<line x1="14" y1="11" x2="14" y2="17" />
	{:else if name === 'x'}
		<line x1="18" y1="6" x2="6" y2="18" />
		<line x1="6" y1="6" x2="18" y2="18" />
	{:else if name === 'help'}
		<circle cx="12" cy="12" r="10" />
		<path d="M9.09 9a3 3 0 0 1 5.83 1c0 2-3 3-3 3" />
		<line x1="12" y1="17" x2="12.01" y2="17" />
	{:else if name === 'plus'}
		<line x1="12" y1="5" x2="12" y2="19" />
		<line x1="5" y1="12" x2="19" y2="12" />
	{:else if name === 'more'}
		<circle cx="12" cy="5" r="1.6" />
		<circle cx="12" cy="12" r="1.6" />
		<circle cx="12" cy="19" r="1.6" />
	{:else if name === 'refresh'}
		<path d="M21 12a9 9 0 1 1-2.64-6.36" />
		<polyline points="21 3 21 8 16 8" />
	{:else if name === 'grip'}
		<line x1="4" y1="9" x2="20" y2="9" />
		<line x1="4" y1="15" x2="20" y2="15" />
	{:else if name === 'megaphone'}
		<path d="m3 11 18-5v12L3 14v-3z" />
		<path d="M11.6 16.8a3 3 0 1 1-5.8-1.6" />
	{:else if name === 'megaphone-off'}
		<path d="M9.26 9.26 3 11v3l14 4v-2.34" />
		<path d="M21 15V6l-6.5 1.86M11.6 16.8a3 3 0 1 1-5.8-1.6" />
		<line x1="2" y1="2" x2="22" y2="22" />
	{:else if name === 'ear'}
		<path d="M6 8.5a6.5 6.5 0 1 1 13 0c0 6-6 6-6 10a3.5 3.5 0 1 1-7 0" />
		<path d="M15 8.5a2.5 2.5 0 0 0-5 0v1a2 2 0 1 1 0 4" />
	{:else if name === 'undo'}
		<path d="M3 7v6h6" />
		<path d="M21 17a9 9 0 0 0-15-6.7L3 13" />
	{:else if name === 'grip-vertical'}
		<circle cx="9" cy="5" r="1" />
		<circle cx="9" cy="12" r="1" />
		<circle cx="9" cy="19" r="1" />
		<circle cx="15" cy="5" r="1" />
		<circle cx="15" cy="12" r="1" />
		<circle cx="15" cy="19" r="1" />
	{/if}
</svg>

<style lang="sass">
.icon
	width: 1.2em
	height: 1.2em
	display: block
</style>
