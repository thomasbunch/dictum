/// <reference types="vite/client" />
// DICTUM HUD overlay — one canvas painter + a state switch, no framework.
// (the reference above pulls in ambient *.css side-effect import types; tsconfig.json has no global "types" entry for vite/client)
// DESIGN.md §2 (geometry/lane, binding every px), §7 (motion), §1.2 (copy).
import { listen } from "@tauri-apps/api/event";
import { api, MS_PER_BAR } from "../bindings";
import type { Config, HudEvent, HudState } from "../bindings";
import "@fontsource/ibm-plex-sans/600.css"; // status word (label style)
import "@fontsource/ibm-plex-mono/400.css"; // elapsed / char count / pct

const EASE = "cubic-bezier(0.2, 0, 0, 1)";
const DRY_MS = 120; // pen ink-dry: oxide -> ink@88%
const GREYOUT_MS = 140; // cancel trace grey-out: ink@88% -> ink2@40%
const BAR_PITCH = 3; // 2px bar + 1px gap
const BAR_W = 2;
// DESIGN.md pins bars at ink@88% resting; it never pins a numeric resting
// opacity for status-word ink-dry. Picked a subtle value; revise if design adds one.
const WORD_RESTING_OPACITY = 0.92;

type RGB = [number, number, number];
interface Bar { amp: number; clip: boolean; bornAt: number }

let wordEl: HTMLElement;
let subEl: HTMLElement;
let canvas: HTMLCanvasElement;
let ctx: CanvasRenderingContext2D;

let laneW = 0, laneH = 0, headX = 0;
let palette: { oxide: RGB; ink: RGB; ink2: RGB; tick: RGB };

let hudState: HudState = { k: "hidden" };
let listeningLike = false; // Listening or ConfirmDiscard: audio still capturing, paper still scrolling
let bars: Bar[] = []; // index 0 = newest (at pen head), increasing index = further left/older
let totalBars = 0; // for elapsed = totalBars * MS_PER_BAR
let paperPx = 0; // cumulative scroll distance, drives tick phase; frozen when not listeningLike
let greyOutStart: number | null = null;
let loopRunning = false;
let wordAnim: Animation | null = null;
let stampAnim: Animation | null = null;

function hexToRgb(hex: string): RGB {
  const n = parseInt(hex.trim().replace("#", ""), 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}
function rgba(c: RGB, a: number): string {
  return `rgba(${c[0]},${c[1]},${c[2]},${a})`;
}
function lerp(a: number, b: number, t: number): number {
  return a + (b - a) * t;
}
function lerpColor(c1: RGB, a1: number, c2: RGB, a2: number, t: number): string {
  return rgba(
    [lerp(c1[0], c2[0], t), lerp(c1[1], c2[1], t), lerp(c1[2], c2[2], t)],
    lerp(a1, a2, t),
  );
}

function readPalette() {
  const cs = getComputedStyle(document.documentElement);
  const get = (name: string) => hexToRgb(cs.getPropertyValue(name));
  palette = { oxide: get("--oxide"), ink: get("--ink"), ink2: get("--ink2"), tick: get("--tick") };
}

function formatElapsed(ms: number): string {
  const totalSec = Math.floor(ms / 1000);
  const m = Math.floor(totalSec / 60);
  const s = totalSec % 60;
  return `${m}:${String(s).padStart(2, "0")}`;
}

// --- status column -----------------------------------------------------

function setWord(text: string, stamp = false) {
  wordEl.textContent = text;
  wordAnim?.cancel();
  wordAnim = wordEl.animate(
    [{ opacity: 1 }, { opacity: WORD_RESTING_OPACITY }],
    { duration: DRY_MS, easing: EASE, fill: "forwards" },
  );
  if (stamp) {
    stampAnim?.cancel();
    stampAnim = wordEl.animate(
      [
        { transform: "translateY(0)", offset: 0, easing: "linear" },
        { transform: "translateY(1px)", offset: 0.5, easing: EASE },
        { transform: "translateY(0)", offset: 1 },
      ],
      { duration: 100 },
    );
  }
}
function setSub(text: string) {
  subEl.textContent = text;
}

// --- lane (chart-recorder) ----------------------------------------------

function setupCanvas() {
  const rect = canvas.getBoundingClientRect();
  laneW = rect.width;
  laneH = rect.height;
  headX = laneW * 0.75; // pen head fixed at 75% of lane width
  const dpr = window.devicePixelRatio || 1;
  canvas.width = Math.round(laneW * dpr);
  canvas.height = Math.round(laneH * dpr);
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
}

function drawTickAt(x: number, h: number) {
  ctx.fillRect(Math.round(x), laneH - h, 1, h);
}
function drawTicks() {
  ctx.fillStyle = rgba(palette.tick, 1);
  const phaseMinor = ((paperPx % 8) + 8) % 8;
  for (let x = headX - phaseMinor; x > -8; x -= 8) drawTickAt(x, 2);
  for (let x = headX - phaseMinor + 8; x < laneW + 8; x += 8) drawTickAt(x, 2);
  const phaseMajor = ((paperPx % 40) + 40) % 40;
  for (let x = headX - phaseMajor; x > -40; x -= 40) drawTickAt(x, 4);
  for (let x = headX - phaseMajor + 40; x < laneW + 40; x += 40) drawTickAt(x, 4);
}
function drawZeroLine() {
  ctx.fillStyle = rgba(palette.ink2, 0.14);
  ctx.fillRect(0, laneH / 2, laneW, 1);
}
function drawBars(now: number) {
  const centerY = laneH / 2;
  const maxHalf = centerY;
  for (let i = 0; i < bars.length; i++) {
    const bar = bars[i];
    const x = headX - i * BAR_PITCH - BAR_W;
    if (x + BAR_W < 0) break; // everything further (higher i) is off-screen too
    let color: string;
    let half: number;
    if (bar.clip) {
      color = rgba(palette.oxide, 1);
      half = maxHalf; // clipped bars stay oxide, full lane height, forever
    } else {
      half = Math.min(maxHalf, bar.amp * maxHalf);
      if (greyOutStart !== null) {
        const t = Math.min(1, (now - greyOutStart) / GREYOUT_MS);
        color = lerpColor(palette.ink, 0.88, palette.ink2, 0.4, t);
      } else {
        const age = now - bar.bornAt;
        color = age < DRY_MS
          ? lerpColor(palette.oxide, 1, palette.ink, 0.88, Math.max(0, age / DRY_MS))
          : rgba(palette.ink, 0.88);
      }
    }
    ctx.fillStyle = color;
    ctx.fillRect(x, centerY - half, BAR_W, half * 2);
  }
}
function render(now: number) {
  ctx.clearRect(0, 0, laneW, laneH);
  drawTicks();
  drawZeroLine();
  drawBars(now);
}

function trimBars() {
  const maxIndex = Math.ceil(headX / BAR_PITCH) + 2;
  if (bars.length > maxIndex) bars.length = maxIndex;
}
function resetLane() {
  bars = [];
  paperPx = 0;
  totalBars = 0;
  greyOutStart = null;
}
// Force any in-flight ink-dry to its resting state and paint one static frame
// (Transcribing/Injected/idle-error: "nothing animates" per DESIGN §7 idle audit).
function freezeBars() {
  const settled = performance.now() - DRY_MS - 1;
  for (const b of bars) b.bornAt = settled;
  greyOutStart = null;
  render(performance.now());
}
function clearLaneStatic() {
  bars = [];
  paperPx = 0;
  render(performance.now());
}

function ensureLoop() {
  if (loopRunning) return;
  loopRunning = true;
  requestAnimationFrame(tick);
}
function tick(now: number) {
  render(now);
  const greyDone = greyOutStart !== null && now - greyOutStart >= GREYOUT_MS;
  if (greyDone) greyOutStart = null;
  const dryInFlight = bars.some((b) => now - b.bornAt < DRY_MS);
  const shouldContinue = listeningLike || greyOutStart !== null || dryInFlight;
  if (shouldContinue) requestAnimationFrame(tick);
  else loopRunning = false;
}

// --- HUD protocol ---------------------------------------------------------

function onLevels(newBars: { amp: number; clip: boolean }[]) {
  if (!listeningLike) return; // defensive: ignore stray bars outside a live session
  const now = performance.now();
  const k = newBars.length;
  for (let idx = 0; idx < k; idx++) {
    const b = newBars[idx];
    bars.unshift({ amp: b.amp, clip: b.clip, bornAt: now - (k - 1 - idx) * MS_PER_BAR });
  }
  totalBars += k;
  paperPx += k * BAR_PITCH;
  trimBars();
  setSub(formatElapsed(totalBars * MS_PER_BAR));
  ensureLoop();
}

function fadeBody(show: boolean) {
  document.body.classList.toggle("show", show);
}

function onState(next: HudState) {
  const prev = hudState;
  hudState = next;

  const wasHidden = prev.k === "hidden";
  const nowHidden = next.k === "hidden";
  if (nowHidden !== wasHidden) fadeBody(!nowHidden);

  listeningLike = next.k === "listening" || next.k === "confirm_discard";

  switch (next.k) {
    case "hidden":
      break;
    case "loading_model":
      clearLaneStatic();
      setWord("LOADING MODEL");
      setSub(`${next.pct}%`);
      break;
    case "listening":
      if (prev.k !== "confirm_discard") resetLane(); // same continuous recording, don't wipe the trace
      setWord("LISTENING");
      setSub(formatElapsed(totalBars * MS_PER_BAR));
      ensureLoop();
      break;
    case "confirm_discard":
      setWord("ESC AGAIN TO DISCARD");
      ensureLoop();
      break;
    case "transcribing":
      freezeBars();
      setWord("PRINTING");
      break;
    case "injected":
      freezeBars();
      setWord("PRINTED", true);
      setSub(`${next.chars} CH`);
      break;
    case "cancelled":
      greyOutStart = performance.now();
      setWord("DISCARDED");
      setSub("");
      ensureLoop();
      break;
    case "error":
      clearLaneStatic();
      setWord(next.msg); // exact copy from Rust, rendered in ink, never oxide
      setSub("");
      break;
  }
}

function onHudEvent(e: HudEvent) {
  if (e.t === "levels") onLevels(e.bars);
  else onState(e.s);
}

// --- theme -----------------------------------------------------------------

function applyTheme(theme: string) {
  document.documentElement.dataset.field = theme;
  readPalette();
  if (!loopRunning) render(performance.now()); // repaint the static frame in the new colors
}

// --- boot --------------------------------------------------------------

function init() {
  const host = document.getElementById("hud")!;
  host.innerHTML = `
    <div class="pill">
      <div class="status">
        <div class="word label"></div>
        <div class="sub mono"></div>
      </div>
      <div class="hair"></div>
      <canvas class="lane"></canvas>
    </div>`;
  wordEl = host.querySelector(".word")!;
  subEl = host.querySelector(".sub")!;
  canvas = host.querySelector(".lane")!;
  ctx = canvas.getContext("2d")!;
  setupCanvas();
  readPalette();

  api.getConfig().then((c: Config) => applyTheme(c.theme));
  listen<Config>("config://changed", (e) => applyTheme(e.payload.theme));
  api.subscribeHud(onHudEvent);
}
init();
