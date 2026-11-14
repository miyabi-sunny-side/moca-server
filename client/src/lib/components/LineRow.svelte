<script>
	import Icon from '$lib/components/Icon.svelte';
	import Spinner from '$lib/components/Spinner.svelte';
	import EmotionChips from '$lib/components/EmotionChips.svelte';
	import EmotionEditor from '$lib/components/EmotionEditor.svelte';

	// One line row, left to right: drag grip, play button, line text (immutable in
	// place), emotion chips (acting only), mode toggle badge, delete button.
	// Clicking the card toggles the inline emotion editor on a 演技 line and does
	// nothing on an アナウンサー line — the card click is never an edit affordance.
	// All text operations live in the line context menu (right-click / long-press).
	// The playing row shows a 4px accent left bar.
	let {
		line,
		rev = 0,
		playing = false,
		spinner = false,
		toggling = false,
		draggable = false,
		dragging = false,
		dropBefore = false,
		dropAfter = false,
		// Controlled by the App (single source of truth) so keyboard (Enter/Space)
		// and click drive the same expand state; `focused` = the vim cursor.
		expanded = false,
		focused = false,
		onplay,
		onsave,
		ontoggle,
		onrequestDelete,
		onmenu,
		onToggleExpand,
		ondragstart,
		ondragover,
		ondrop,
		ondragend
	} = $props();

	let rowDraggable = $state(false);

	// The text the voice actually reads when the reading check respelled a
	// misread word (the shown text never changes).
	const respelled = $derived(
		(line.reading?.segments ?? [])
			.map((s) => s.spoken)
			.filter(Boolean)
			.join(' ')
	);

	function saveScript(script) {
		onsave?.(line, { script });
	}

	// Card click toggles the emotion editor on 演技 lines only; アナウンサー = no-op.
	function toggleExpand() {
		if (line.mode === 'acting') onToggleExpand?.(line);
	}

	// --- Context menu: right-click and long-press both open the line menu. The
	// browser context menu is intercepted on the card only (elsewhere it stays). ---
	function openMenu(e) {
		e.preventDefault();
		onmenu?.(line);
	}

	// Long-press (touch): ~500ms hold on the card fires the menu. Movement or an
	// early release cancels it. The grip is excluded so it never fights the drag.
	let pressTimer = null;
	function clearPress() {
		if (pressTimer) {
			clearTimeout(pressTimer);
			pressTimer = null;
		}
	}
	function onPointerDown(e) {
		if (e.pointerType === 'mouse') return; // right-click handles mouse
		clearPress();
		pressTimer = setTimeout(() => {
			pressTimer = null;
			// Swallow the ghost click that the browser synthesizes on touchend right
			// after the long-press — otherwise it lands on the just-opened menu's
			// scrim and immediately closes it (menu unreachable on touch).
			window.addEventListener(
				'click',
				(ev) => {
					ev.stopPropagation();
					ev.preventDefault();
				},
				{ capture: true, once: true }
			);
			onmenu?.(line);
		}, 500);
	}

	// --- Drag: only the grip initiates it; the row is draggable only while held. ---
	function handleDragStart(e) {
		if (!draggable) {
			e.preventDefault();
			return;
		}
		e.dataTransfer.effectAllowed = 'move';
		e.dataTransfer.setData('text/plain', String(line.id));
		ondragstart?.(line);
	}

	function handleDragOver(e) {
		if (!draggable) return;
		e.preventDefault();
		e.dataTransfer.dropEffect = 'move';
		const rect = e.currentTarget.getBoundingClientRect();
		const pos = e.clientY - rect.top < rect.height / 2 ? 'before' : 'after';
		ondragover?.(line, pos);
	}

	function handleDrop(e) {
		e.preventDefault();
		ondrop?.();
	}

	function handleDragEnd() {
		rowDraggable = false;
		ondragend?.();
	}
</script>

<svelte:window onpointerup={() => (rowDraggable = false)} />

<div
	class="row"
	class:playing
	class:focused
	class:dragging
	class:drop-before={dropBefore}
	class:drop-after={dropAfter}
	draggable={rowDraggable}
	oncontextmenu={openMenu}
	ondragstart={handleDragStart}
	ondragover={handleDragOver}
	ondrop={handleDrop}
	ondragend={handleDragEnd}
	role="listitem"
>
	<div class="header">
		<span
			class="grip"
			class:enabled={draggable}
			aria-hidden="true"
			onpointerdown={() => draggable && (rowDraggable = true)}
		>
			<Icon name="grip-vertical" />
		</span>

		<button
			type="button"
			class="play"
			class:active={playing}
			aria-label={playing ? '停止' : '再生'}
			onclick={() => onplay?.(line)}
		>
			{#if spinner}
				<Spinner size="1rem" />
			{:else}
				<Icon name={playing ? 'stop' : 'play'} />
			{/if}
		</button>

		<!-- Text area is the card-click / long-press target. Immutable in place. -->
		<div
			class="body"
			class:clickable={line.mode === 'acting'}
			role="presentation"
			onclick={toggleExpand}
			onpointerdown={onPointerDown}
			onpointermove={clearPress}
			onpointerup={clearPress}
			onpointercancel={clearPress}
		>
			<div class="text">{line.text}</div>
			{#if line.mode === 'acting'}
				<EmotionChips script={line.script} />
			{/if}
		</div>

		{#if respelled}
			<span class="reading-mark" title={`読み上げ: ${respelled}`}>読み</span>
		{/if}

		<button
			type="button"
			class="badge"
			class:acting={line.mode === 'acting'}
			disabled={toggling}
			aria-label={line.mode === 'acting' ? 'アナウンサーに戻す' : '演技に分析'}
			onclick={() => ontoggle?.(line)}
		>
			{#if toggling}
				<Spinner size="0.9rem" />
			{:else}
				{line.mode === 'acting' ? '演技' : 'アナ'}
			{/if}
		</button>

		<button
			type="button"
			class="icon-btn"
			aria-label="行を削除"
			onclick={() => onrequestDelete?.(line)}
		>
			<Icon name="trash" />
		</button>
	</div>

	{#if expanded && line.mode === 'acting' && line.script}
		<div class="expansion">
			<!-- Remount (re-seed) only when a mode toggle bumps rev, not on autosave reloads. -->
			{#key rev}
				<EmotionEditor script={line.script} onsave={saveScript} />
			{/key}
		</div>
	{/if}
</div>

<style lang="sass">
.row
	position: relative
	padding: var(--sp-2) var(--sp-3)
	border-radius: var(--radius-md)
	background: var(--c-surface)
	border: 1px solid var(--c-border)

	&.playing
		border-left: 4px solid var(--c-accent)
		padding-left: calc(var(--sp-3) - 3px)

	&.dragging
		opacity: 0.5

	// 2px accent insertion line at the drop position, centered in the row gap.
	&.drop-before::before,
	&.drop-after::after
		content: ''
		position: absolute
		left: 0
		right: 0
		height: 2px
		background: var(--c-accent)

	&.drop-before::before
		top: -5px

	&.drop-after::after
		bottom: -5px

// Vim cursor: the shared focus-ring recipe. Drawn only when the 台本 column is
// the active pane (the cursor is remembered but not painted on the idle column).
:global([data-active-col='line']) .row.focused
	outline: 2px solid var(--c-accent-active)
	outline-offset: 2px

.header
	display: flex
	align-items: center
	gap: var(--sp-2)

.grip
	display: flex
	align-items: center
	justify-content: center
	flex: none
	width: 18px
	color: var(--c-text-muted)
	cursor: default
	touch-action: none

	&.enabled
		cursor: grab

		&:active
			cursor: grabbing

	&:not(.enabled)
		opacity: 0.35

.play
	display: flex
	align-items: center
	justify-content: center
	flex: none
	width: 36px
	height: 36px
	padding: 0
	border: 1px solid var(--c-border)
	border-radius: var(--radius-sm)
	background: var(--c-surface)
	color: var(--c-text)
	cursor: pointer

	&:hover
		background: var(--c-hover-2)

	&.active
		color: var(--c-accent)
		border-color: var(--c-accent-border)

.body
	flex: 1
	min-width: 0

	// 演技 rows expand the editor on click; announcer rows are inert.
	&.clickable
		cursor: pointer

	// Immutable display text: quiet, two lines max, ellipsized. No affordance.
	.text
		display: -webkit-box
		width: 100%
		color: var(--c-text)
		font-size: var(--fs-sm)
		line-height: 1.6
		-webkit-line-clamp: 2
		-webkit-box-orient: vertical
		overflow: hidden

// Reading-check mark: this row is spoken with a respelled text. Caption type,
// neutral chrome (a state of the row, not an action).
.reading-mark
	flex: none
	padding: 1px var(--sp-1)
	border: 1px dashed var(--c-border)
	border-radius: var(--radius-sm)
	color: var(--c-text-muted)
	font-size: var(--fs-xs)
	white-space: nowrap
	cursor: default

// Mode toggle: a real button that stays as quiet as an outline badge —
// affordance is cursor + hover wash only, no weight or fill change.
.badge
	display: inline-flex
	align-items: center
	justify-content: center
	flex: none
	min-width: 2.75rem
	min-height: 1.5rem
	padding: 1px var(--sp-2)
	border-radius: var(--radius-full)
	font-size: var(--fs-xs)
	color: var(--c-text-muted)
	background: transparent
	border: 1px solid var(--c-border)
	cursor: pointer

	&.acting
		color: var(--c-text-sub)

	&:hover
		background: var(--c-hover-2)

	&:disabled
		cursor: wait

.icon-btn
	display: flex
	align-items: center
	justify-content: center
	flex: none
	width: 36px
	height: 36px
	padding: 0
	border: none
	border-radius: var(--radius-sm)
	background: transparent
	color: var(--c-text-sub)
	cursor: pointer

	&:hover
		color: var(--c-danger)

.expansion
	margin-top: var(--sp-3)
</style>
