import { sayTextUrl, sayScriptUrl, sayScriptRequest } from '$lib/api.js';

// Single global audio player. Two ways to start: a single line (play) or a
// whole-project listen-through (playAll, "radio"). Exactly one line sounds at a
// time and the two modes are mutually exclusive — starting either stops the
// other (DESIGN.md). Audio is never stored: every play streams a fresh /say
// synthesis. Acting lines POST the script JSON and play the resulting blob;
// announcer lines stream directly via a GET src.
//
// During a radio run we prefetch the *next* line while the current one plays:
// VOICEPEAK is idle during playback, so synthesizing line N+1 ahead of time
// removes the inter-line synthesis gap. Prefetch is always at most one line
// (VOICEPEAK's queue is serial and shared with notifications), fully invisible
// (no loading/spinner/indicator on the prefetched row), and torn down on stop.
let playingId = $state(null);
let loadingId = $state(null);
let spinnerId = $state(null); // set only after 300ms of loading
let radioProjectId = $state(null); // project currently playing through as radio

let queue = null; // ordered lines for the active radio run
let queueIndex = 0;

let audio = null;
let objectUrl = null;
let spinnerTimer = null;
let token = 0; // guards against races when switching lines quickly

// At most one look-ahead synthesis in flight for the radio run.
// { lineId, controller, objectUrl: string|null, ready: Promise }
let prefetch = null;

function ensureAudio() {
	if (audio) return audio;
	audio = new Audio();
	audio.addEventListener('playing', () => {
		loadingId = null;
		clearSpinnerTimer();
		spinnerId = null;
	});
	audio.addEventListener('ended', onEnded);
	audio.addEventListener('error', reset);
	return audio;
}

function clearSpinnerTimer() {
	if (spinnerTimer) {
		clearTimeout(spinnerTimer);
		spinnerTimer = null;
	}
}

function revoke() {
	if (objectUrl) {
		URL.revokeObjectURL(objectUrl);
		objectUrl = null;
	}
}

// Build the /say request for a line and return its audio blob. Encapsulates the
// mode difference (acting POSTs the script, announcer GETs the text URL) so the
// look-ahead path reuses the exact same request logic as normal playback.
async function synthesize(line, signal) {
	const res =
		line.mode === 'acting' && line.script
			? await fetch(sayScriptUrl(line.id), { ...sayScriptRequest(line.script), signal })
			: await fetch(sayTextUrl(line.text, line.raw, line.id), { signal });
	if (!res.ok) throw new Error(`${res.status}`);
	return res.blob();
}

// Abort any in-flight look-ahead synthesis and release its ObjectURL. Safe to
// call repeatedly. The AbortController stops the server synthesis (kill_on_drop
// frees VOICEPEAK's queue); revoke prevents ObjectURL leaks.
function clearPrefetch() {
	if (!prefetch) return;
	prefetch.controller.abort();
	if (prefetch.objectUrl) URL.revokeObjectURL(prefetch.objectUrl);
	prefetch = null;
}

// Kick off look-ahead synthesis for the next line, if any. Always at most one
// line ahead; never re-arms for a line already being prefetched. Deliberately
// invisible: no loadingId/spinnerId, no playing indicator.
function startPrefetch(nextLine) {
	if (!queue || !nextLine) return;
	if (prefetch && prefetch.lineId === nextLine.id) return;
	clearPrefetch();
	const controller = new AbortController();
	const entry = { lineId: nextLine.id, controller, objectUrl: null, ready: null };
	entry.ready = synthesize(nextLine, controller.signal)
		.then((blob) => {
			// Guard against a teardown that raced with resolution.
			if (prefetch === entry) entry.objectUrl = URL.createObjectURL(blob);
		})
		.catch(() => {
			// Aborted or failed: leave objectUrl null so onEnded falls back to startLine.
		});
	prefetch = entry;
}

function reset() {
	playingId = null;
	loadingId = null;
	spinnerId = null;
	clearSpinnerTimer();
	revoke();
	clearPrefetch();
	queue = null;
	queueIndex = 0;
	radioProjectId = null;
}

// During a radio run, advance to the next line instead of resetting; otherwise
// (single line, or last line of the run) stop.
function onEnded() {
	if (queue && queueIndex + 1 < queue.length) {
		queueIndex++;
		const line = queue[queueIndex];
		if (prefetch && prefetch.lineId === line.id) {
			const entry = prefetch;
			// Wait for the in-flight (or already-resolved) synthesis. If it produced
			// a blob, promote it instantly; otherwise fall back to a fresh synthesis.
			// The token snapshot keeps this async continuation from resurrecting
			// playback after stop() or hijacking a manual play() that happened while
			// the synthesis was still in flight (both bump the token).
			const my = token;
			entry.ready
				.then(() => {
					if (my !== token) return;
					if (entry.objectUrl) playPrefetchedLine(line, entry);
					else startLine(line).catch(() => stop());
				})
				.catch(() => {
					if (my !== token) return;
					startLine(line).catch(() => stop());
				});
		} else {
			startLine(line).catch(() => stop());
		}
	} else {
		stop();
	}
}

export function stop() {
	token++;
	if (audio) {
		audio.pause();
		audio.removeAttribute('src');
		audio.load();
	}
	revoke();
	reset();
}

// Play a line whose audio was prefetched: no fetch, no loading/spinner state,
// just swap the blob in and start. Then arm the look-ahead for the line after.
async function playPrefetchedLine(line, entry) {
	const my = ++token;
	const el = ensureAudio();
	// Take ownership of the prefetched ObjectURL as the current playback blob.
	revoke();
	objectUrl = entry.objectUrl;
	entry.objectUrl = null;
	if (prefetch === entry) prefetch = null;
	el.src = objectUrl;
	playingId = line.id;
	try {
		await el.play();
	} catch (e) {
		if (my !== token) return;
		reset();
		return;
	}
	if (my !== token) return;
	startPrefetch(queue[queueIndex + 1]);
}

// Synthesize and play one line. Does not touch the queue/radio state, so it is
// reused both for single play and for each leg of a radio run.
async function startLine(line) {
	const my = ++token;
	const el = ensureAudio();
	loadingId = line.id;
	clearSpinnerTimer();
	spinnerTimer = setTimeout(() => {
		if (loadingId === line.id) spinnerId = line.id;
	}, 300);

	try {
		if (line.mode === 'acting' && line.script) {
			// POST script JSON, play the streamed blob.
			const res = await fetch(sayScriptUrl(line.id), sayScriptRequest(line.script));
			if (my !== token) return; // superseded
			if (!res.ok) throw new Error(`${res.status}`);
			revoke();
			objectUrl = URL.createObjectURL(await res.blob());
			if (my !== token) return;
			el.src = objectUrl;
		} else {
			el.src = sayTextUrl(line.text, line.raw, line.id);
		}
		playingId = line.id;
		await el.play();
	} catch (e) {
		// A superseded attempt's failure (e.g. its play() promise rejecting after
		// stop() yanked the src) must not bubble up and kill the current playback.
		if (my !== token) return;
		reset();
		throw e;
	}
	if (my !== token) return;
	// Radio run: once this line is actually sounding, look ahead one line.
	if (queue) startPrefetch(queue[queueIndex + 1]);
}

export async function play(line) {
	// Toggle off if the same line is already active.
	if (playingId === line.id || loadingId === line.id) {
		stop();
		return;
	}
	stop(); // cancels any single play or radio run in progress
	await startLine(line);
}

// Play a whole project's lines in order like radio. Tapping the same project
// again stops it; starting a run cancels any single play or other run.
export function playAll(projectId, lines) {
	if (radioProjectId === projectId) {
		stop();
		return;
	}
	stop();
	if (!lines || lines.length === 0) return;
	queue = lines;
	queueIndex = 0;
	radioProjectId = projectId;
	startLine(lines[0]).catch(() => stop());
}

export const player = {
	get playingId() {
		return playingId;
	},
	get loadingId() {
		return loadingId;
	},
	get spinnerId() {
		return spinnerId;
	},
	get radioProjectId() {
		return radioProjectId;
	}
};
