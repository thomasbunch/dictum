/// <reference types="vite/client" />
// DICTUM HUD overlay — DESIGN.md §5.6 (binding, every px), §7 (motion), §6 (copy).
// One canvas painter + a state switch, no framework.
// (the reference above pulls in ambient *.css side-effect import types)
import { listen } from "@tauri-apps/api/event";
import { api, MS_PER_BAR } from "../bindings";
import type { Config, HudEvent, HudState } from "../bindings";
import "@fontsource/ibm-plex-sans/600.css"; // state label
import "@fontsource/ibm-plex-mono/400.css"; // timer / % / hints

// Waveform constants (§5.6): 140px window ≈ last 4.2s → 112 bars at 37.5ms.
const LANE_W = 140;
const LANE_H = 32;
const PX_PER_BAR = LANE_W / (4200 / MS_PER_BAR); // 1.25px
const MID_Y = 16;
const AMP_PX = 15;
const CLIP_TICK_W = 2.4;
const CLIP_TICK_H = 5;

interface Bar { amp: number; clip: boolean; index: number }

let contentEl: HTMLElement;
let stateEl: HTMLElement;
let laneEl: HTMLCanvasElement;
let progressEl: HTMLElement;
let fillEl: HTMLElement;
let msgEl: HTMLElement;
let rightEl: HTMLElement;
let hintEl: HTMLElement;
let ctx: CanvasRenderingContext2D;

let palette = { ink: "#211F1A", dots: "#D0CCC0", oxide: "#C23B2B" };

let hudState: HudState = { k: "hidden" };
let bars: Bar[] = []; // newest last
let totalBars = 0;
let lastBarAt = 0; // performance.now() of the newest bar (drives sub-bar scroll)
let listening = false; // canvas redraws ONLY while listening (§7 M4)
let rafRunning = false;

function readPalette() {
  const cs = getComputedStyle(document.documentElement);
  const get = (name: string) => cs.getPropertyValue(name).trim();
  palette = { ink: get("--ink"), dots: get("--dots"), oxide: get("--oxide") };
}

function formatElapsed(ms: number): string {
  const totalSec = Math.floor(ms / 1000);
  return `${Math.floor(totalSec / 60)}:${String(totalSec % 60).padStart(2, "0")}`;
}

// --- waveform (pen trace) --------------------------------------------------

function setupCanvas() {
  const dpr = window.devicePixelRatio || 1;
  laneEl.width = Math.round(LANE_W * dpr);
  laneEl.height = Math.round(LANE_H * dpr);
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
}

/** Signed pen deflection for a bar: amplitude with an alternating-phase sign so
 * the trace oscillates about the zero line like a strip-chart pen. */
function deflect(b: Bar): number {
  return b.amp * AMP_PX * Math.sin(b.index * 1.9);
}

function render(now: number) {
  ctx.clearRect(0, 0, LANE_W, LANE_H);
  // Zero line, 1px dots.
  ctx.fillStyle = palette.dots;
  ctx.fillRect(0, MID_Y, LANE_W, 1);
  if (bars.length === 0) return;

  // Newest bar sits at the right edge; sub-bar scroll interpolates between
  // arrivals so the paper moves smoothly at 60fps while listening.
  const frac = listening ? Math.min(1, (now - lastBarAt) / MS_PER_BAR) : 0;
  const xOf = (i: number) =>
    LANE_W - 1 - (bars.length - 1 - i + frac) * PX_PER_BAR;

  ctx.strokeStyle = palette.ink;
  ctx.lineWidth = 1.6;
  ctx.lineJoin = "round";
  ctx.beginPath();
  let started = false;
  for (let i = 0; i < bars.length; i++) {
    const x = xOf(i);
    if (x < -PX_PER_BAR) continue;
    const y = MID_Y - deflect(bars[i]);
    if (started) ctx.lineTo(x, y);
    else { ctx.moveTo(x, y); started = true; }
  }
  ctx.stroke();

  // Clip ticks: 2.4px oxide, 5px tall, from the top edge (§5.6). Oxide appears
  // nowhere else.
  ctx.fillStyle = palette.oxide;
  for (let i = 0; i < bars.length; i++) {
    if (!bars[i].clip) continue;
    const x = xOf(i);
    if (x < 0) continue;
    ctx.fillRect(x - CLIP_TICK_W / 2, 0, CLIP_TICK_W, CLIP_TICK_H);
  }
}

function tick(now: number) {
  if (!listening) { rafRunning = false; return; }
  render(now);
  requestAnimationFrame(tick);
}
function ensureRaf() {
  if (rafRunning || !listening) return;
  rafRunning = true;
  requestAnimationFrame(tick);
}

function resetLane() {
  bars = [];
  totalBars = 0;
}

function onLevels(newBars: { amp: number; clip: boolean }[]) {
  if (!listening) return; // stray bars outside a live session
  for (const b of newBars) {
    bars.push({ amp: b.amp, clip: b.clip, index: totalBars++ });
  }
  const maxBars = Math.ceil(LANE_W / PX_PER_BAR) + 2;
  if (bars.length > maxBars) bars = bars.slice(-maxBars);
  lastBarAt = performance.now();
  rightEl.textContent = formatElapsed(totalBars * MS_PER_BAR);
  ensureRaf();
}

// --- state switch ----------------------------------------------------------

type Slot = "lane" | "progress" | "msg" | "none";

function layout(opts: {
  label: string;
  slot: Slot;
  right?: string;
  hint?: string;
  frozenLane?: boolean;
  msg?: string;
}) {
  stateEl.textContent = opts.label;
  laneEl.hidden = opts.slot !== "lane";
  laneEl.classList.toggle("frozen", !!opts.frozenLane);
  progressEl.hidden = opts.slot !== "progress";
  msgEl.hidden = opts.slot !== "msg";
  if (opts.slot === "msg") msgEl.textContent = opts.msg ?? "";
  rightEl.textContent = opts.right ?? "";
  hintEl.textContent = opts.hint ?? "";
  hintEl.hidden = !opts.hint;
  // M3: content crossfade, 80ms linear.
  contentEl.classList.remove("swap");
  void contentEl.offsetWidth; // restart the animation
  contentEl.classList.add("swap");
}

function onState(next: HudState) {
  const prev = hudState;
  hudState = next;

  const nowHidden = next.k === "hidden";
  if (nowHidden !== (prev.k === "hidden")) {
    document.body.classList.toggle("show", !nowHidden);
  }

  listening = next.k === "listening" || next.k === "confirm_discard";

  switch (next.k) {
    case "hidden":
      break;
    case "loading_model":
      // A fresh session may start here (cold start) — wipe the old trace.
      resetLane();
      layout({ label: "WARMING UP", slot: "progress", right: `${next.pct}%` });
      fillEl.style.transform = `scaleX(${next.pct / 100})`;
      break;
    case "listening":
      // Same continuous recording when returning from HOLD ON or WARMING UP —
      // keep the trace; anything else is a new take.
      if (prev.k !== "confirm_discard" && prev.k !== "loading_model") resetLane();
      layout({
        label: "LISTENING",
        slot: "lane",
        right: formatElapsed(totalBars * MS_PER_BAR),
        hint: "ESC ✕",
      });
      render(performance.now());
      ensureRaf();
      break;
    case "transcribing":
      layout({
        label: "PRINTING…",
        slot: "lane",
        frozenLane: true,
        right: formatElapsed(totalBars * MS_PER_BAR),
      });
      render(performance.now()); // one static frame, frozen at 45% via CSS
      break;
    case "reformatting":
      // LLM rewrite in flight (seconds on CPU). Freeze the trace like PRINTING…
      // and keep the elapsed clock; the paper has already stopped.
      layout({
        label: "REFORMATTING",
        slot: "lane",
        frozenLane: true,
        right: formatElapsed(totalBars * MS_PER_BAR),
      });
      render(performance.now());
      break;
    case "injected":
      layout({ label: "PRINTED", slot: "none", right: `${next.chars} CH` });
      break;
    case "cancelled":
      layout({ label: "KILLED", slot: "none" });
      break;
    case "confirm_discard":
      layout({
        label: "HOLD ON",
        slot: "msg",
        msg: "ESC AGAIN KILLS THE TAKE",
        right: formatElapsed(totalBars * MS_PER_BAR),
      });
      break;
    case "error":
      layout({ label: next.label, slot: "msg", msg: next.detail });
      break;
  }
}

function onHudEvent(e: HudEvent) {
  if (e.t === "levels") onLevels(e.bars);
  else onState(e.s);
}

// --- theme -----------------------------------------------------------------

function applyTheme(theme: string) {
  document.documentElement.className = `theme-${theme.toLowerCase()}`;
  readPalette();
  if (!rafRunning) render(performance.now()); // repaint the static frame
}

// --- boot ------------------------------------------------------------------

function init() {
  const host = document.getElementById("hud")!;
  host.innerHTML = `
    <div class="hud">
      <div class="square"></div>
      <div class="content">
        <div class="state label"></div>
        <canvas class="lane" hidden></canvas>
        <div class="progress" hidden><div class="fill"></div></div>
        <div class="msg value-sm" hidden></div>
        <div class="spacer"></div>
        <div class="right hud-timer"></div>
        <div class="hint value-xs" hidden></div>
      </div>
    </div>`;
  contentEl = host.querySelector(".content")!;
  stateEl = host.querySelector(".state")!;
  laneEl = host.querySelector(".lane")!;
  progressEl = host.querySelector(".progress")!;
  fillEl = host.querySelector(".fill")!;
  msgEl = host.querySelector(".msg")!;
  rightEl = host.querySelector(".right")!;
  hintEl = host.querySelector(".hint")!;
  ctx = laneEl.getContext("2d")!;
  setupCanvas();
  readPalette();

  api.getConfig().then((c: Config) => applyTheme(c.theme));
  listen<Config>("config://changed", (e) => applyTheme(e.payload.theme));
  // Retry until the backend accepts the subscription: the webview can finish
  // loading before .setup() has managed AppState, and a HUD that never
  // subscribes is invisible forever (observed). Errors surface in the window
  // title (debug channel — the overlay has no visible chrome of its own).
  subscribeWithRetry(0);
}

function subscribeWithRetry(attempt: number) {
  api.subscribeHud(onHudEvent).then(
    () => { document.title = "DICTUM"; },
    (err) => {
      document.title = `DICTUM SUB-ERR ${String(err).slice(0, 60)}`;
      if (attempt < 20) setTimeout(() => subscribeWithRetry(attempt + 1), 250);
    },
  );
}

window.addEventListener("error", (e) => { document.title = `DICTUM JS-ERR ${e.message?.slice(0, 60)}`; });
init();
