---
version: alpha
name: Sumi / moca-server
description: >
  moca-server project overrides for the Sumi design system. The canonical
  templates live at ~/.claude/designs/sumi/DESIGN.md (dark = Sumi) and
  ~/.claude/designs/kinari/DESIGN.md (light = Kinari); this file records
  ONLY what is specific to moca-server (primary + secondary accents +
  emotion data colors + domain components for the script-production
  workspace). CSS custom properties in client/src/global.sass are the
  implementation of these tokens.
colors:
  # --- Project primary accent (mocha brown) ---
  # Unsuffixed = Kinari theme (light), -dark = Sumi theme (dark).
  # Mocha: the tool is named after 宮舞モカ. Deliberately desaturated
  # coffee brown so it never reads as 5ch-viewer's vivid amber
  # (amber = 5ch-viewer, blue = novel-server, red = youtube-sub-feed,
  # violet = comic-server). Kinari value keeps white-on-accent ≥ 4.5:1
  # and reads warm on the cream surface.
  accent: "#6b4632"
  accent-subtle: "rgba(107, 70, 50, 0.12)"
  accent-dark: "#c9a086"
  accent-subtle-dark: "rgba(201, 160, 134, 0.15)"
  # accent-strong-dark: the Sumi primary-button surface. The pale mocha
  # accent-dark (needed for underlines/focus on dark surfaces) fails
  # white-on-accent contrast (~2:1), so filled primary buttons in Sumi
  # use this deepened mocha instead (white text ≥ 5.5:1). Kinari primary
  # buttons keep using accent unchanged.
  accent-strong-dark: "#8a5a3c"
  # --- Project secondary accent (emerald green) ---
  # 宮舞モカのハイライトカラー。公式イラストの髪色から取ったエメラルド。
  # Role: "alive / subscribed / on-air" — a persistent state that runs
  # in the background of a screen while the user does something else
  # (currently the notification-subscribe toggle). Distinct from primary
  # (foreground of intent). Doyaru's emotion color is unified with this
  # hue (see below): モカちゃんの落ち着いた透き通った声 = ドヤ = 彼女の色。
  secondary: "#2a9d6e"
  secondary-subtle: "rgba(42, 157, 110, 0.12)"
  secondary-dark: "#4dd6a1"
  secondary-subtle-dark: "rgba(77, 214, 161, 0.15)"
  # --- Functional data colors: the five emotion axes ---
  # One hue per VOICEPEAK emotion axis. Kinari values are warm inks that
  # stay AA as chip text on surface-raised while letting hue read as a
  # first-class cue (Kinari license); Sumi values are muted pastels
  # readable on surface-raised-dark.
  emo-bosoboso: "#4a5248"
  emo-bosoboso-dark: "rgba(150, 168, 152, 0.85)"
  # doyaru is intentionally the same hue as secondary — see Colors below.
  emo-doyaru: "#2a9d6e"
  emo-doyaru-dark: "rgba(77, 214, 161, 0.85)"
  emo-honwaka: "#8a5a20"
  emo-honwaka-dark: "rgba(240, 190, 120, 0.85)"
  emo-angry: "#8f2222"
  emo-angry-dark: "rgba(235, 120, 110, 0.85)"
  emo-teary: "#1f4e8c"
  emo-teary-dark: "rgba(120, 170, 235, 0.85)"
---

# moca-server — Sumi Project Overrides

## Overview

**This project follows the Sumi design system, pairing Sumi (dark) with
Kinari (light).** The canonical templates are
`~/.claude/designs/sumi/DESIGN.md` (all shared rules: neutral chrome,
accent rules, scales, flat elevation, iconography, component recipes) and
`~/.claude/designs/kinari/DESIGN.md` (the warm screen-first light theme:
cream surfaces, sepia ink, accent-sprinkle license). Shared rules are NOT
restated here. This document records only what is unique to moca-server.
On chrome questions the templates win; on the domain semantics below this
file wins.

moca-server does **not** use Washi: this is an audio tool — nobody opens
it on an e-paper device — so its light theme optimizes for warmth and
charm on ordinary screens, not for e-ink contrast survival.

moca-server is a **script-production workspace** for the 宮舞モカ voice:
projects hold ordered lines (台本行), each line is either plain narration
or an emotion-annotated script, and the produced artifact is the script
JSON — not the audio. The user's loop is: pour text in → listen → nudge
emotion → listen again. The UI must make that loop one-tap tight.

Primary accent: **mocha brown** (`#6b4632` Kinari / `#c9a086` Sumi). A
quiet, coffee-toned brown fitting the calm narration voice this tool
exists for. It marks interactive chrome only: active tabs, the one
primary button per screen, focused inputs, the shared focus ring, and
the currently-playing line indicator.

Secondary accent: **emerald green** (`#2a9d6e` Kinari / `#4dd6a1` Sumi).
Taken from 宮舞モカ's own hair color — her hallmark hue. Its role is
strictly *persistent "alive" state* that runs in the background while
the user does something else: currently, the notification-subscribe
toggle in the header (ON = 購読中). Primary owns "the next action";
secondary owns "this switch is still on". They must never fight for the
same region — a screen may show both because they mean different things.

Themes follow the family's Sumi-first convention: `:root` in
`client/src/global.sass` IS the Sumi (dark) theme, and Kinari (light,
warm) is applied via `@media (prefers-color-scheme: light)` — the OS
decides; there is no in-app toggle and no `data-theme` attribute.

**Kinari sprinkles in this app** (where the accent-decoration license is
spent — deliberately few, all light-theme only, none carrying meaning
that text/shape doesn't already carry):

- **App header band:** a faint mocha wash (`accent-subtle`) instead of
  plain surface-raised, so the workspace opens on warmth instead of
  blank paper.
- **Hover / active-row washes:** list rows and buttons may hover with
  `accent-subtle` (or `secondary-subtle` on the megaphone) rather than
  the neutral border swap.
- **Pane headers & sticky footer:** may carry the same faint wash as the
  header band to frame the panes as one warm desk.
- **Emotion chips:** keep their axis-tinted backgrounds — on cream these
  tints read as the "sprinkled color" the theme wants; no change needed.
- **Focus ring:** verify the mocha ring ≥ 3:1 on cream; darken ring
  alpha from 60% if measurement fails (Kinari template rule).

## Domain model (declarative)

These statements define the ideal state; screens and components exist to
express them.

- **The script JSON is the master data; audio is a derived, disposable
  artifact.** Synthesis is deterministic (same JSON → same voice), so
  audio is never persisted — not in the DB, not on disk. Every playback
  is synthesized on demand and streamed. Storage holds text and JSON
  only; it must stay small no matter how many lines exist.
- **A project is an ordered list of lines.** A line is the atomic unit of
  listening, editing, and (future) export. Line order is the narration
  order.
- **Every line is in exactly one of two modes**, chosen at pour-in and
  **togglable per line afterward** (the mode badge is the toggle):
  - **アナウンサー (announcer):** plain text, zero emotion parameters,
    synthesized via the text path. The unadorned 宮舞モカ voice is a
    feature, not a fallback — this mode is the default.
  - **演技 (acting):** the line carries a script JSON (segments with
    emotion / speed / pitch / pause) produced once by /analyze and then
    owned by the user. After analysis, the JSON is hand-tuned via the
    editor; re-analysis never runs implicitly (it would overwrite the
    user's direction).
  - Toggling アナ→演技 runs /analyze for that line (explicit user tap =
    allowed; show the analysis latency honestly). Toggling 演技→アナ
    discards the hand-tuned script JSON and is guarded by ConfirmModal.
- **Whole-project listen-through is a first-class review tool.** Reading
  errors (e.g. "Hard" spelled out letter by letter) are only caught by
  ear, so a project plays end to end like radio: lines play sequentially
  in narration order, the playing indicator walks down the rows, and one
  tap stops it.
- **Emotion axes are a fixed five:** bosoboso, doyaru, honwaka, angry,
  teary (0–100 each), plus speed / pitch / pause. The editor exposes
  exactly these — no abstraction on top, no hidden parameters.
- **読み替え辞書 (reading dictionary):** a global table of surface →
  reading pairs applied to text **at synthesis time only**. The master
  text — line text and script JSON — never changes, and the UI always
  displays the original spelling; the dictionary rewrites sound, not
  text. Entries apply longest-surface-first; ASCII surfaces match
  case-insensitively. It applies to アナウンサー text and to every 演技
  segment text; /analyze always reads the original.
- **A misreading has two fixes:** words that are wrong everywhere
  (foreign words, proper nouns) go in the dictionary; readings that
  depend on context (一日 = いちにち / ついたち) are fixed per line by a
  **reading-only respelling** that never touches the shown text or the
  script JSON. The reading check finds context-dependent misreadings by
  comparing an AI's reading with what the synthesized audio actually
  says, and applies the respelling itself (user decision, task
  bde3a31e, 2026-11-15 — this replaces the earlier "never pretends to
  automate the choice" stance). Because the AI can be wrong, every
  automatic fix stays visible and reversible: the row carries a quiet
  mark, and the check lists what was heard, what the AI read, and the
  respelled text, with a one-tap way to drop the fix.
- **Accent / intonation control is explicitly out of scope.** VOICEPEAK
  decides accent from the (rewritten) text; occasional odd intonation is
  accepted as character, not fought with tooling.
- **ZIP / bulk audio export is deliberately out of scope** until storage
  economics change. Nothing in the UI may promise batch download.

## Colors

The five emotion axes are the project's functional data colors. Each axis
owns one hue, monosemous across the app:

- **bosoboso — moss gray** (`#4a5248` / `rgba(150,168,152,0.85)`): 陰気
- **doyaru — emerald** (`#2a9d6e` / `rgba(77,214,161,0.85)`): ドヤ
- **honwaka — warm peach** (`#8a5a20` / `rgba(240,190,120,0.85)`): ほんわか
- **angry — red** (`#8f2222` / `rgba(235,120,110,0.85)`): 怒り
- **teary — blue** (`#1f4e8c` / `rgba(120,170,235,0.85)`): 涙声

**doyaru's hue is intentionally the same emerald green as the project's
secondary accent** — モカちゃんの落ち着いた透き通った声 = 彼女のドヤ =
彼女自身の色。This is a deliberate three-way unification (character
identity ⇔ signature emotion ⇔ persistent-alive state), not a token
collision. The single hue appears in three functionally distinct
contexts: the secondary token colors chrome (subscribe toggle ON),
`emo-doyaru` colors data (doyaru slider fill + chip). The template's
"emotion colors never color chrome" rule still holds — chrome uses the
`secondary` token, data uses the `emo-doyaru` token. They just happen
to resolve to the same color.

An emotion color appears in exactly two places: the fill of that axis's
slider, and that axis's value chip on a line row. Emotion colors never
color chrome, never mix into gradients, and never indicate state other
than "this axis has this value". Speed / pitch / pause are scalar
adjustments, not emotions — they stay in neutral chrome.

アナウンサー lines are **colorless by definition**: no emotion chips, no
data color anywhere on the row. The visual quietness of an announcer row
IS the mode indicator, reinforced by the mode toggle badge (アナ / 演技)
in caption type — a real button (see Components), but visually as quiet
as an outline badge.

**Primary buttons in Sumi** use `accent-strong-dark` as their surface
with white text; `accent-dark` (the pale mocha) remains the color of
active-tab underlines, focus rings, and accent text, where it must stand
out against dark surfaces. One accent identity, two luminances, each
where its contrast works.

**Emotion colors in Kinari** keep the same ink values as before but are
freed from the Washi darkness-ramp rule: hue is a first-class cue on
LCD. They must stay AA as chip text against surface-raised (`#fffdf8`);
if the designer tunes any hue warmer, that constraint is the gate.

## Layout

The template's two-pane list + detail grid applies directly, under one
piece of shared chrome:

- **App header:** a thin full-width band (a slim bar, visibly lighter
  presence than the panes — surface-raised, 1px hairline bottom border)
  that carries the always-on chrome the workspace can't. Currently
  that's the site title 「宮舞モカ 台本工房」 in label type (muted),
  the tab navigation for the two Sumi-recipe screens (台本 for the
  two-pane workspace, 辞書 for the reading dictionary), and — flushed
  to the far right — the **notification-subscribe megaphone toggle**
  (see Components). Nothing else has earned a spot here yet; when
  something eventually does, it goes here because the header is the
  natural home for app-global state, not because the header is the
  only place chrome is allowed. The bar stays visually quieter than
  the workspace so the panes below remain the primary reading surface.
- **List pane: projects.** Card rows — project name (label type), line
  count and updated-at (caption muted). The one primary button here is
  「新規プロジェクト」.
- **Detail pane: the line workspace.** A single scrolling column of line
  rows in narration order. The pane's sticky footer holds the pour-in
  entry point (labeled 「台本追加」) — pour-in is the primary action of
  this pane.
- **The two pane headers share one computed height** — title rows,
  padding, and any header controls align across the pane boundary so the
  workspace reads as one surface, not two stacked tools.
- **Dictionary view (辞書 tab):** not two-pane — a single dense table in
  the comic-server bookshelf spirit: one row per entry, columns 表記 /
  読み / play / delete, scanning many entries at once is the point. The
  add form (two inputs + the screen's one primary button) sits above the
  table. The per-row play button speaks the reading via /say for an
  instant ear check.

## Components

Domain components on top of the Sumi recipes:

- **Project row (list pane):** play-all icon-button (SVG triangle) to
  the left of the project name — tapping it starts the radio
  listen-through of that project's lines in order. During playback the
  button becomes stop, and the detail pane's playing indicator walks
  down the rows. Line count and updated-at stay caption muted.
- **Line row:** a card row containing, left to right: drag handle
  (SVG grip-vertical, muted; cursor grab), play icon-button, line text
  (body-sm, two lines max, ellipsized), emotion chips (演技 lines only),
  mode toggle badge, delete icon-button (quiet, danger on hover). There
  is no overflow menu and no edit button. Clicking the card toggles the
  inline emotion editor on a 演技 line and does nothing on an
  アナウンサー line — the card click is never an edit affordance. The
  currently-playing row carries a 4px accent left bar per the template's
  data-bar recipe — playing is chrome ("you are here"), not data.
- **Line text is immutable in place.** There is no inline editing of any
  kind (no input swap, no textarea, no contenteditable). All text
  operations live in the line context menu.
- **Line context menu (right-click / long-press on a line card):** the
  browser context menu is intercepted and replaced with a Sumi menu
  modal scoped to that line, top to bottom:
  - **台本追加** — inserts new lines *after this line* via the pour-in
    modal (the sticky-footer 台本追加 keeps appending at the end).
  - **テキスト編集** — a single-line text input pre-filled with the
    line's text inside the modal; Enter / primary commits, Esc cancels.
    This is the only way to change a line's text.
  - **この行を複製** — duplicates the line (mode and script JSON
    included), inserting the copy directly below. This exists for A/B
    direction work: tune the copy's emotions, keep the better take,
    delete the loser.
- **Mode toggle badge (アナ / 演技):** caption-size outline badge that is
  a real button. Tapping アナ runs /analyze on that line (button shows
  the template spinner while analyzing, ~10s) and flips it to 演技;
  tapping 演技 opens ConfirmModal (the hand-tuned JSON is destroyed)
  before reverting to アナ. Visually it stays as quiet as a badge —
  affordance comes from cursor and hover wash, not weight.
- **Reorder by drag:** the grip handle drags the row; a 2px accent
  insertion line shows the drop position between rows; dropping persists
  the new order immediately. Reordering exists because narration order
  is the artifact — but drag is the only mechanism; no up/down buttons.
- **Play / stop button:** the template 36px icon-button with SVG
  triangle / square glyphs (`currentColor`, Lucide-style). Never the
  characters ▶ / |> / ■. One line plays at a time; starting a line stops
  the previous one. Playback streams — the button flips to stop as soon
  as the stream opens, with the template spinner shown only if first
  audio takes >300ms.
- **Emotion chips:** caption-size value chips (`axis名 值`) on a line
  row, tinted with the axis color at subtle alpha with the axis color as
  text. Chips show only axes present in the line's JSON, in the fixed
  axis order. When a line has multiple segments, chips summarize the
  maximum value per axis; the full per-segment detail lives in the
  editor.
- **Emotion editor (inline expansion):** per segment: the segment text
  (body-sm), five labeled sliders (axis color fill on a muted track,
  current value as caption at the right), and three scalar steppers for
  speed / pitch / pause in neutral chrome. Edits update the JSON
  immediately (autosave, no save button) — the play button is always one
  tap away so the listen→nudge→listen loop never leaves the row.
  Re-analysis has no dedicated control: it is the mode toggle round-trip
  (演技→アナ→演技), each leg guarded as described above.
- **Pour-in modal (台本追加):** the template modal with a large
  textarea and a two-option mode segmented control — アナウンサー
  (default) / 演技 — described in one caption line each. Confirm is the
  modal's primary button. In 演技 mode the modal shows per-line analyze
  progress (「3/12 を分析中…」, muted caption + template spinner);
  analysis is slow (~10s per line) and the UI says so plainly instead of
  pretending otherwise. Lines that fail analysis land as アナウンサー
  lines with a danger-role caption, never lost.
- **Destructive actions** (project delete, line delete, 再分析,
  演技→アナ toggle) route through ConfirmModal per the template.
- **Notification-subscribe megaphone (app header, far right):** a
  template quiet icon-button toggling an app-global subscription to
  server-pushed notifications (POST /notify → SSE broadcast). OFF
  (muted): megaphone-off SVG in `muted` color. ON (subscribed):
  megaphone SVG in `secondary` (Kinari) / `secondary-dark` (Sumi) with
  a slow opacity breathing (~2s, 0.6→1.0→0.6; suspended when
  `prefers-reduced-motion: reduce` is set — the color-only state still
  conveys ON). Clicking OFF→ON both opens the SSE stream *and*
  synchronously calls `new Audio().play().catch(()=>{})` inside the same
  handler to satisfy autoplay policies (Safari requires the play() to
  originate from the user gesture). Received text is played via a
  private HTMLAudioElement pointed at `/say` (text/plain, no
  /analyze — notifications are read plain for latency); it is
  **independent from the line-player** and may sound simultaneously
  with a running line or radio playback — script-listening and
  notification-listening are separate concerns, so the template's
  "exactly one line sounds at a time" rule scopes to the line-player
  only. If notifications pile up, they are played serially in FIFO;
  no queue cap is enforced (fire-and-forget delivery in the server
  guarantees the queue only holds arrivals-since-subscribe).

- **Work tab (作業 — pomodoro work-companion):** a third top-level tab
  that turns the app into a Chill-with-You-style 作業通話 window: a
  pomodoro timer plus モカ's voice and portrait keeping the user
  company. It is *scenery, not workspace* — the vim shortcut system
  stays 台本-only (the global keydown guard bails on this tab) and no
  Sumi list/card recipes appear here.
  - **Framing: the whole stage is モカ's laptop-webcam feed.** She sits
    at her desk on a video call with the user (a same-age school
    friend); the camera sees her from the chest up plus her room behind
    her. Never show elbows-down or legs (hands are below the camera),
    never show the bed. Thin call chrome (a 通話中 pill + vignette)
    sells the framing without becoming UI.
  - **Layers (back to front):** WorkScene (her room, drawn honestly or
    not at all: a hard-skewed **window on the left wall** whose left
    edge runs off-frame — rotateY(60°), far edge much shorter, sill at
    her shoulder height —, one dark vertical at the side/back wall
    corner, and a tall door on the back wall cut off at the bottom.
    No ceiling, no floor, no closet: flat-layer CSS cannot keep those
    consistent, so anything that would betray the fake 3D is omitted),
    WorkPortrait (bust-up crop of the official art), call chrome, timer
    panel on a translucent glass slot (`--c-bg` at ~72% + blur) so it
    stays readable over the scenery.
  - **Scenery palette is scene art, not chrome:** the sky gradients
    (dawn 5–8 / day 8–16 / dusk 16–19 / night otherwise), wall shades,
    window-frame wood, lamp glow and drifting motes use fixed literal
    colors and are identical in Sumi and Kinari. Sky and wall bands
    cross-fade by opacity (3s); town lights and the moon appear at
    night only. All ambient motion (motes, portrait breathing, blink,
    lip-flap) stops under `prefers-reduced-motion: reduce`.
  - **Voice tone contract:** モカ is a 2nd-year high-schooler on a
    habitual "mutual accountability" work call with a classmate —
    casual タメ口, zero romantic tension. Emotion params stay subtle:
    `honwaka` never exceeds 20 (higher reads as a put-on phone voice);
    calm is expressed with `speed` ≈ 90, not warmth axes. Chatter is
    overheard muttering (「ええと……」「よし、動いた!」, base
    `bosoboso` ≈ 85 — anything lower barely registers in the actual
    output) plus deliberately trivial small talk, played at
    reduced volume (~0.45 — a friend muttering, not addressing you)
    every 1–1.5 minutes (ひかえめ: 5–10min).
  - **Expression contract:** the idle face never drifts on its own —
    working = half-lidded (208), break/waiting = 001 looking at the
    camera. Every line carries its own eye-set (`{eye, script}`);
    addressed speech always uses 001, and she never *talks* with her
    eyes closed. Closed-eye sets appear only as blinks and inside
    scripted sequences: a mutter → eyes closed for ~8s → a recovery
    line (「よし、集中!」/「しょうがない、切り替えてこ」), a solo
    ドヤ that keeps a pleased mouth for 20s after, and the break-start
    ritual (「はい、そこまで」→ 5s → eyes closed 「うぅ、疲れたぁ」).
    Timer milestones cancel any in-flight sequence (generation token)
    so the face and the voice never disagree.
  - **Timer ring** is always accent — `secondary` stays reserved for
    persistent alive states and is unused on this tab. Phase is
    conveyed by the label (待機/作業中/休憩中/どうする?), not by
    recoloring the ring. The voice has no ON/OFF toggle — a silent
    companion is just another pomodoro timer; おしゃべり「なし」is the
    quiet option, and autoplay is unlocked by the 開始 click itself.
  - **Portrait:** the official 232-PNG diff set (18 eye-sets × 13
    mouths; two sets ship with 12, so the manifest is an explicit
    table, not arithmetic — served from `MOCA_ASSETS_DIR` via
    `/moca-assets`, never committed), shown as a **bust-up webcam
    crop** (face-to-chest region of the source art; the full standing
    figure is never displayed). Blink = 120ms swap to the blink pair every 3–8s
    (suppressed while speaking); lip-flap = ~110ms random vowel cycling
    while the voice audio plays; expression follows the phase (work =
    normal, break = smile) and, while speaking, the line's emotion
    summary. Missing assets degrade gracefully: the layer disappears
    and the tab remains a timer over scenery.
  - **Voice (声かけ):** a private audio path (FIFO, script-JSON POST
    /say) independent from the line-player, like notifications. There
    is no set count: after each break the timer stops and モカ asks
    what to do next (time-of-day aware — lunch / dinner / "it's late,
    stop?"), and continuing is always the user's click. It speaks at
    milestones (start / break start / the ask / session end, plus one
    line after a sleep-resync; manual pause→resume is silent),
    occasionally
    mid-work (チャッター, 1–1.5min, skipped while the line-player is
    sounding), and mixes in LLM-generated lines (~25%, `/work/talk`)
    with a 20s client fallback to the fixed line set. Timer and voice
    state live in module scope so leaving the tab never stops the
    pomodoro.

## Do's and Don'ts

- Do keep emotion colors monosemous: one axis, one hue, slider + chip
  only. Never chrome, never status, never the playing indicator.
- Do keep アナウンサー rows visually colorless — quietness is the mode's
  identity. Don't add a "no emotion" gray chip.
- Don't persist audio anywhere. If a future feature seems to need stored
  WAVs, the feature is out of scope until this document says otherwise.
- Don't run /analyze implicitly — analysis happens at pour-in or behind
  an explicit, ConfirmModal-guarded 再分析.
- Do keep the listen→nudge→listen loop inside the line row: sliders
  autosave and the play button never moves away from the hand.
- Do render play/stop as monochrome SVG per the template — the ▶ glyph
  and emoji are banned even though this is an audio tool.
- Do state real latency (analysis progress, first-audio spinner) in muted
  captions; never fake instant.
- Do keep the master text sacred: the dictionary rewrites what is heard,
  never what is displayed or stored in lines / script JSON.
- Don't build accent or intonation controls — reading (どう読むか) is in
  scope, melody (どう歌うか) is VOICEPEAK's.
