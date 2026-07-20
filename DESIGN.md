# DESIGN.md — Dictum UI Specification

Binding spec for every Tauri webview surface. The design language is CALIPER/LEDGER: a precision measuring instrument printed on paper. Flat ink on a tinted paper field. No gradients, no shadows, no glow, no skeuomorphism, no gold/brass. If a treatment is not in this document, do not invent it.

---

## 1. Brand

### 1.1 Wordmark

```
D I C T U M
```

- IBM Plex Sans SemiBold, all-caps, letter-spacing `0.18em`.
- Color: `ink` on `field` (or `paper`). Never oxide. Never any accent color.
- Sizes: 11px in window title rows and the tray tooltip; 16px on the Settings "certificate" header; nowhere larger.
- No logo glyph in v1. The wordmark is the mark. In the tray it is replaced by the tick-lane icon (§3).
- Optional flourish, one place only (Settings About section): the wordmark followed by a 1px `hair` rule and the mono serial line `DICTUM · v{semver} · LOCAL ONLY`.

### 1.2 Voice and microcopy

Dictum speaks like an instrument label: calm, certain, short declaratives. Present tense. No exclamation points, no ellipses-as-suspense, no apologizing, no "please wait", no personality ("Oops!", "Hmm…" are banned).

| Situation | Copy |
|---|---|
| Recording | `LISTENING` |
| Transcribing | `PRINTING` |
| Text injected | `PRINTED` |
| Cancelled | `DISCARDED` |
| No microphone | `NO MICROPHONE` |
| Model loading | `LOADING MODEL` |
| Model missing | `MODEL NOT FOUND` |
| Injection blocked (elevated window) | `TARGET ELEVATED — COPIED TO CLIPBOARD` |
| Empty history | `NO RECORDS` |
| Hotkey capture prompt | `PRESS KEYS` |

Rules:
- Status words are tracked all-caps (label style, §8). Body copy (settings descriptions) is sentence case, one sentence, no trailing period on fragments.
- Numbers are always mono: `0.8 s`, `48 kHz`, `612 MB`.
- Never blame the user. State the condition, state the remedy if one exists: `NO MICROPHONE` + `Select an input device in Settings.`

---

## 2. Recording HUD overlay

The signature surface. One always-on-top, no-activate, non-focusable, click-through-except-cancel tool window. It must never take focus (Tauri: `focus: false`, `alwaysOnTop: true`, `skipTaskbar: true`, WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW).

### 2.1 Geometry (strict 8px grid)

- **Size:** 320 × 64 px fixed. No resize, no expand-on-hover, no reflow ever.
- **Placement:** bottom-center of the display containing the cursor. Horizontal center; bottom edge 48px above the work-area bottom (above the taskbar).
- **Corner radius:** 2px. **Border:** 1px `line`. **Background:** `paper`. The window itself is transparent; the pill is drawn inside it.
- **Internal layout (left → right):**
  - 8px padding all sides.
  - Status column, 72px wide: status word (11px tracked caps, `ink`) top-aligned, elapsed time below it in mono 13px `ink2` (`0:07`).
  - 1px vertical hairline `hair`.
  - Chart-recorder lane: remaining width (224px), full inner height (48px).

### 2.2 The chart-recorder lane

The waveform is drawn like a strip-chart recorder: paper scrolls left, the pen writes at a fixed head position.

- **Paper:** lane background `paper`. Printed tick scale along the bottom edge of the lane: 1px ticks in `tick`, minor tick every 8px (≈100ms), major tick 4px tall every 40px (500ms), printed on the scrolling layer so ticks scroll with the paper.
- **Grid:** horizontal center hairline (`ink2` @14%) — the zero line.
- **Pen head:** fixed at 75% of lane width. Everything left of it is history, right of it is unwritten paper.
- **Trace:** amplitude envelope drawn as a vertical-bar oscillogram (2px bars, 1px gap, mirrored about the zero line). Historical trace: `ink` @88% (ink-dried). The bar currently being written at the pen head: **oxide** at full opacity, settling to `ink` @88% within 120ms as it scrolls left (ink-drying). Clipping (sample ≥ −1 dBFS): that bar stays **oxide** permanently and renders at full lane height.
- Oxide appears nowhere else in the HUD. It is the live signal and the clipping record — data-ink only.
- **Scroll rate:** 80 px/s, constant, driven by the audio clock (signal-driven — the paper only moves while capture is running).

### 2.3 States

| State | Window | Status column | Lane | Notes |
|---|---|---|---|---|
| **Idle / hidden** | Not shown (window hidden, not minimized) | — | — | Nothing exists on screen at idle. No mini pill, no ghost. |
| **Loading model** | Visible | `LOADING MODEL` · mono `{pct}%` | Empty paper, ticks printed, no trace, no scroll | Only on cold start when hotkey pressed before model ready. |
| **Listening** | Visible | `LISTENING` · mono elapsed `0:07` | Live scrolling trace per §2.2 | Shown ≤50ms after hotkey down. VAD-silent input still scrolls paper (silence is data: flat trace). |
| **Transcribing** | Visible | `PRINTING` · elapsed frozen | Paper stops. Trace stands complete, all ink @88%. Nothing animates. | Should last <1s; no spinner — the stopped paper *is* the state. |
| **Injected** | Visible 600ms, then hidden | `PRINTED` · mono char count `142 CH` | Frozen trace | Stamp-press on the word `PRINTED`: sinks 1px on appear (§7). Then window hides. |
| **Cancelled** | Visible 400ms, then hidden | `DISCARDED` | Trace redrawn at `ink2` @40% (greyed, not erased) | Esc cancels instantly; recordings >30s show `ESC AGAIN TO DISCARD` for 2s first. |
| **Error** | Visible until next hotkey or 4s | `NO MICROPHONE` / `MODEL NOT FOUND` / `TARGET ELEVATED — COPIED` | Empty paper | Error text is `ink`, not oxide. Oxide never means error. |

### 2.4 Transitions

All state changes are signal-driven (a real event occurred) and complete in <200ms:

- Show: opacity 0→1, 120ms, `cubic-bezier(0.2, 0, 0, 1)`. No slide, no scale.
- Hide: opacity 1→0, 160ms, same curve.
- Status word change: old word cut (no fade-out), new word ink-dries — prints at 100% opacity, settles to its resting opacity over 120ms.
- Nothing loops. Nothing pulses. Nothing breathes. If the state is static, the pixels are static.

---

## 3. Tray icon + menu

### 3.1 Icon

16×16 monochrome, drawn to match the system tray (light glyph on dark taskbar and vice versa; ship both). The glyph is a miniature chart-recorder lane: a horizontal baseline with a small trace blip and two ticks.

| State | Glyph |
|---|---|
| Idle | Flat baseline + ticks, single small blip. Static. |
| Recording | Same glyph with the blip region filled solid (heavier ink). Static — no tray animation, ever. Windows tray animation is decoration; the HUD carries the live signal. |
| Mic error | Glyph with a 1px diagonal strike through the blip. Static. |

### 3.2 Menu (native Windows context menu — do not custom-draw)

```
DICTUM                         (disabled header row)
──────────────────────────────
Start dictation    Ctrl+Win    (or "Stop dictation" while recording)
Paste last transcription
──────────────────────────────
History…
Settings…
──────────────────────────────
Quit Dictum
```

- Header row: `DICTUM` disabled item as a label. Accelerator text shows the current binding.
- Mic-error state prepends a disabled row: `NO MICROPHONE`.
- Left-click on the icon: toggle dictation. Double-click: open Settings.

---

## 4. Settings window

Styled as an instrument's calibration certificate: one column, sections separated by 1px hairlines, tracked-caps labels on the left, mono values on the right, everything on the 8px grid.

### 4.1 Frame

- Fixed width 560px, natural height, vertical scroll if needed. Background `field`. Content lane: `paper`, inset 16px, 1px `line` border, 2px radius.
- Header: `DICTUM` wordmark 16px + mono `SETTINGS` right-aligned in `ink2`. 1px `hair` rule below.
- Section pattern: section title (11px tracked caps, `ink`) with a 1px `hair` rule; rows of 40px height: label left (tracked caps 11px, `ink2`), value/control right (mono 13px, `ink`). Row separators: 1px `hair2`.
- Disabled/inactive values grey out to `ink2` @55% — never hidden, never collapsed (values on demand: grey, don't hide).
- Controls: flat. Toggles are a 32×16 track (`register`, 1px `line` border) with a 12px square `ink` thumb; checked state fills the track `ink` and inverts the thumb to `paper`. No oxide on any control.

### 4.2 Sections

**HOTKEY**
- `ACTIVATION` → mono chip `CTRL + WIN` on `register`, 1px `line` border. Click → chip text becomes `PRESS KEYS`, captures next chord.
- `MODE` → segmented control (flat, hairline-divided): `HOLD` / `TOGGLE` / `BOTH` (both = tap toggles, hold is PTT).
- Rejected bindings (CapsLock, bare F-keys) show inline: `Not supported. Remap CapsLock with PowerToys.`

**MICROPHONE**
- `INPUT DEVICE` → native `<select>`, mono text.
- `LEVEL` → a static-when-silent inline lane, 224×24: same trace rules as the HUD (§2.2), oxide pen head + clipping only. This is the sole moving element in Settings, and only while the mic delivers signal and this section is visible.

**MODEL**
- `MODEL` → mono `PARAKEET-TDT 0.6B V2 INT8`
- `STATUS` → mono `LOADED · 612 MB` (greyed `NOT DOWNLOADED · GET` with an ink text-button when absent)
- `RUNTIME` → mono `SHERPA-ONNX · CPU`

**VOCABULARY** (two subsections, mirroring the two-layer dictionary)
- `VOCABULARY` — biasing hints. Plain list editor, mono entries. Footer note: `Hints bias recognition. Many entries reduce accuracy.` Entry counter mono: `12 / 50`.
- `REPLACEMENTS` — two-column table `HEARD → PRINTED`, mono both sides, hairline row rules. Case-insensitive badge in header.
- Row of ink text-buttons: `IMPORT` `EXPORT` (TXT/JSON).

**HISTORY**
- `KEEP TRANSCRIPTS` → toggle (default on)
- `KEEP AUDIO` → toggle (default **off**)
- `RETENTION` → select: `KEEP NOTHING / 24 H / 7 D / 30 D / FOREVER`

**APPEARANCE**
- `FIELD` → theme picker: six 40×24 swatches in a row — LEDGER, BONE, PLASTER, GLACIER, LILAC, OBSIDIAN — each a flat rect of that variant's field color, 1px `line` border, selected swatch gets a 2px inset `ink` keyline. Tracked-caps name under each, 9px. Selection applies instantly (<200ms), no preview modal.

**ABOUT**
- Wordmark + mono serial line: `DICTUM · v1.0.0 · LOCAL ONLY · ZERO EGRESS`
- `LICENSE` → mono `APACHE-2.0` · `SOURCE` → mono URL as ink text-link (underline on hover only, no color change).

---

## 5. History window

- 560×480 default, resizable. Same field/paper framing as Settings.
- Header row: `HISTORY` tracked caps + search input right-aligned (flat, `register` bg, 1px `line` border, mono input text; searches raw transcript text).
- List: one row per dictation, 48px tall, hairline-separated:
  - Left: mono timestamp, `ink2`: `07-20 14:32:08`
  - Middle: first line of transcript, IBM Plex Sans Regular 13px `ink`, single line, ellipsized.
  - Right: mono char count `142` in `ink2`; on row hover, replaced by ink text-buttons `COPY` · `DELETE` (no reflow — buttons occupy the same fixed 96px slot the count sits in; count greys to @0, buttons print in).
- Row click: expands in place (only permitted layout change; user-initiated, 160ms) to full transcript in mono 13px, with `COPY` and `DELETE` beneath.
- Delete is immediate, no oxide, no red. A 4s footer line `RECORD DELETED · UNDO` (ink text-button) covers mistakes.
- Empty state: centered `NO RECORDS` tracked caps `ink2`.
- Footer: mono `24 RECORDS · AUDIO OFF · RETENTION 7 D`, `ink2` — the privacy state is always printed on the surface.

---

## 6. Color semantics

| Token | LEDGER value | OBSIDIAN value | Meaning in Dictum |
|---|---|---|---|
| `field` | `#DCE2D0` | `#211D22` | Window background |
| `paper` | `#E3E8D9` | `#2A252B`* | Content lanes, HUD pill, chips |
| `register` | `#C8D1B9` | `#332D34`* | Secondary surfaces: inputs, toggle tracks, swatch wells |
| `ink` | `#1C211A` | `#E6E1E7`* | Text, controls, dried waveform trace (@88%) |
| `ink2` | `#4F584A` | `#A79FA8`* | Secondary text, timestamps, greyed values |
| `line` | `#9AA38D` | `#4A434B`* | 1px borders |
| `hair` / `hair2` | `#BEC6B1` / `#ACB49E` | `#3B353C` / `#443D45`* | Hairline rules |
| `tick` | `#47503F` | `#8F878F`* | Printed tick scales |
| `num` | `#2E332A` | `#D8D2D9`* | Mono numerals when distinguished from ink |
| `oxide` | `#C23B2B` | `#E05A3F` | **Data-ink only.** |

\* OBSIDIAN is the inverted variant: derive paper/register as field lightened steps, ink/ink2 as light-on-dark equivalents holding the same contrast ratios as LEDGER (ink on paper ≥ 12:1, ink2 ≥ 4.5:1). Other field variants (BONE, PLASTER `oxide #B93326`, GLACIER, LILAC) re-derive paper/register/hairlines from their field the same way.

**Oxide may mean, exhaustively:** the live bar at the waveform pen head (settling to ink within 120ms) and permanently-marked clipped bars. **Oxide may never mean:** errors, warnings, delete actions, CTAs, focus/selection, branding, the recording tray state, toggles, badges, or emphasis. Errors are ink words. Delete is an ink word. If a design wants red for anything but captured signal, the design is wrong.

---

## 7. Motion

Global: nothing moves at idle. Every animation below has a named triggering signal. All easing `cubic-bezier(0.2, 0, 0, 1)` unless noted. 60fps (waveform on canvas/WebGL, transforms and opacity only elsewhere) or the animation is cut.

| Animation | Trigger (signal) | Duration | Spec |
|---|---|---|---|
| HUD show | Hotkey down | 120ms | Opacity 0→1 |
| HUD hide | Inject/cancel/error timeout | 160ms | Opacity 1→0 |
| Paper scroll | Audio frames arriving | continuous | 80px/s, audio-clocked; stops the instant capture stops |
| Pen ink-dry | Each new amplitude bar | 120ms | Bar prints oxide 100% → ink 88% |
| Status word print | State machine transition | 120ms | New word at 100% opacity settles to resting opacity (ink-dry) |
| `PRINTED` stamp-press | Injection completed | 100ms | Word sinks 1px (translateY) and returns; linear down, eased up |
| Trace grey-out | Cancel | 140ms | Trace opacity 88%→40% |
| Theme swatch apply | User click | 160ms | Crossfade of CSS custom properties |
| History row expand | User click | 160ms | Height auto-animate; the only layout-affecting motion in the app |
| Hover states | Pointer enter | 0ms | Instant; tone change only, no motion, no reflow |
| Settings level lane | Live mic frames, section visible | continuous | Same rules as HUD lane |

**Idle audit list** — with no user input and no audio signal, verify all of the following are pixel-static: HUD (must be hidden entirely), tray icon, tray glyph in all states, Settings level lane (flat, not scrolling, when silent — hidden section renders nothing), toggles, theme swatches, history list, search input (native caret excepted), scrollbars, window chrome, the `PRINTED`/`DISCARDED` words after settling. No pulsing dots, no shimmer, no skeletons, no marquees, no animated spinners anywhere in the app — the loading state is a printed percentage.

---

## 8. Type scale

IBM Plex Sans + IBM Plex Mono. Per surface: exactly two sizes and two weights. Nothing else ships.

| Surface | Role | Face | Size / weight | Tracking / case |
|---|---|---|---|---|
| **HUD** | Status word | Plex Sans SemiBold | 11px | 0.18em, all-caps |
| | Elapsed / char count | Plex Mono Regular | 13px | normal |
| **Tray menu** | (native menu font — exempt) | — | — | — |
| **Settings** | Section titles + row labels | Plex Sans SemiBold | 11px | 0.18em, all-caps |
| | Values, inputs, chips, notes | Plex Mono Regular | 13px | normal (notes may render Plex Sans Regular 13px — counts as the surface's second size/weight pairing; pick one per row type and keep it) |
| **Settings header** | Wordmark | Plex Sans SemiBold | 16px | 0.18em, all-caps |
| **History** | Labels, headers, buttons | Plex Sans SemiBold | 11px | 0.18em, all-caps |
| | Timestamps, counts, transcripts (expanded) | Plex Mono Regular | 13px | normal |
| | Transcript preview (collapsed) | Plex Sans Regular | 13px | normal |

Line-height: 16px for 11px labels, 20px for 13px text (8px grid multiples/halves). No italics. No bold beyond SemiBold. No third size on any surface — the Settings header 16px wordmark is the single sanctioned exception and appears once.

---

## 9. Sounds

Optional (Settings → MICROPHONE → `AUDIO CUES` toggle, default on), and brief, dry, mechanical — the sound of an instrument engaging, not a chime. No melody, no reverb tail, no synth pads.

| Cue | Trigger | Character | Length |
|---|---|---|---|
| Start | Capture begins | Single low mechanical click — relay closing / typebar seat. ~1kHz-centered, dry | ≤60ms |
| Stop | Capture ends (release/toggle) | Same click, pitched slightly down — the release of the same mechanism | ≤60ms |
| Discard | Cancel | Two rapid dry ticks | ≤80ms |
| Error | Mic/model failure | Single dull thunk, lower and quieter than start | ≤100ms |

Rules: mono, −18 LUFS-ish quiet, played through the default output at fixed volume relative to system volume. No sound on injection success — the text appearing is the confirmation. Never a sound at idle, never repeating, never voice.

---

## Appendix: hard rules recap for implementers

1. Oxide = captured signal only. Grep the stylesheet for `oxide`; every hit must trace to the waveform or clipping marks.
2. HUD never takes focus. Test: caret position in Notepad is unchanged after a full dictation cycle.
3. Nothing animates without a named signal. Run the idle audit (§7) before every release.
4. 8px grid: every box dimension, padding, and position divisible by 8 (1px hairlines and 2px radii exempt).
5. Two sizes, two weights per surface. A third is a bug.
6. Grey out, never hide. Disabled states keep their footprint.
7. No layout reflow on hover, anywhere.