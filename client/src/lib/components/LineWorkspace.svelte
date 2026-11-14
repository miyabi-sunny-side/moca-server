<script>
	import LineRow from '$lib/components/LineRow.svelte';
	import Icon from '$lib/components/Icon.svelte';
	import { player } from '$lib/player.svelte.js';

	// Detail pane: the line workspace. A scrolling column of line rows in
	// narration order, plus a sticky footer whose primary action is pour-in.
	let {
		project,
		editorRev = {},
		toggleBusy = {},
		expandedIds = {},
		lineCursor = null,
		onrename,
		onplay,
		onsave,
		onreorder,
		ontoggle,
		onrequestDelete,
		onmenu,
		onToggleExpand,
		onpourin,
		onhelp,
		oncheck
	} = $props();

	let editingName = $state(false);
	let draftName = $state('');

	function startRename() {
		draftName = project.name;
		editingName = true;
	}

	function commitRename() {
		const name = draftName.trim();
		if (name && name !== project.name) onrename?.(name);
		editingName = false;
	}

	// --- Drag reorder state (owned here; LineRow reports drag events) ---
	let dragId = $state(null); // id of the row being dragged
	let overId = $state(null); // id of the row the pointer is over
	let overPos = $state(null); // 'before' | 'after' relative to overId

	function onDragStart(line) {
		dragId = line.id;
	}
	function onDragOver(line, pos) {
		if (dragId == null) return;
		if (line.id === dragId) return; // dropping onto self is a no-op; skip indicator too
		overId = line.id;
		overPos = pos;
	}
	function clearDrag() {
		dragId = null;
		overId = null;
		overPos = null;
	}
	function onDrop() {
		const droppedId = dragId;
		const targetId = overId;
		const pos = overPos;
		clearDrag();
		const original = project.lines.map((l) => l.id);
		const ids = original.slice();
		const from = ids.indexOf(droppedId);
		if (from === -1 || ids.indexOf(targetId) === -1) return;
		const [moved] = ids.splice(from, 1);
		// Recompute the target index after removal, then insert before/after it.
		let target = ids.indexOf(targetId);
		if (pos === 'after') target += 1;
		ids.splice(target, 0, moved);
		if (ids.every((id, i) => id === original[i])) return; // no-op: order unchanged
		onreorder?.(ids);
	}
</script>

<div class="pane">
	<div class="pane-head">
		{#if editingName}
			<form
				onsubmit={(e) => {
					e.preventDefault();
					commitRename();
				}}
			>
				<!-- svelte-ignore a11y_autofocus -->
				<input bind:value={draftName} onblur={commitRename} autofocus />
			</form>
		{:else}
			<button type="button" class="title-btn" onclick={startRename} title="クリックで改名">
				{project.name}
			</button>
		{/if}
		<span class="count">{project.lines.length} 行</span>
		{#if project.lines.length > 0}
			<button type="button" class="help check" aria-label="読みを確認" title="読みを確認" onclick={oncheck}>
				<Icon name="ear" />
			</button>
		{/if}
	</div>

	<div class="lines">
		{#if project.lines.length === 0}
			<p class="empty">まだ行がありません。下の「台本追加」から台本を追加しましょう。</p>
		{:else}
			{#each project.lines as line (line.id)}
				<LineRow
					{line}
					rev={editorRev[line.id] ?? 0}
					playing={player.playingId === line.id}
					spinner={player.spinnerId === line.id}
					toggling={toggleBusy[line.id] ?? false}
					expanded={expandedIds[line.id] ?? false}
					focused={line.id === lineCursor}
					draggable={project.lines.length > 1}
					dragging={dragId === line.id}
					dropBefore={overId === line.id && overPos === 'before'}
					dropAfter={overId === line.id && overPos === 'after'}
					{onplay}
					{onsave}
					{ontoggle}
					{onrequestDelete}
					{onmenu}
					{onToggleExpand}
					ondragstart={onDragStart}
					ondragover={onDragOver}
					ondrop={onDrop}
					ondragend={clearDrag}
				/>
			{/each}
		{/if}
	</div>

	<div class="footer">
		<button type="button" class="primary" onclick={onpourin}>
			<Icon name="plus" /> 台本追加
		</button>
		<button type="button" class="help" aria-label="ショートカット一覧" onclick={onhelp}>
			<Icon name="help" />
		</button>
	</div>
</div>

<style lang="sass">
.pane
	display: flex
	flex-direction: column
	height: 100%
	min-height: 0

// Shares --pane-head-h with ProjectList so the two pane headers align across
// the boundary and the workspace reads as one surface.
.pane-head
	display: flex
	align-items: center
	gap: var(--sp-3)
	min-height: var(--pane-head-h)
	padding: var(--sp-3)
	background: var(--c-wash-base)
	border-bottom: 1px solid var(--c-border)

	.title-btn
		margin: 0
		padding: 0
		background: transparent
		border: none
		color: var(--c-text)
		font-size: var(--fs-xl)
		font-weight: 600
		text-align: left
		cursor: text

	input
		font-family: inherit
		font-size: var(--fs-xl)
		font-weight: 600
		color: var(--c-text)
		background: var(--c-bg)
		border: 1px solid var(--c-accent)
		border-radius: var(--radius-sm)
		padding: 2px var(--sp-2)

	.count
		font-size: var(--fs-xs)
		color: var(--c-text-muted)

// 読みを確認: quiet icon-button at the far right of the head. The head's height
// is shared with the project pane, so the button must not grow it.
.check
	margin-left: auto
	margin-block: calc(var(--sp-2) * -1)

.lines
	flex: 1
	min-height: 0
	overflow: auto
	display: flex
	flex-direction: column
	gap: var(--sp-2)
	padding: var(--sp-3)

.empty
	color: var(--c-text-muted)
	font-size: var(--fs-sm)

.footer
	position: sticky
	bottom: 0
	display: flex
	align-items: center
	justify-content: space-between
	padding: var(--sp-3)
	border-top: 1px solid var(--c-border)
	background: var(--c-wash-base)

// Quiet icon-button (Modal .close recipe): 36px hit area, no fill, sub text color.
.help
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
		color: var(--c-text)

.primary
	display: inline-flex
	align-items: center
	gap: var(--sp-1)
	padding: var(--sp-2) var(--sp-4)
	background: var(--c-accent-strong)
	border: 1px solid var(--c-accent-strong)
	border-radius: var(--radius-sm)
	color: var(--c-on-accent)
	font-size: var(--fs-md)
	cursor: pointer
</style>
