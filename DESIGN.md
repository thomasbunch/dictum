# DICTUM — TAPE — Binding Design Specification

Direction: **TAPE** — Dictum as a wire-service printer. Every dictation is a line printed on one
continuous tape. The main window's home *is* the history. This document is binding: if a state,
size, or duration isn't here, the doc is incomplete — file an issue, don't invent.

Implementation target: Tauri 2 webviews, vanilla TS + CSS. No component libraries. Fonts are
bundled locally (IBM Plex Sans + IBM Plex Mono, OFL — ship the license notice). Zero egress.

---

## 1. Design tokens

All colors are CSS custom properties on `:root` (theme class swaps the whole set, §9).

### BONE (default light)

| Token | Value | Role |
|---|---|---|
| `--field` | `#E9E6DF` | window background |
| `--paper` | `#F1EFE9` | chips, keycaps, inputs, cards |
| `--register` | `#DCD8CE` | hover fills, undo bar, secondary surfaces |
| `--ink` | `#211F1A` | primary text, borders of interactive elements, waveform |
| `--ink2` | `#57534A` | secondary text, values, hints |
| `--line` | `#A8A296` | window border, input borders (non-interactive) |
| `--hair` | `#D6D2C7` | hairline rules, titlebar border, meter track |
| `--hair2` | `#E0DCD2` | row separators inside the tape |
| `--dots` | `#D0CCC0` | sprocket-hole dots, waveform zero-line |
| `--oxide` | `#C23B2B` | **clipped audio data ONLY** (§10 rule 1) |

### OBSIDIAN (dark)

| Token | Value |
|---|---|
| `--field` | `#211D22` |
| `--paper` | `#2A262B` |
| `--register` | `#191619` |
| `--ink` | `#EAE7E0` |
| `--ink2` | `#A5A09A` |
| `--line` | `#5A5560` |
| `--hair` | `#3A353C` |
| `--hair2` | `#2E2A30` |
| `--dots` | `#38333A` |
| `--oxide` | `#E05A3F` |

### Additional themes (same 10 slots; ink set reused from BONE)

LEDGER field `#DCE2D0` / paper `#E3E8D9` / register `#C8D1B9` / ink `#1C211A` / ink2 `#4F584A` /
line `#9AA38D` / hair `#C5CDB6` / hair2 `#D3DAC5` / dots `#BEC6B1` / oxide `#C23B2B`.
GLACIER field `#D9E0E6` / paper `#E4EAEF` / register `#C6CFD8` / ink `#1A1F24` / ink2 `#49525C` /
line `#96A0AC` / hair `#C4CCD4` / hair2 `#CBD3DA` / dots `#BCC5CE` / oxide `#C23B2B`.
LILAC field `#E0DBE6` / paper `#EAE6EF` / register `#CEC7D8` / ink `#201C26` / ink2 `#554F5E` /
line `#A29AAE` / hair `#CEC8D6` / hair2 `#D6D0DE` / dots `#C6BFD0` / oxide `#C23B2B`.

Derived alphas: disabled = element at `opacity: 0.38`. Ink-dry settled text = `opacity: 0.88`
(fresh prints at 1.0, see §7).

## 2. Typography

Two families, bundled: **IBM Plex Sans** (400, 600) and **IBM Plex Mono** (400, 500).

| Style | Font | Size/LH | Weight | Tracking | Case | Use |
|---|---|---|---|---|---|---|
| Wordmark | Sans | 20 / auto | 600 | .30em | caps | masthead DICTUM |
| Label | Sans | 10 / auto | 600 | .18em | caps | nav, section heads, HUD state, buttons |
| Microlabel | Sans | 9 / auto | 600 | .18em | caps | counter captions, footer, day rules |
| Action | Sans | 9 / auto | 600 | .14em | caps | inline links (COPY, STRIKE, UNDO) — underlined, offset 3px |
| Body | Sans | 13.5 / 1.55 | 400 | 0 | sentence | transcript text |
| Value-lg | Mono | 28 / auto | 500 | 0 | — | masthead counters |
| Value | Mono | 11 / auto | 400 | 0 | caps | status line, timestamps, meta |
| Value-sm | Mono | 10 / 1.6–1.7 | 400 | caps | — | notes, hints, footer meta |
| Value-xs | Mono | 9 / auto | 400 | caps | — | HUD hints, specimen captions |
| Keycap | Mono | 12 / auto | 400 | 0 | caps | keycap chips |
| HUD timer | Mono | 13 / auto | 400 | 0 | — | elapsed time, char count |

No other sizes/weights. Titlebar caption glyphs (`─ ▢ ✕`) are Mono 11 in `--ink2`.

## 3. Layout constants

- Grid: **8px**. Radius: **0 everywhere** (the OS may round the outer window; the native tray
  menu is exempt). No shadows, no gradients, no glow, no blur.
- Main window: default **880×700**, min 720×520, resizable. Content column is fluid; the
  sprocket margin and paddings are fixed.
- Titlebar: 32px, bottom border 1px `--hair`.
- Masthead: padding 20px 24px 16px, bottom border **1px `--ink`** (strong rules mark the
  masthead and footer; everything inside uses hairlines).
- Footer bar: padding 8px 24px, top border 1px `--ink`.
- Sprocket margin: 28px wide, right border 1px `--hair`, dot pattern:
  `radial-gradient(circle 3px at 14px 14px, var(--dots) 97%, transparent)`, tile 28×28px.
- Section paddings inside views: 18px 24px. Hairline rules between sections: 1px `--hair2`.
- Keycap chip: Mono 12, padding 4px 8px, border 1px `--ink`, background `--paper`, gap 4px.

## 4. Global interactive states (every control)

| State | Treatment |
|---|---|
| default | 1px `--ink` border, `--paper` fill (buttons); underline offset 3px (links) |
| hover | fill `--register` (buttons); text `--ink2` (links); row fill `--register` (tape rows). No size change |
| focus (keyboard) | `outline: 2px solid var(--ink); outline-offset: 2px` — identical every theme |
| active/press | stamp-press: `transform: translateY(1px)`, no transition |
| disabled | `opacity: 0.38`, `cursor: default`. **Never hidden.** |

Toggle: track 36×16, 1px `--ink` border, `--paper` fill; knob 10×10 square, inset 2px;
ON = knob `--ink` at right; OFF = knob `--ink2` at left. Radio: 10×10 square, 1px `--ink`
border; selected = filled `--ink`. Text input: border 1px `--line`, `--paper` fill, padding
6px 10px, Mono 11; focus adds 1px `--ink` border + focus outline; placeholder `--ink2` caps.

## 5. Surfaces

### 5.1 Main window — shell

Views: **TAPE** (home) · **WORDS** · **SETUP**, switched by masthead nav (Label style; active =
`--ink` + 2px bottom border; inactive = `--ink2`; hover = `--ink`). Tray deep-links: History →
TAPE, Settings → SETUP. Footer on every view: left `NOTHING LEAVES THIS MACHINE` (Microlabel,
`--ink2`), right live privacy meta (Value-sm): `TRANSCRIPTS · <retention> / AUDIO · <ON|OFF>`.

Masthead (TAPE view only; WORDS/SETUP use the slim masthead: wordmark + nav + no counters):
- Status line (Value, `--ink2`): `MODEL LOADED · PARAKEET-TDT 0.6B V2 — MIC OK · <device> —
  LOCAL ONLY · ZERO EGRESS`. Degraded states print in place: `MODEL NOT LOADED (IDLE)`,
  `NO MODEL ON THIS MACHINE`, `NO MICROPHONE`.
- Counters (Value-lg + Microlabel captions): WORDS TODAY · PRINTED · DAYS RUNNING.
- BY APP line (Value-sm, `--ink2`) under counters: `code.exe 41 · Terminal 22 · …  %`
  — **proposal**, driven by per-record exe data; cut if unwanted.
- Right: keycap chips of current hotkey + caption `HOLD TO SPEAK — THE TAPE PRINTS HERE`
  (caption follows mode: TOGGLE → `TAP TO SPEAK…`, BOTH → `TAP OR HOLD…`). Clicking the chips
  starts a test dictation targeted at a scratch line in the masthead. **Keycaps are the
  "prominent start path" and mic test.**

### 5.2 TAPE view (home = history)

- Toolbar under masthead: search input 260px, placeholder `SEARCH THE TAPE…`; right meta
  (Value-sm): `<n> LINES · KEPT <retention> · AUDIO <ON|OFF>`. Search filters as you type;
  active search shows `<n> LINES MATCH · ESC CLEARS` in the meta slot.
- Feed: sprocket margin left; rows padding 10px 24px 12px 16px, separated 1px `--hair2`.
- Day rule: Microlabel (`TODAY — MON JUL 20`, then `SUN JUL 19`…) + hairline to the right edge.
- Row anatomy: meta line = time (Value, `--ink2`) · exe chip (Mono 9.5, 1px `--hair` border,
  `--paper` fill, padding 1px 6px) · spacer · char count `<n> CH · PRINTED` (Value-sm).
  Text line = Body, `text-wrap: pretty`.
- Row hover: fill `--register`; char count is replaced in place by actions `COPY` `STRIKE`
  (Action style, 12px gap). **No layout shift** — same slot, same line-height.
- Row expanded (click anywhere on row): fill `--paper`, 2px `--ink` left border; meta line gains
  `· LINE #<id> · <dur> S · <NO CLIPPING|CLIPPED>`; full text (max-width 640px); printed
  envelope trace of the take (SVG/canvas 280×24, stroke `--ink2` 1.4px, zero-line `--dots`;
  oxide ticks only if that take clipped); injection line (Value-xs, `--ink2`):
  `PRINTED TO <exe> · <TYPED|PASTED> · <n> CHARS`. Actions: COPY · STRIKE · CLOSE ✕.
  Only one row expanded at a time. Expansion is instant (no height animation).
- STRIKE: the row is replaced in place by the undo bar (fill `--register`, 1px `--line`
  border): `■ LINE PULLED` + `#<id> · <n> CH` + right `UNDO` (Action) + countdown `6 S`
  (Value). Counts 6→0 (text update per second, no bar animation); at 0 the bar and row are
  removed (removal is instant). UNDO restores the row in place.
- Empty state (no records at all, or retention = NOTHING): centered in feed,
  `THE TAPE IS BLANK.` (Label) + `HOLD <hotkey> AND SPEAK.` (Value, `--ink2`).
  Empty search: `NOTHING ON THE TAPE MATCHES.` + `ESC CLEARS.`
- New line after injection: prints at top with ink-dry (§7). If the main window is open during
  dictation, no live row is shown — the line prints when it lands.

### 5.3 WORDS view

Two columns (1fr/1fr, 1px `--hair` divider), plus filler strip and footer.
- VOCABULARY (left): head = label + live counter `12 / 50` (Value-sm). Note:
  `PROPER NOUNS AND JARGON — BIASES THE EAR.` Add row: input (flex) + `ADD` button.
  Terms are chips (Mono 11, 1px `--hair` border, `--paper` fill, padding 3px 8px) with `✕`
  remove (hover: `--ink`; chip hover fill `--register`). At 50, input + ADD disable
  (opacity .38) and counter reads `50 / 50 — FULL`. Warning line (always printed, hairline
  above): `A LONG LIST DULLS THE EAR — KEEP IT UNDER 30.` At >30 the counter renders in
  `--ink` weight 600 (never oxide).
- REPLACEMENTS (right): head = label + `CASE-INSENSITIVE` (Value-sm). Note: `HEARD ON THE
  LEFT, PRINTED ON THE RIGHT.` Table: header row Microlabel with 1px `--ink` rule; rows
  Mono 11.5, 1px `--hair2` separators, `✕` per row; last row is the ghost add row
  (`HEARD…` / `PRINTED…` in `--ink2`; click focuses inline inputs). Links: `ADD ROW`, right
  `IMPORT` `EXPORT` + `TXT · JSON` (Value-sm `--ink2`). Import/export via native file dialog.
- Filler strip (full width, hairline above): toggle + `STRIKE FILLER WORDS` (Label) +
  `UM, UH, ER NEVER REACH THE TAPE.` (Value-sm).

### 5.4 SETUP view

Printed form: rows `grid-template-columns: 180px 1fr; gap: 24px`, padding 18px 24px, 1px
`--hair2` rules between. Left cell: section Label + Value-sm note. Sections in order:

1. **KEY** — note `THE ONLY KEY DICTUM OWNS.` Recorder chip: 1px `--ink` border, `--paper`,
   padding 10px 14px; chord in Mono 14 + caption `CLICK, THEN PRESS A CHORD` (Action style,
   `--ink2`). Click → armed: border turns **dashed** 1px `--ink`, text `PRESS A CHORD…`
   (Mono 14 `--ink2`), caption `ESC CANCELS.` Accepted chord prints and disarms. Rejected
   (CapsLock, bare F-keys, bare modifiers): the offending key prints struck-through
   (`text-decoration: line-through`) for 1.2s with line below (Value-sm, `--ink`):
   `THAT KEY CAN'T CARRY THE WIRE. PRESS A CHORD.` — stays armed. Mode radios: HOLD
   (`PUSH-TO-TALK. RELEASE PRINTS.`) · TOGGLE (`TAP ON, TAP OFF.`) · BOTH
   (`TAP TOGGLES · HOLD TALKS.`).
2. **INPUT** — note `METER LIVE ONLY WHILE VISIBLE.` Device select 320px (Mono 11.5 + `▾`),
   native dropdown listing devices + `SYSTEM DEFAULT`; caption `FOLLOWS SYSTEM DEFAULT WHEN
   UNSET.` Level meter 320×8: segmented track `repeating-linear-gradient(90deg, var(--hair)
   0 6px, transparent 6px 9px)`, fill same pattern in `--ink`, width = level (canvas or
   clip-path; renders **only while SETUP is visible** — destroy the stream on view exit).
   The meter never shows oxide (input clipping shows only in HUD/records). Toggle AUDIO CUES:
   `CLICKS ON START · STOP · KILL · ERROR. NEVER ON SUCCESS.`
3. **MODEL** — note `THE ONLY DOWNLOAD DICTUM WILL EVER MAKE.` Card 440px, 1px `--line`
   border, `--paper`: name Mono 12.5 + status `● LOADED` (Mono 10; states: `● LOADED`,
   `○ IDLE — UNLOADED`, `NOT ON THIS MACHINE`); line 2: `610 MB · SHERPA-ONNX · CPU`.
   Download flow inside the card (states, all Mono 10.5):
   - fetching: `FETCHING…` + right `<n>% OF 610 MB`; bar 6px, track `--hair`, fill `--ink`
     scaled by `transform: scaleX` (transform-origin left).
   - verifying: `PROOFING…` + right `SHA-256`; bar full, striped
     `repeating-linear-gradient(90deg, var(--ink) 0 6px, var(--hair) 6px 12px)`, static.
   - done: `READY.` + `● LOADED`.
   - failed: `FAILED — THE WIRE BROKE AT <n> MB.` + `RETRY` button.
   Toggle UNLOAD ON IDLE (default OFF): `FREES ~600 MB. THE NEXT TAKE PAYS A RELOAD.`
4. **TAPE & PRIVACY** — note `THE STATE IS PRINTED ON EVERY SURFACE.` Toggles KEEP
   TRANSCRIPTS (default ON) and KEEP AUDIO (default OFF, caption `OFF BY DEFAULT.
   TRANSCRIPTS ONLY.`). Retention segmented row (radio semantics, chips Mono 10):
   NOTHING · 24 H · 7 D · 30 D · FOREVER; selected = 1px `--ink` border + `--paper` fill +
   `--ink` text; unselected = 1px `--line` border, `--ink2`. KEEP TRANSCRIPTS OFF disables
   the retention row and search (disabled treatment, still visible).
5. **INJECTION** — note `PER-APP OVERRIDES.` Table (Mono 11): APP (190px) · METHOD (80px:
   PASTE|TYPE) · PASTE KEY (120px: CTRL+V | CTRL+SHIFT+V | —) · DELAY (80px: `<n> MS/CHUNK`,
   TYPE only) · `✕`. First row `DEFAULT — ALL APPS` (`--ink2`, not deletable). `ADD APP`
   link → row with exe input; METHOD/PASTE KEY/DELAY are click-to-cycle chips.
6. **APPEARANCE** — note `APPLIES INSTANTLY.` Theme cards 72px wide, field-colored, name in
   own ink; selected = 1px `--ink` + focus-style 2px outline offset 2px. Order: BONE ·
   LEDGER · GLACIER · LILAC · OBSIDIAN. Apply on click, no confirm, no transition (§7).
7. **ABOUT** — `DICTUM <ver> · APACHE-2.0 · SOURCE ↗` (Mono 11; SOURCE opens repo in
   default browser — the app itself still makes no calls). Statement (Value-sm, 1.7):
   `EVERYTHING — AUDIO, TEXT, MODEL — STAYS ON THIS MACHINE.` / `DICTUM MAKES NO NETWORK
   CALLS. THE MODEL FETCH IS THE ONE EXCEPTION, AND IT ONLY HAPPENS WHEN YOU ASK.`
   Include OFL notice for IBM Plex here (small `FONTS: IBM PLEX · OFL` line linking a
   bundled license file).

### 5.5 First run (no model)

TAPE view with: status line `NO MODEL ON THIS MACHINE — MIC OK · <device> — LOCAL ONLY ·
ZERO EGRESS`; masthead counters replaced by button `FETCH THE MODEL · 610 MB` (Label style,
padding 10px 18px) + two-line note (Value-sm): `PARAKEET-TDT 0.6B V2 · SHERPA-ONNX · CPU` /
`THE ONLY DOWNLOAD DICTUM WILL EVER MAKE. AFTER THIS, THE WIRE GOES DARK.` Fetching swaps
the button for the download bar (§5.4.3 states). Feed shows the blank-tape state with
`FETCH THE MODEL, THEN HOLD <hotkey> AND SPEAK.` / `WHAT YOU SAY PRINTS INTO WHATEVER WINDOW
HAS FOCUS.` No wizard, no modal.

### 5.6 HUD overlay

Geometry: **400×52**, bottom-center of the active monitor, 24px above the work-area edge.
1px `--ink` border, `--field` fill, radius 0. Always-on-top, never focused, no taskbar
presence, click-through EXCEPT during `confirm_discard` and `error` (hover shows nothing —
the HUD has no interactive elements). Must be visible ≤50ms after hotkey press: keep the
window pre-created and hidden; show + start canvas immediately.

Internal layout (flex, gap 12px, padding 0 14px): status square 8×8 `--ink` · state Label ·
center slot (waveform canvas 140×32 | progress 140×6 | message Value-sm) · spacer · right
value (HUD timer style) · hint (Value-xs `--ink2`).

Waveform (canvas): pen trace through per-bar amplitudes — one bar = 37.5ms; plot midline
y=16, amplitude ±15px, connected polyline, stroke `--ink` 1.6px; zero-line 1px `--dots`.
Scroll: newest sample enters at right, 140px window ≈ last 4.2s. **Clip flag: a 2.4px oxide
tick, 5px tall, drawn from the top edge at that bar's x** — oxide appears nowhere else.
Canvas redraws only while `listening`.

| State | Label | Center | Right | Hint | Enter/exit |
|---|---|---|---|---|---|
| `hidden` | — | — | — | — | window hidden; zero paint, no ghost |
| `loading_model` | `WARMING UP` | progress bar (scaleX = %) | `<n>%` | — | shows on cold-start hotkey; → listening when loaded |
| `listening` | `LISTENING` | live trace | elapsed `m:ss` | `ESC ✕` | in: rise+fade 120ms |
| `transcribing` | `PRINTING…` | trace frozen, opacity 0.45 | elapsed frozen | — | crossfade 80ms |
| `injected` | `PRINTED` | empty | `<n> CH` | — | dwell 900ms → fade out 100ms |
| `cancelled` | `KILLED` | empty | — | — | dwell 500ms → fade out 100ms |
| `confirm_discard` | `HOLD ON` | `ESC AGAIN KILLS THE TAKE` | elapsed | — | replaces listening content; Esc→cancelled, any speech/hotkey→listening |
| `error` (no mic) | `NO MICROPHONE` | `PLUG IN OR PICK ANOTHER INPUT` | — | — | dwell 2400ms → fade out 100ms |
| `error` (no model) | `NO MODEL` | `OPEN DICTUM TO DOWNLOAD` | — | — | dwell 2400ms |
| `error` (elevated) | `PROTECTED WINDOW` | `SENT TO CLIPBOARD — PASTE IT` | — | — | dwell 2400ms |

`confirm_discard` triggers only when Esc is pressed on a recording >30s (else Esc →
`cancelled` directly). Errors never use oxide; the status square stays `--ink` in every state.
HUD theme follows the app theme.

### 5.7 Tray

Icon 16×16, static, three states (see mockup §07): **idle** = tape-cartridge outline (1px
stroke) with dashed center line; **recording** = solid 3px center bar; **mic-error** = idle
glyph + diagonal strike. Render in the system foreground color (light/dark taskbar aware).
No animation, no badge counts.

Menu (native; list binding): `Start dictation　Ctrl+Alt+Space` (reads `Stop dictation` while
recording) / `Paste last transcription` (disabled when history empty) / separator / `History`
(opens TAPE) / `Settings` (opens SETUP) / separator / `Quit Dictum`. Left-click toggles
dictation; double-click opens the main window.

## 6. Microcopy (binding; the wire voice)

Rules: states are Labels in tracked caps; numbers are Mono; sentences are short declaratives,
period included; never exclamation marks; success is stated, not celebrated.

| Where | Copy |
|---|---|
| HUD states | `WARMING UP` · `LISTENING` · `PRINTING…` · `PRINTED` · `KILLED` · `HOLD ON` / `ESC AGAIN KILLS THE TAKE` · `NO MICROPHONE` / `PLUG IN OR PICK ANOTHER INPUT` · `NO MODEL` / `OPEN DICTUM TO DOWNLOAD` · `PROTECTED WINDOW` / `SENT TO CLIPBOARD — PASTE IT` |
| Footer | `NOTHING LEAVES THIS MACHINE` |
| Status line | `MODEL LOADED · <name>` · `NO MODEL ON THIS MACHINE` · `MIC OK · <device>` · `NO MICROPHONE` · `LOCAL ONLY · ZERO EGRESS` |
| Start path | `HOLD TO SPEAK — THE TAPE PRINTS HERE` |
| Empty tape | `THE TAPE IS BLANK.` / `HOLD <hotkey> AND SPEAK.` |
| Empty search | `NOTHING ON THE TAPE MATCHES.` / `ESC CLEARS.` |
| Undo | `LINE PULLED` · `UNDO` · `<n> S` |
| Row actions | `COPY` · `STRIKE` · `CLOSE ✕` |
| Key capture | `CLICK, THEN PRESS A CHORD` · `PRESS A CHORD…` · `ESC CANCELS.` · `THAT KEY CAN'T CARRY THE WIRE. PRESS A CHORD.` |
| Vocabulary | `PROPER NOUNS AND JARGON — BIASES THE EAR.` · `A LONG LIST DULLS THE EAR — KEEP IT UNDER 30.` · `50 / 50 — FULL` |
| Replacements | `HEARD ON THE LEFT, PRINTED ON THE RIGHT.` · `CASE-INSENSITIVE` |
| Filler | `STRIKE FILLER WORDS` / `UM, UH, ER NEVER REACH THE TAPE.` |
| Mic meter | `METER LIVE ONLY WHILE VISIBLE.` · `FOLLOWS SYSTEM DEFAULT WHEN UNSET.` |
| Audio cues | `CLICKS ON START · STOP · KILL · ERROR. NEVER ON SUCCESS.` |
| Model | `THE ONLY DOWNLOAD DICTUM WILL EVER MAKE.` · `FETCHING…` / `<n>% OF 610 MB` · `PROOFING…` / `SHA-256` · `READY.` · `FAILED — THE WIRE BROKE AT <n> MB.` / `RETRY` · `FREES ~600 MB. THE NEXT TAKE PAYS A RELOAD.` |
| Privacy | `THE STATE IS PRINTED ON EVERY SURFACE.` · `OFF BY DEFAULT. TRANSCRIPTS ONLY.` |
| First run | `FETCH THE MODEL · 610 MB` · `AFTER THIS, THE WIRE GOES DARK.` · `WHAT YOU SAY PRINTS INTO WHATEVER WINDOW HAS FOCUS.` |
| About | `EVERYTHING — AUDIO, TEXT, MODEL — STAYS ON THIS MACHINE.` / `DICTUM MAKES NO NETWORK CALLS. THE MODEL FETCH IS THE ONE EXCEPTION, AND IT ONLY HAPPENS WHEN YOU ASK.` |

## 7. Motion table

Rule zero: **nothing moves at idle.** Only transforms and opacity animate (waveform is canvas).
60fps or the animation is cut. Easing `--ease: cubic-bezier(0.2, 0, 0, 1)`.

| # | Trigger | Animation | Duration / easing |
|---|---|---|---|
| M1 | HUD show | `translateY(8px)→0` + `opacity 0→1` | 120ms `--ease`; first paint ≤50ms after hotkey |
| M2 | HUD hide (all exits) | `opacity 1→0` | 100ms ease-in |
| M3 | HUD state swap | content crossfade | 80ms linear; geometry never changes |
| M4 | Waveform | canvas redraw, listening only | per-frame; frozen elsewhere |
| M5 | Download progress | bar `scaleX` | on progress events, 160ms linear between values |
| M6 | New tape line (ink-dry) | row `opacity 1 → 0.88` | 400ms linear after 600ms delay |
| M7 | Undo bar in | `translateY(-4px)→0` + fade | 120ms `--ease` |
| M8 | Press (any control) | `translateY(1px)` | 0ms in, 0ms out (stamp) |
| M9 | Hover, focus, view switch, row expand, theme apply, menu | none — instant | 0ms |
| M10 | Undo countdown | text update `6 S…0 S` | 1s steps, no tween |

## 8. Accessibility

- Contrast (body/`--ink` on `--field`): BONE 13.9:1, OBSIDIAN 13.2:1; `--ink2` on `--field`
  ≥ 5.4:1 in both. Every theme must keep `--ink2`/`--field` ≥ 4.5:1 and `--ink`/`--field`
  ≥ 7:1. Oxide is never the sole carrier of meaning (clip also prints `CLIPPED` in expanded
  rows).
- Full keyboard nav in main window: Tab order = nav → view content top-to-bottom → footer.
  Arrow keys move within radio groups/retention chips; Enter/Space activates; Esc clears
  search, collapses expanded row, disarms key capture. Focus outline per §4 — never removed.
- Tape rows are buttons (`aria-expanded`); undo bar is `role="status"`; HUD window is never
  focusable (it must not steal the caret) — its state changes are also announced via the
  main window's live region when open.
- Hit targets ≥ 24×24 CSS px for mouse; all type ≥ 9px only for non-essential captions,
  body 13.5px.

## 9. Theming rules

- Every color in the UI comes from the 10 tokens — **no literal hex anywhere else.**
- Theme = a class on `<html>` (`theme-bone`, `theme-ledger`, `theme-glacier`, `theme-lilac`,
  `theme-obsidian`) defining all 10 tokens. Applying = swapping the class; instant, no
  transition; persists to config; HUD + all windows repaint on the same tick (broadcast).
- OBSIDIAN is the required dark theme; oxide brightens there (`#E05A3F`) — per-theme oxide
  is part of the token set, never computed.
- New themes: pick a field, keep ink near-black (or near-paper for dark), verify §8 ratios,
  keep oxide within `#B93326–#E05A3F`.

## 10. Hard rules (grep appendix)

1. OXIDE-IS-DATA: `--oxide` marks clipped audio only. Never CTA, error, link, hover, focus.
2. NOTHING-AT-IDLE: no animation, timer, or canvas frame when no dictation is active.
3. HUD-HIDDEN-IS-HIDDEN: idle HUD = window hidden. No ghost, pill, or 1px remnant.
4. HUD-50MS: HUD visible ≤50ms after hotkey. Pre-create the window.
5. HUD-NO-FOCUS: HUD never takes focus or taskbar presence.
6. TRANSFORM-OPACITY-ONLY: DOM animations use transform/opacity only; waveform is canvas.
7. SIXTY-FPS: any animation that can't hold 60fps is cut, not degraded.
8. TOKENS-ONLY: no hex outside the theme token blocks.
9. RADIUS-ZERO: border-radius 0 on everything Dictum draws.
10. NO-DECORATION: no shadows, gradients, glow, blur, or skeuomorphism.
11. TWO-FONTS: IBM Plex Sans + IBM Plex Mono, bundled. No web fonts, no other families.
12. ZERO-EGRESS: no network call except the user-initiated model fetch.
13. DISABLED-VISIBLE: disabled controls grey out (opacity .38), never hide.
14. FOCUS-VISIBLE: 2px `--ink` outline, offset 2px, on every focusable element.
15. NO-LAYOUT-SHIFT: hover/value swaps replace content in place; nothing reflows.
16. VOICE: labels tracked caps, numbers mono, declaratives with periods, no exclamations.
17. PRIVACY-PRINTED: footer privacy line appears on every main-window view.
18. UNDO-6S: every destructive act on the tape offers a 6s in-place undo.
19. CONTRAST: `--ink2` on `--field` ≥ 4.5:1 in every theme.
20. STATES-COMPLETE: every interactive element implements default/hover/focus/disabled.
