<script>
	import ProjectList from '$lib/components/ProjectList.svelte';
	import LineWorkspace from '$lib/components/LineWorkspace.svelte';
	import PourInModal from '$lib/components/PourInModal.svelte';
	import ConfirmModal from '$lib/components/ConfirmModal.svelte';
	import LineContextMenu from '$lib/components/LineContextMenu.svelte';
	import TextEditModal from '$lib/components/TextEditModal.svelte';
	import NewLineModal from '$lib/components/NewLineModal.svelte';
	import HelpModal from '$lib/components/HelpModal.svelte';
	import ReadingModal from '$lib/components/ReadingModal.svelte';
	import DictionaryView from '$lib/components/DictionaryView.svelte';
	import WorkView from '$lib/components/WorkView.svelte';
	import NotifySubscribe from '$lib/components/NotifySubscribe.svelte';
	import Toast from '$lib/components/Toast.svelte';
	import * as api from '$lib/api.js';
	import { play, playAll, stop, player } from '$lib/player.svelte.js';
	import { addToast } from '$lib/toast.svelte.js';
	import { router, navigate } from '$lib/router.svelte.js';

	let projects = $state(null);
	let selectedId = $state(null);
	let selected = $state(null); // full project detail with lines
	// Which top-level view is active (router.tab): 台本 = two-pane workspace,
	// 辞書 = dictionary, 作業 = pomodoro work-companion. タブは URL (/, /dict,
	// /work) と同期し、リロードしても保たれる。Global vim shortcuts stay
	// 台本-only (onWindowKeydown bails on the other tabs).
	// Pour-in modal state. `afterLineId` inserts after that line; null = append end.
	let pourIn = $state(false);
	let pourInAfter = $state(null);
	let readingOpen = $state(false);
	let confirm = $state(null); // { kind, target, busy }
	// Line context menu / text edit targets (null = closed).
	let menuLine = $state(null);
	let textEditLine = $state(null);
	// Caret placement for TextEditModal: 'end' (vim a) / 'start' (vim i) /
	// undefined (menu default = select-all).
	let textEditCaret = $state(undefined);
	// NewLineModal state for the o / O shortcuts: { pos: 'below' | 'above' } | null.
	let newLine = $state(null);
	// Project-title edit (プロジェクト列 a/i): { id, name, caret } | null. Distinct
	// from the workspace inline rename — this edits the *focused* project, which may
	// not be the one that is open.
	let projectEdit = $state(null);
	// Shortcut cheat-sheet modal (? key / footer button).
	let helpOpen = $state(false);
	// Per-line editor revision. Bumped ONLY when a mode toggle re-seeds a script so
	// the inline editor remounts from the fresh server script; autosave reloads
	// must not bump it, or an open editor would reset mid-edit.
	let editorRev = $state({});
	// Per-line busy flag for the アナ→演技 toggle (analysis is ~10s; blocks re-taps).
	let toggleBusy = $state({});

	// --- Vim-style keyboard cursor ---
	// The focused column and the per-column cursor. Distinct from selectedId
	// (which project is open): moving projectCursor does NOT change selection until
	// Enter, per vim. Cursors are tracked by id so they follow across reloads.
	let focusCol = $state('project'); // 'project' | 'line'
	let projectCursor = $state(null); // project id
	let lineCursor = $state(null); // line id
	// Accordion open state, lifted from LineRow so click and keyboard share one
	// source of truth. Keyed by line id (spread-to-update, like editorRev).
	let expandedIds = $state({});
	// Yank register: a single memory slot, cross-project, never consumed by paste.
	let yankRegister = $state(null); // { text, mode, script } | null
	// NotifySubscribe instance, so the global `n` shortcut can call its toggle().
	let notify;

	async function loadProjects() {
		projects = await api.listProjects();
	}

	async function loadSelected() {
		if (selectedId == null) {
			selected = null;
			return;
		}
		selected = await api.getProject(selectedId);
	}

	async function selectProject(id) {
		selectedId = id;
		await loadSelected();
	}

	async function createProject(name) {
		try {
			const p = await api.createProject(name);
			await loadProjects();
			await selectProject(p.id);
		} catch (e) {
			addToast(`作成に失敗しました: ${e.message}`, 'danger');
		}
	}

	async function renameProject(name) {
		try {
			await api.renameProject(selectedId, name);
			await Promise.all([loadProjects(), loadSelected()]);
		} catch (e) {
			addToast(`改名に失敗しました: ${e.message}`, 'danger');
		}
	}

	// Open the project-title editor for the focused project (a = caret end, i = start).
	// Works even when the project is not open (unlike the workspace inline rename).
	function openProjectEdit(caret) {
		const p = (projects ?? []).find((x) => x.id === projectCursor);
		if (!p) return;
		projectEdit = { id: p.id, name: p.name, caret };
	}

	async function commitProjectEdit(name) {
		const target = projectEdit;
		if (!target) return;
		try {
			await api.renameProject(target.id, name);
			await loadProjects();
			// Refresh the open workspace only if the edited project is the open one.
			if (selectedId === target.id) await loadSelected();
		} catch (e) {
			addToast(`改名に失敗しました: ${e.message}`, 'danger');
		}
	}

	function onplay(line) {
		play(line).catch((e) => addToast(`再生に失敗しました: ${e.message}`, 'danger'));
	}

	// Whole-project listen-through (radio). Tapping the active project stops it.
	async function onPlayAll(projectId) {
		if (player.radioProjectId === projectId) {
			stop();
			return;
		}
		try {
			const lines =
				selectedId === projectId && selected
					? selected.lines
					: (await api.getProject(projectId)).lines;
			playAll(projectId, lines);
		} catch (e) {
			addToast(`再生に失敗しました: ${e.message}`, 'danger');
		}
	}

	async function saveLine(line, patch) {
		try {
			await api.updateLine(line.id, patch);
			await loadSelected();
		} catch (e) {
			addToast(`保存に失敗しました: ${e.message}`, 'danger');
		}
	}

	async function reorderLines(order) {
		try {
			await api.reorderLines(selectedId, order);
			await loadSelected();
		} catch (e) {
			addToast(`並び替えに失敗しました: ${e.message}`, 'danger');
		}
	}

	// Re-run /analyze for `text` and persist the fresh script alongside `patch`.
	// Guards against concurrent taps (toggleBusy) and remounts the inline editor
	// from the new script (editorRev). Shared by the mode toggle and text edits.
	async function analyzeAndSave(line, text, patch) {
		if (toggleBusy[line.id]) return; // analysis already running
		toggleBusy = { ...toggleBusy, [line.id]: true };
		try {
			const script = await api.analyzeLine(text);
			await api.updateLine(line.id, { ...patch, script });
			await loadSelected();
			editorRev = { ...editorRev, [line.id]: (editorRev[line.id] ?? 0) + 1 };
		} catch (e) {
			addToast(`分析に失敗しました: ${e.message}`, 'danger');
		} finally {
			toggleBusy = { ...toggleBusy, [line.id]: false };
		}
	}

	// Mode toggle (the badge). アナ→演技 runs /analyze inline; 演技→アナ is
	// destructive (discards hand-tuned JSON) and routes through ConfirmModal.
	async function toggleMode(line) {
		if (line.mode === 'acting') {
			confirm = { kind: 'to-announcer', target: line, busy: false };
			return;
		}
		await analyzeAndSave(line, line.text, { mode: 'acting' });
	}

	// --- Confirm-guarded actions ---
	function requestDeleteProject(project) {
		confirm = { kind: 'delete-project', target: project, busy: false };
	}
	function requestDeleteLine(line) {
		confirm = { kind: 'delete-line', target: line, busy: false };
	}

	async function runConfirm() {
		if (!confirm) return;
		confirm = { ...confirm, busy: true };
		const { kind, target } = confirm;
		try {
			if (kind === 'delete-project') {
				await api.deleteProject(target.id);
				if (selectedId === target.id) {
					selectedId = null;
					selected = null;
				}
				await loadProjects();
			} else if (kind === 'delete-line') {
				await api.deleteLine(target.id);
				await loadSelected();
			} else if (kind === 'to-announcer') {
				await api.updateLine(target.id, { mode: 'announcer', script: null });
				await loadSelected();
			}
		} catch (e) {
			addToast(`失敗しました: ${e.message}`, 'danger');
		} finally {
			confirm = null;
		}
	}

	const confirmText = {
		'delete-project': {
			title: 'プロジェクトを削除',
			message: 'このプロジェクトと全ての行を削除します。元に戻せません。',
			confirmLabel: '削除',
			danger: true
		},
		'delete-line': {
			title: '行を削除',
			message: 'この行を削除します。元に戻せません。',
			confirmLabel: '削除',
			danger: true
		},
		'to-announcer': {
			title: '演技を解除',
			message: '手調整した感情パラメータを破棄してアナウンサーに戻します。元に戻せません。',
			confirmLabel: '解除',
			danger: true
		}
	};

	// Pour-in from the footer: append at the end (no afterLineId).
	function openPourIn() {
		pourInAfter = null;
		pourIn = true;
	}

	async function onPourInDone() {
		await Promise.all([loadSelected(), loadProjects()]);
	}

	// --- Line context menu (right-click / long-press) and its actions ---
	function openLineMenu(line) {
		menuLine = line;
	}

	// 台本追加 (menu): open pour-in inserting directly after this line.
	function menuAddAfter(line) {
		pourInAfter = line.id;
		pourIn = true;
	}

	async function commitTextEdit(text) {
		const line = textEditLine;
		if (!line) return;
		// Editing an acting line's text invalidates its analysis: re-run /analyze so
		// text and script never drift. Applies to every text-edit path (menu + a/i),
		// not just the shortcuts.
		// TODO(docs/DESIGN.md, unfreeze): document this keyboard/edit re-analyze.
		if (line.mode === 'acting') await analyzeAndSave(line, text, { text });
		else await saveLine(line, { text });
	}

	async function duplicateLine(line) {
		try {
			await api.duplicateLine(line.id);
			await loadSelected();
		} catch (e) {
			addToast(`複製に失敗しました: ${e.message}`, 'danger');
		}
	}

	// --- Keyboard cursor: helpers and actions ---

	// Keep cursors valid as the underlying lists change (reload / delete / paste):
	// stay on the same id if it survives, otherwise snap to the first item.
	$effect(() => {
		const list = projects ?? [];
		if (list.length === 0) projectCursor = null;
		else if (!list.some((p) => p.id === projectCursor)) projectCursor = list[0].id;
	});
	$effect(() => {
		const list = selected?.lines ?? [];
		if (list.length === 0) lineCursor = null;
		else if (!list.some((l) => l.id === lineCursor)) lineCursor = list[0].id;
	});

	function focusedLine() {
		return (selected?.lines ?? []).find((l) => l.id === lineCursor) ?? null;
	}

	// Bring the active column's cursor into view. block:'nearest', immediate (no
	// smooth scroll) per Sumi's motion discipline; the idle column is not scrolled.
	function scrollCursorIntoView() {
		requestAnimationFrame(() => {
			const sel =
				focusCol === 'project' ? '.list-pane .card.focused' : '.detail-pane .row.focused';
			document.querySelector(sel)?.scrollIntoView({ block: 'nearest' });
		});
	}

	// Single source of truth for accordion expand (click + Enter/Space share it).
	// Announcer rows never expand.
	function toggleExpandLine(line) {
		if (line.mode !== 'acting') return;
		expandedIds = { ...expandedIds, [line.id]: !expandedIds[line.id] };
	}

	function moveCursor(dir) {
		const inProject = focusCol === 'project';
		const list = (inProject ? projects : selected?.lines) ?? [];
		const idx = list.findIndex((x) => x.id === (inProject ? projectCursor : lineCursor));
		const next = idx + dir;
		if (idx === -1 || next < 0 || next >= list.length) return;
		if (inProject) projectCursor = list[next].id;
		else lineCursor = list[next].id;
		scrollCursorIntoView();
	}

	function activateCursor() {
		if (focusCol === 'project') {
			if (projectCursor != null) selectProject(projectCursor);
		} else {
			const line = focusedLine();
			if (line) toggleExpandLine(line);
		}
	}

	// J / K: swap the focused line with its neighbor via the existing reorder PUT.
	// The cursor follows because it is tracked by id (unchanged across the reload).
	async function moveFocusedLine(dir) {
		const list = selected?.lines ?? [];
		const idx = list.findIndex((l) => l.id === lineCursor);
		const swap = idx + dir;
		if (idx === -1 || swap < 0 || swap >= list.length) return;
		const ids = list.map((l) => l.id);
		[ids[idx], ids[swap]] = [ids[swap], ids[idx]];
		await reorderLines(ids);
		scrollCursorIntoView();
	}

	function yank(line) {
		yankRegister = { text: line.text, mode: line.mode, script: line.script ?? null };
	}

	function yankFocusedLine() {
		const line = focusedLine();
		if (line) yank(line);
	}

	// dd: delete without a ConfirmModal (keyboard modality only — the yank register
	// is the undo). Menu/button delete keeps its confirmation. Cursor follows to the
	// next line (or previous if last).
	async function deleteFocusedLine() {
		const list = selected?.lines ?? [];
		const idx = list.findIndex((l) => l.id === lineCursor);
		if (idx === -1) return;
		const line = list[idx];
		yank(line);
		const nextId = list[idx + 1]?.id ?? list[idx - 1]?.id ?? null;
		try {
			await api.deleteLine(line.id);
			lineCursor = nextId;
			await loadSelected();
			scrollCursorIntoView();
		} catch (e) {
			addToast(`削除に失敗しました: ${e.message}`, 'danger');
		}
	}

	// Insert a new line (addLine appends, then reorder to the target slot) relative
	// to the cursor. Shared by p/P (paste) and o/O (new). Cursor follows the new line.
	async function insertLineAt(payload, pos) {
		try {
			const created = await api.addLine(selectedId, payload);
			const ids = (selected?.lines ?? []).map((l) => l.id).filter((id) => id !== created.id);
			const anchor = ids.indexOf(lineCursor);
			const at = anchor === -1 ? ids.length : pos === 'above' ? anchor : anchor + 1;
			ids.splice(at, 0, created.id);
			await api.reorderLines(selectedId, ids);
			await loadSelected();
			lineCursor = created.id;
			scrollCursorIntoView();
		} catch (e) {
			addToast(`追加に失敗しました: ${e.message}`, 'danger');
		}
	}

	async function pasteYank(pos) {
		if (!yankRegister) return; // register empty: no-op
		const payload = { mode: yankRegister.mode, text: yankRegister.text };
		if (yankRegister.mode === 'acting' && yankRegister.script) payload.script = yankRegister.script;
		await insertLineAt(payload, pos);
	}

	function openNewLine(pos) {
		newLine = { pos };
	}

	async function commitNewLine(text) {
		const pos = newLine?.pos ?? 'below';
		await insertLineAt({ mode: 'announcer', text }, pos);
	}

	function openTextEdit(caret) {
		const line = focusedLine();
		if (!line) return;
		textEditCaret = caret;
		textEditLine = line;
	}

	function toggleFocusedMode() {
		const line = focusedLine();
		if (line) toggleMode(line);
	}

	// Two-key sequences (yy / dd): remember the last key + timestamp; a matching
	// second key within 500ms fires. A single y / d does nothing.
	let lastKey = null;
	let lastKeyAt = 0;

	function editableFocused() {
		const el = document.activeElement;
		if (!el) return false;
		const tag = el.tagName;
		return tag === 'INPUT' || tag === 'TEXTAREA' || el.isContentEditable;
	}

	function anyModalOpen() {
		return !!(
			pourIn ||
			confirm ||
			menuLine ||
			textEditLine ||
			newLine ||
			projectEdit ||
			helpOpen ||
			readingOpen
		);
	}

	function onWindowKeydown(e) {
		// Global shortcuts only on the 台本 tab, never while typing, inside a modal,
		// mid-IME-composition, or with a modifier held (leave those to the browser).
		if (router.tab !== 'script') return;
		if (e.isComposing) return;
		if (editableFocused()) return;
		if (anyModalOpen()) return;
		if (e.metaKey || e.ctrlKey || e.altKey) return;

		const k = e.key;
		const now = Date.now();
		const prev = lastKey && now - lastKeyAt < 500 ? lastKey : null;
		lastKey = null; // consumed unless we set a pending first key below

		// yy / dd (台本 column only).
		if (focusCol === 'line' && (k === 'y' || k === 'd')) {
			e.preventDefault();
			if (prev === k) {
				if (k === 'y') yankFocusedLine();
				else deleteFocusedLine();
			} else {
				lastKey = k;
				lastKeyAt = now;
			}
			return;
		}

		// Cross-column keys.
		switch (k) {
			case 'h':
				e.preventDefault();
				focusCol = 'project';
				scrollCursorIntoView();
				return;
			case 'l':
				e.preventDefault();
				// Boundary no-op: never move focus into an empty 台本 column (no project
				// open, or a project with no lines) — the cursor ring would vanish with
				// nothing to land on. Stay in the プロジェクト column so focus stays visible.
				if (!selected?.lines?.length) return;
				focusCol = 'line';
				scrollCursorIntoView();
				return;
			case 'j':
				e.preventDefault();
				moveCursor(1);
				return;
			case 'k':
				e.preventDefault();
				moveCursor(-1);
				return;
			case 'Enter':
				// Enter is accordion/open only. プロジェクト列 = open (selectProject),
				// 台本列 = toggle the accordion.
				e.preventDefault();
				activateCursor();
				return;
			case ' ': {
				// Space plays/stops the focused target via the exact same functions the
				// buttons call — so it shares their visual state and toggle logic.
				// preventDefault keeps the page from scrolling.
				e.preventDefault();
				if (focusCol === 'project') {
					if (projectCursor != null) onPlayAll(projectCursor);
				} else {
					const line = focusedLine();
					if (line) onplay(line);
				}
				return;
			}
			case '?':
				// Shortcut cheat-sheet. Works in either column (like n). The modal guard
				// makes a re-press a no-op; Esc / the × closes it.
				e.preventDefault();
				helpOpen = true;
				return;
			case 'n':
				// Toggle the header notify megaphone from either column. Synchronous
				// call preserves subscribe()'s Audio autoplay unlock (user gesture).
				e.preventDefault();
				notify?.toggle();
				return;
		}

		// プロジェクト列での a / i: フォーカス中プロジェクトのタイトル編集
		// (末尾 / 先頭キャレット)。開いていないプロジェクトも対象にできる。
		if (focusCol === 'project' && (k === 'a' || k === 'i')) {
			e.preventDefault();
			openProjectEdit(k === 'a' ? 'end' : 'start');
			return;
		}

		// 台本-column-only keys.
		if (focusCol !== 'line') return;
		switch (k) {
			case 'J':
				e.preventDefault();
				moveFocusedLine(1);
				break;
			case 'K':
				e.preventDefault();
				moveFocusedLine(-1);
				break;
			case 'p':
				e.preventDefault();
				pasteYank('below');
				break;
			case 'P':
				e.preventDefault();
				pasteYank('above');
				break;
			case 'o':
				e.preventDefault();
				openNewLine('below');
				break;
			case 'O':
				e.preventDefault();
				openNewLine('above');
				break;
			case 'a':
				e.preventDefault();
				openTextEdit('end');
				break;
			case 'i':
				e.preventDefault();
				openTextEdit('start');
				break;
			case 'm':
				e.preventDefault();
				toggleFocusedMode();
				break;
		}
	}

	$effect(() => {
		loadProjects();
	});
</script>

<svelte:window onkeydown={onWindowKeydown} />

<div class="app">
	<header class="app-header">
		<span class="site-title">宮舞モカ 台本工房</span>
		<nav class="tabs">
			<button type="button" class:active={router.tab === 'script'} onclick={() => navigate('script')}>
				台本
			</button>
			<button type="button" class:active={router.tab === 'dict'} onclick={() => navigate('dict')}>
				辞書
			</button>
			<button type="button" class:active={router.tab === 'work'} onclick={() => navigate('work')}>
				作業
			</button>
		</nav>
		<NotifySubscribe bind:this={notify} />
	</header>

	{#if router.tab === 'script'}
		<div class="layout" data-active-col={focusCol}>
			<aside class="list-pane">
				<ProjectList
					{projects}
					{selectedId}
					focusedId={focusCol === 'project' ? projectCursor : null}
					radioProjectId={player.radioProjectId}
					onselect={selectProject}
					oncreate={createProject}
					onplayall={onPlayAll}
					onrequestDelete={requestDeleteProject}
				/>
			</aside>
			<main class="detail-pane">
				{#if selected}
					<LineWorkspace
						project={selected}
						{editorRev}
						{toggleBusy}
						{expandedIds}
						lineCursor={focusCol === 'line' ? lineCursor : null}
						onrename={renameProject}
						{onplay}
						onsave={saveLine}
						onreorder={reorderLines}
						ontoggle={toggleMode}
						onrequestDelete={requestDeleteLine}
						onmenu={openLineMenu}
						onToggleExpand={toggleExpandLine}
						onpourin={openPourIn}
						onhelp={() => (helpOpen = true)}
						oncheck={() => (readingOpen = true)}
					/>
				{:else}
					<div class="placeholder">プロジェクトを選択してください。</div>
				{/if}
			</main>
		</div>
	{:else if router.tab === 'dict'}
		<main class="dict-view">
			<DictionaryView />
		</main>
	{:else}
		<main class="work-view">
			<WorkView />
		</main>
	{/if}
</div>

{#if pourIn && selected}
	<PourInModal
		projectId={selected.id}
		afterLineId={pourInAfter}
		onclose={() => {
			pourIn = false;
			pourInAfter = null;
		}}
		ondone={onPourInDone}
	/>
{/if}

{#if menuLine}
	{@const line = menuLine}
	<LineContextMenu
		onclose={() => (menuLine = null)}
		onadd={() => menuAddAfter(line)}
		onedit={() => {
			textEditCaret = undefined;
			textEditLine = line;
		}}
		onduplicate={() => duplicateLine(line)}
	/>
{/if}

{#if textEditLine}
	<TextEditModal
		text={textEditLine.text}
		caret={textEditCaret}
		acting={textEditLine.mode === 'acting'}
		onclose={() => (textEditLine = null)}
		oncommit={commitTextEdit}
	/>
{/if}

{#if newLine}
	<NewLineModal onclose={() => (newLine = null)} oncommit={commitNewLine} />
{/if}

{#if projectEdit}
	<TextEditModal
		text={projectEdit.name}
		caret={projectEdit.caret}
		title="プロジェクト名を編集"
		label="プロジェクト名"
		onclose={() => (projectEdit = null)}
		oncommit={commitProjectEdit}
	/>
{/if}

{#if readingOpen && selected}
	<ReadingModal
		projectId={selected.id}
		{onplay}
		onclose={() => {
			readingOpen = false;
			loadSelected();
		}}
	/>
{/if}

{#if helpOpen}
	<HelpModal onclose={() => (helpOpen = false)} />
{/if}

{#if confirm}
	{@const t = confirmText[confirm.kind]}
	<ConfirmModal
		title={t.title}
		message={t.message}
		confirmLabel={t.confirmLabel}
		danger={t.danger}
		busy={confirm.busy}
		onconfirm={runConfirm}
		oncancel={() => (confirm = null)}
	/>
{/if}

<Toast />

<style lang="sass">
.app
	display: grid
	grid-template-rows: auto 1fr
	height: 100dvh

// A thin band with the site title and the app's only navigation: the 台本 / 辞書
// tabs. No actions, no shadow. Sumi = surface-raised; Kinari overrides the wash
// token to a faint mocha so the workspace opens on warmth (see docs/DESIGN.md).
.app-header
	display: flex
	align-items: center
	gap: var(--sp-5)
	padding: var(--sp-2) var(--sp-4)
	background: var(--c-wash-raised)
	border-bottom: 1px solid var(--c-border)

.site-title
	font-size: var(--fs-md)
	font-weight: 500
	color: var(--c-text-muted)

// Sumi tab recipe: label-type, muted when inactive; active adds on-surface text
// and a 2px accent underline — no background change.
.tabs
	display: flex
	gap: var(--sp-4)

	button
		padding: var(--sp-1) 0
		background: transparent
		border: none
		border-bottom: 2px solid transparent
		color: var(--c-text-sub)
		font-family: inherit
		font-size: var(--fs-sm)
		cursor: pointer

		&:hover
			color: var(--c-text)

		&.active
			color: var(--c-text)
			border-bottom-color: var(--c-accent)

.layout
	display: grid
	grid-template-columns: 1fr
	min-height: 0

	@media (min-width: 768px)
		grid-template-columns: 20rem 1fr

.list-pane
	overflow: auto
	border-right: 1px solid var(--c-border)

	@media (max-width: 767px)
		border-right: none
		border-bottom: 1px solid var(--c-border)

.detail-pane
	min-height: 0
	overflow: hidden

.dict-view,
.work-view
	min-height: 0
	overflow: hidden

.placeholder
	display: flex
	align-items: center
	justify-content: center
	height: 100%
	color: var(--c-text-muted)
	font-size: var(--fs-sm)
</style>
