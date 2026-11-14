<script>
	import Modal from '$lib/components/Modal.svelte';
	import Icon from '$lib/components/Icon.svelte';
	import Spinner from '$lib/components/Spinner.svelte';
	import { checkReadings, dismissReading } from '$lib/api.js';
	import { addToast } from '$lib/toast.svelte.js';
	import { player } from '$lib/player.svelte.js';

	// 読みの確認. Opening it runs the check for the project: the server compares
	// the AI's reading of each line with what the synthesized audio actually
	// says, respells misread words, and reports line by line. Lines already
	// checked come back instantly, so reopening just shows the stored results.
	// Lines needing attention are listed; correctly read lines fold into a count.
	let { projectId, onplay, onclose } = $props();

	let running = $state(false);
	let progress = $state(null); // { index, total }
	let refused = $state(null); // why the check could not start
	let results = $state([]); // [{ status, error, line }] in narration order

	let controller = null;

	const LABELS = {
		fixed: '直した',
		kept: 'そのまま',
		unresolved: '直せず',
		dismissed: '外した',
		failed: '確認できず'
	};

	// A line's state: a failed check outranks whatever was stored before.
	const stateOf = (r) => (r.status === 'failed' ? 'failed' : (r.line.reading?.status ?? 'ok'));
	const notable = $derived(results.filter((r) => stateOf(r) !== 'ok'));
	const fine = $derived(results.filter((r) => stateOf(r) === 'ok'));
	const segments = (r) => r.line.reading?.segments ?? [];

	async function run(force) {
		controller?.abort();
		controller = new AbortController();
		running = true;
		refused = null;
		progress = null;
		results = [];
		try {
			await checkReadings(projectId, {
				force,
				signal: controller.signal,
				onEvent: (e) => {
					progress = { index: e.index, total: e.total };
					results.push({ status: e.status, error: e.error, line: e.line });
				}
			});
		} catch (e) {
			if (e.name !== 'AbortError') refused = e.message;
		} finally {
			running = false;
		}
	}

	async function dismiss(result) {
		try {
			result.line = await dismissReading(result.line.id);
		} catch (e) {
			addToast(`直しを外せませんでした: ${e.message}`, 'danger');
		}
	}

	function close() {
		controller?.abort();
		onclose?.();
	}

	function onKeydown(e) {
		if (e.key === 'Escape') {
			e.preventDefault();
			close();
		}
	}

	run(false);
</script>

<svelte:window onkeydown={onKeydown} />

<Modal title="読みの確認" onclose={close} maxWidth="40rem" maxHeight="85dvh">
	{#if running}
		<p class="progress" role="status">
			<Spinner size="1rem" />
			{#if progress}{progress.index}/{progress.total} 行を確認中…{:else}確認を始めています…{/if}
		</p>
	{/if}

	{#if refused}
		<p class="refused" role="alert">確認を始められません: {refused}</p>
	{/if}

	{#if notable.length > 0}
		<ul class="results">
			{#each notable as result (result.line.id)}
				{@const state = stateOf(result)}
				<li class="result">
					<div class="top">
						<span class="state" class:fixed={state === 'fixed'} class:failed={state === 'failed'}>
							{LABELS[state]}
						</span>
						<span class="text">{result.line.text}</span>
						<button
							type="button"
							class="icon-btn"
							aria-label={player.playingId === result.line.id ? '停止' : '再生'}
							onclick={() => onplay?.(result.line)}
						>
							<Icon name={player.playingId === result.line.id ? 'stop' : 'play'} />
						</button>
						{#if state === 'fixed'}
							<button
								type="button"
								class="icon-btn"
								aria-label="直しを外す"
								title="直しを外す"
								onclick={() => dismiss(result)}
							>
								<Icon name="undo" />
							</button>
						{/if}
					</div>

					{#if state === 'failed'}
						<details>
							<summary>理由</summary>
							<p class="reason">{result.error}</p>
						</details>
					{:else}
						{#each segments(result) as seg}
							{#if seg.words.length > 0}
								<dl class="words">
									{#each seg.words as word}
										<div>
											<dt>{word.surface}</dt>
											<dd>聞き取り <b>{word.heard}</b></dd>
											<dd>AI <b>{word.expected}</b></dd>
										</div>
									{/each}
								</dl>
								{#if seg.spoken}
									<p class="spoken">読み上げ: {seg.spoken}</p>
								{/if}
								<details>
									<summary>全文の読み</summary>
									<p class="reason">聞き取り: {seg.heard}</p>
									<p class="reason">AI: {seg.expected}</p>
								</details>
							{/if}
						{/each}
					{/if}
				</li>
			{/each}
		</ul>
	{/if}

	{#if fine.length > 0}
		<details class="fine">
			<summary>読み違いなし {fine.length} 行</summary>
			<ul>
				{#each fine as result (result.line.id)}
					<li>
						<span class="text">{result.line.text}</span>
						{#each segments(result) as seg}
							<span class="reason">{seg.expected}</span>
						{/each}
					</li>
				{/each}
			</ul>
		</details>
	{/if}

	<div class="actions">
		<button type="button" class="quiet" disabled={running} onclick={() => run(true)}>
			<Icon name="refresh" /> 確かめ直す
		</button>
		<button type="button" class="quiet" onclick={close}>閉じる</button>
	</div>
</Modal>

<style lang="sass">
.progress
	display: flex
	align-items: center
	gap: var(--sp-2)
	margin: 0 0 var(--sp-3)
	font-size: var(--fs-sm)
	color: var(--c-text-sub)

.refused
	margin: 0 0 var(--sp-3)
	padding: var(--sp-2) var(--sp-3)
	border: 1px solid var(--c-danger-border)
	border-radius: var(--radius-sm)
	background: var(--c-danger-bg)
	color: var(--c-danger)
	font-size: var(--fs-sm)
	overflow-wrap: anywhere

.results
	display: flex
	flex-direction: column
	gap: var(--sp-2)
	margin: 0
	padding: 0
	list-style: none

.result
	padding: var(--sp-2) var(--sp-3)
	border: 1px solid var(--c-border)
	border-radius: var(--radius-md)
	background: var(--c-bg)

.top
	display: flex
	align-items: center
	gap: var(--sp-2)

	.text
		flex: 1
		min-width: 0
		overflow-wrap: anywhere

// State label: caption type, neutral outline. 「直した」 is the one state that
// changed how the line sounds, so it alone takes the accent text color.
.state
	flex: none
	padding: 1px var(--sp-2)
	border: 1px solid var(--c-border)
	border-radius: var(--radius-sm)
	color: var(--c-text-sub)
	font-size: var(--fs-xs)
	white-space: nowrap

	&.fixed
		border-color: var(--c-accent)
		color: var(--c-accent)

	&.failed
		border-color: var(--c-danger-border)
		color: var(--c-danger)

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
		color: var(--c-text)

.words
	margin: var(--sp-1) 0 0
	font-size: var(--fs-sm)

	div
		display: flex
		flex-wrap: wrap
		align-items: baseline
		gap: var(--sp-1) var(--sp-3)

	dt
		font-weight: 600

	dd
		margin: 0
		color: var(--c-text-sub)

	b
		color: var(--c-text)
		font-weight: 600

.spoken
	margin: var(--sp-1) 0 0
	font-size: var(--fs-sm)
	overflow-wrap: anywhere

details
	margin-top: var(--sp-1)
	font-size: var(--fs-xs)
	color: var(--c-text-muted)

	summary
		cursor: pointer

.reason
	display: block
	margin: var(--sp-1) 0 0
	color: var(--c-text-sub)
	font-size: var(--fs-xs)
	overflow-wrap: anywhere

.fine
	margin-top: var(--sp-3)
	font-size: var(--fs-sm)

	ul
		margin: var(--sp-2) 0 0
		padding: 0
		list-style: none

	li
		padding: var(--sp-1) 0
		border-top: 1px solid var(--c-border)

	.text
		color: var(--c-text)

.actions
	display: flex
	justify-content: flex-end
	gap: var(--sp-2)
	margin-top: var(--sp-4)

	button
		display: inline-flex
		align-items: center
		gap: var(--sp-1)
		padding: var(--sp-2) var(--sp-4)
		font-size: var(--fs-md)
		border-radius: var(--radius-sm)
		background: transparent
		border: 1px solid var(--c-border)
		color: var(--c-text-sub)
		cursor: pointer

		&:disabled
			opacity: 0.5
			cursor: not-allowed
</style>
