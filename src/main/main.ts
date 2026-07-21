/// <reference types="vite/client" />
// DICTUM main window — DESIGN.md §5.1 (shell), §5.5 (first run).
// Titlebar + masthead + view (TAPE | WORDS | SETUP) + footer. No framework.
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getVersion } from "@tauri-apps/api/app";
import { api } from "../bindings";
import type { Config, HistoryRecord, HudState, ModelInfo, ModelStatus, Retention } from "../bindings";
import { h, initTheme, mountError, debounce } from "../shared";
import { renderTape } from "./tape";
import { renderWords } from "./words";
import { renderSetup } from "./setup";

export type View = "tape" | "words" | "setup";

export interface Ctx {
  config: Config;
  models: ModelInfo[];
  modelStatus: ModelStatus;
  devices: string[];
  records: HistoryRecord[];
  totalLines: number;
  /** Newest record id after a live injection — its row ink-dries (M6). */
  freshId: number | null;
  version: string;
  persist(): void;
  persistNow(): void;
  reloadHistory(search: string | null): Promise<void>;
  renderMasthead(): void;
  renderView(): void;
  updateFooter(): void;
  /** Set by SETUP while visible: live mic meter fill + model card updater. */
  meterFill: HTMLElement | null;
  modelCardUpdate: (() => void) | null;
}

// ---------------------------------------------------------------------------
// Shared formatting helpers (used by views too)
// ---------------------------------------------------------------------------
const MOD_DISPLAY: Record<string, string> = {
  Ctrl: "CTRL", Control: "CTRL", CmdOrCtrl: "CTRL",
  Alt: "ALT", Option: "ALT", Shift: "SHIFT",
  Super: "WIN", Meta: "WIN", Cmd: "WIN", Command: "WIN", Win: "WIN",
};

/** "Ctrl+Alt+Space" -> ["CTRL","ALT","SPACE"] for keycap chips. */
export function chordTokens(chord: string): string[] {
  return chord.split("+").map((t) => {
    const p = t.trim();
    if (MOD_DISPLAY[p]) return MOD_DISPLAY[p];
    if (p.length === 4 && p.startsWith("Key")) return p.slice(3).toUpperCase();
    if (p.length === 6 && p.startsWith("Digit")) return p.slice(5);
    return p.toUpperCase();
  });
}

export function chordDisplay(chord: string): string {
  return chordTokens(chord).join("+");
}

export function retentionLabel(r: Retention): string {
  return { keepNothing: "NOTHING", hours24: "24 H", days7: "7 D", days30: "30 D", forever: "FOREVER" }[r];
}

/** The model the config points at (falls back to the first registry entry). */
export function activeModel(c: Ctx): ModelInfo | undefined {
  return c.models.find((m) => m.id === c.config?.modelId) ?? c.models[0];
}

/** Download flow states (§5.4.3), reused by the first-run masthead and the
 * SETUP model cards. Renders into `slot`; calls onDone after READY. */
export function runDownloadFlow(id: string, slot: HTMLElement, sizeMb: number, onDone: () => void): void {
  let mbDone = 0;
  const left = h("span");
  const right = h("span");
  const fill = h("div", { class: "fill" });
  const bar = h("div", { class: "dl-bar" }, [fill]);
  const box = h("div", { class: "dl" }, [h("div", { class: "dl-line" }, [left, right]), bar]);
  slot.innerHTML = "";
  slot.append(box);

  const start = () => {
    left.textContent = "FETCHING…";
    right.textContent = `0% OF ${sizeMb} MB`;
    bar.className = "dl-bar";
    fill.style.transform = "scaleX(0)";
    api
      .downloadModel(id, (p) => {
        if (p.t === "progress") {
          mbDone = p.mb_done;
          right.textContent = `${p.pct}% OF ${p.mb_total} MB`;
          fill.style.transform = `scaleX(${p.pct / 100})`;
        } else if (p.t === "verifying") {
          left.textContent = "PROOFING…";
          right.textContent = "SHA-256";
          bar.className = "dl-bar striped";
        } else if (p.t === "done") {
          left.textContent = "READY.";
          right.textContent = "● LOADED";
          bar.className = "dl-bar";
          fill.style.transform = "scaleX(1)";
          onDone();
        } else {
          fail(p.error);
        }
      })
      .catch((e: unknown) => fail(String(e)));
  };
  const fail = (error: string) => {
    console.error("model download failed:", error);
    left.textContent = `FAILED — THE WIRE BROKE AT ${mbDone} MB.`;
    right.innerHTML = "";
    right.append(h("button", { class: "btn", onclick: start }, "RETRY"));
    bar.className = "dl-bar";
    fill.style.transform = "scaleX(0)";
  };
  start();
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------
let currentView: View = "tape";
let mastheadEl: HTMLElement;
let viewEl: HTMLElement;
let footerMetaEl: HTMLElement;
let liveEl: HTMLElement;
let navButtons: Partial<Record<View, HTMLButtonElement>> = {};
let saveTimer: ReturnType<typeof setTimeout> | undefined;
let searchQuery: string | null = null; // owned by tape.ts via ctx.reloadHistory
let testActive = false;

const ctx: Ctx = {
  config: undefined as unknown as Config,
  models: [],
  modelStatus: { k: "missing" },
  devices: [],
  records: [],
  totalLines: 0,
  freshId: null,
  version: "",
  persist() {
    clearTimeout(saveTimer);
    saveTimer = setTimeout(() => void api.setConfig(ctx.config), 150);
  },
  persistNow() {
    clearTimeout(saveTimer);
    void api.setConfig(ctx.config);
  },
  async reloadHistory(search: string | null) {
    searchQuery = search;
    ctx.records = await api.historyList(search);
    ctx.totalLines = await api.historyCount();
  },
  renderMasthead,
  renderView,
  updateFooter,
  meterFill: null,
  modelCardUpdate: null,
};

// ---------------------------------------------------------------------------
// Masthead (§5.1): full on TAPE (status + counters + keycaps, or first run),
// slim on WORDS/SETUP.
// ---------------------------------------------------------------------------
function statusLine(): string {
  const model = (() => {
    switch (ctx.modelStatus.k) {
      case "ready": return `MODEL LOADED · ${activeModel(ctx)?.display ?? "PARAKEET-TDT 0.6B V2"}`;
      case "loading": return `MODEL LOADING · ${ctx.modelStatus.pct}%`;
      case "unloaded": return "MODEL NOT LOADED (IDLE)";
      case "missing": return "NO MODEL ON THIS MACHINE";
      case "error": return "MODEL ERROR";
    }
  })();
  const mic = ctx.devices.length
    ? `MIC OK · ${(ctx.config.inputDevice ?? "SYSTEM DEFAULT").toUpperCase()}`
    : "NO MICROPHONE";
  return `${model} — ${mic} — LOCAL ONLY · ZERO EGRESS`;
}

function startCaption(): string {
  switch (ctx.config.hotkeyMode) {
    case "hold": return "HOLD TO SPEAK — THE TAPE PRINTS HERE";
    case "toggle": return "TAP TO SPEAK — THE TAPE PRINTS HERE";
    case "both": return "TAP OR HOLD — THE TAPE PRINTS HERE";
  }
}

function isToday(ts: number): boolean {
  const d = new Date(ts), n = new Date();
  return d.getFullYear() === n.getFullYear() && d.getMonth() === n.getMonth() && d.getDate() === n.getDate();
}

function buildCounters(): HTMLElement {
  const today = ctx.records.filter((r) => isToday(r.ts));
  const words = today.reduce((n, r) => n + r.text.trim().split(/\s+/).filter(Boolean).length, 0);
  const days = new Set(ctx.records.map((r) => new Date(r.ts).toDateString())).size;
  const counter = (value: string, cap: string) =>
    h("div", { class: "counter" }, [
      h("div", { class: "value-lg" }, value),
      h("div", { class: "microlabel cap" }, cap),
    ]);
  const wrap = h("div");
  wrap.append(
    h("div", { class: "counters" }, [
      counter(words.toLocaleString("en-US"), "WORDS TODAY"),
      counter(String(today.length), "PRINTED"),
      counter(String(days), "DAYS RUNNING"),
    ]),
  );
  // BY APP — driven by per-record exe data (§5.1 proposal).
  if (ctx.records.length > 0) {
    const byExe = new Map<string, number>();
    for (const r of ctx.records) {
      const k = r.exe ?? "other";
      byExe.set(k, (byExe.get(k) ?? 0) + 1);
    }
    const sorted = [...byExe.entries()].sort((a, b) => b[1] - a[1]);
    const top = sorted.slice(0, 4);
    const rest = sorted.slice(4).reduce((n, [, c]) => n + c, 0);
    const pct = (c: number) => Math.round((c / ctx.records.length) * 100);
    const parts = top.map(([exe, c]) => `${exe} ${pct(c)}`);
    if (rest > 0) parts.push(`other ${pct(rest)}`);
    wrap.append(h("div", { class: "value-sm byapp" }, `BY APP ${parts.join(" · ")} %`));
  }
  return wrap;
}

function buildStartPath(): HTMLElement {
  const caption = h("div", { class: "microlabel start-cap" }, startCaption());
  const scratch = h("input", {
    class: "field-input scratch",
    type: "text",
    "aria-label": "Test dictation scratch line",
  });
  scratch.hidden = true;

  const stopTest = () => {
    if (!testActive) return;
    testActive = false;
    void api.toggleDictation();
    scratch.hidden = true;
    caption.hidden = false;
  };
  scratch.addEventListener("blur", stopTest);
  scratch.addEventListener("keydown", (e) => {
    if (e.key === "Escape") stopTest();
  });

  // Keycaps are the prominent start path and mic test (§5.1): clicking starts
  // a test dictation targeted at the scratch line.
  const keycaps = h(
    "button",
    {
      class: "keycaps",
      "aria-label": "Start test dictation",
      onclick: () => {
        if (testActive) { stopTest(); return; }
        testActive = true;
        caption.hidden = true;
        scratch.hidden = false;
        scratch.value = "";
        scratch.focus();
        void api.toggleDictation();
      },
    },
    chordTokens(ctx.config.hotkey).map((t) => h("span", { class: "keycap" }, t)),
  );
  return h("div", { class: "startpath" }, [keycaps, caption, scratch]);
}

function renderMasthead(): void {
  testActive = false;
  mastheadEl.innerHTML = "";
  mastheadEl.classList.toggle("slim", currentView !== "tape");

  const nav = h("nav", { class: "nav", "aria-label": "View" });
  navButtons = {};
  for (const [v, label] of [["tape", "TAPE"], ["words", "WORDS"], ["setup", "SETUP"]] as [View, string][]) {
    const b = h("button", { "aria-current": String(v === currentView), onclick: () => setView(v) }, label);
    navButtons[v] = b;
    nav.append(b);
  }
  mastheadEl.append(h("div", { class: "mast-top" }, [h("span", { class: "wordmark" }, "DICTUM"), nav]));

  if (currentView !== "tape") return;

  mastheadEl.append(h("div", { class: "value status-line" }, statusLine()));

  const present = ctx.models[0]?.present ?? false;
  if (!present) {
    // First run (§5.5): the masthead carries the fetch.
    const active = activeModel(ctx);
    const sizeMb = active?.sizeMb ?? 610;
    const slot = h("div");
    const btn = h(
      "button",
      {
        class: "btn fetch-btn",
        onclick: () =>
          runDownloadFlow(active?.id ?? "parakeet-tdt-0.6b-v2-int8", slot, sizeMb, () => void refreshModel()),
      },
      `FETCH THE MODEL · ${sizeMb} MB`,
    );
    slot.append(btn);
    mastheadEl.append(
      h("div", { class: "mast-body" }, [
        h("div", { class: "firstrun" }, [
          slot,
          h("div", { class: "value-sm note" }, [
            `${active?.display ?? "PARAKEET-TDT 0.6B V2"} · SHERPA-ONNX · CPU`,
            h("br"),
            "THE ONLY DOWNLOAD DICTUM WILL EVER MAKE. AFTER THIS, THE WIRE GOES DARK.",
          ]),
        ]),
      ]),
    );
    return;
  }

  mastheadEl.append(h("div", { class: "mast-body" }, [buildCounters(), buildStartPath()]));
}

// ---------------------------------------------------------------------------
// Footer (§5.1): the privacy state is printed on every view.
// ---------------------------------------------------------------------------
function updateFooter(): void {
  const t = ctx.config.keepTranscripts ? retentionLabel(ctx.config.retention) : "OFF";
  footerMetaEl.textContent = `TRANSCRIPTS · ${t} / AUDIO · OFF`;
}

// ---------------------------------------------------------------------------
// Views
// ---------------------------------------------------------------------------
function renderView(): void {
  ctx.meterFill = null;
  ctx.modelCardUpdate = null;
  viewEl.innerHTML = "";
  if (currentView === "tape") renderTape(ctx, viewEl);
  else if (currentView === "words") renderWords(ctx, viewEl);
  else renderSetup(ctx, viewEl);
}

function setView(v: View): void {
  if (v === currentView) {
    for (const [key, b] of Object.entries(navButtons)) b.setAttribute("aria-current", String(key === v));
    return;
  }
  currentView = v;
  searchQuery = null;
  renderMasthead();
  renderView();
}

// ---------------------------------------------------------------------------
// Model / HUD / history wiring
// ---------------------------------------------------------------------------
async function refreshModel(): Promise<void> {
  ctx.models = await api.modelInfo();
  ctx.modelStatus = await api.getModelStatus();
  renderMasthead();
  ctx.modelCardUpdate?.();
}

function announce(s: HudState): void {
  const text = (() => {
    switch (s.k) {
      case "hidden": return "";
      case "loading_model": return `WARMING UP · ${s.pct}%`;
      case "listening": return "LISTENING";
      case "transcribing": return "PRINTING";
      case "injected": return `PRINTED · ${s.chars} CH`;
      case "cancelled": return "KILLED";
      case "confirm_discard": return "HOLD ON — ESC AGAIN KILLS THE TAKE";
      case "error": return `${s.label} — ${s.detail}`;
    }
  })();
  if (text) liveEl.textContent = text;
}

function subscribeHudWithRetry(attempt: number): void {
  api
    .subscribeHud((e) => {
      if (e.t === "levels") {
        if (ctx.meterFill && e.bars.length > 0) {
          const peak = Math.max(...e.bars.map((b) => b.amp));
          ctx.meterFill.style.width = `${Math.min(100, Math.round(peak * 100))}%`;
        }
      } else {
        announce(e.s);
        if (e.s.k !== "listening" && e.s.k !== "confirm_discard" && ctx.meterFill) {
          ctx.meterFill.style.width = "0%";
        }
      }
    })
    .catch(() => {
      if (attempt < 20) setTimeout(() => subscribeHudWithRetry(attempt + 1), 250);
    });
}

// ---------------------------------------------------------------------------
// Boot
// ---------------------------------------------------------------------------
async function main() {
  const app = document.getElementById("app");
  if (!app) return;

  const win = getCurrentWindow();
  const titlebar = h("div", { class: "titlebar", "data-tauri-drag-region": true }, [
    h("span", { class: "tb-name" }, "DICTUM"),
    h("div", { class: "tb-caption" }, [
      h("button", { "aria-label": "Minimize", onclick: () => void win.minimize() }, "─"),
      h("button", { "aria-label": "Maximize", onclick: () => void win.toggleMaximize() }, "▢"),
      h("button", { "aria-label": "Close", onclick: () => void win.close() }, "✕"),
    ]),
  ]);

  mastheadEl = h("div", { class: "masthead" });
  viewEl = h("div", { class: "view" });
  footerMetaEl = h("span", { class: "value-sm meta" });
  liveEl = h("div", { class: "visually-hidden", role: "status", "aria-live": "polite" });
  const footer = h("div", { class: "footer" }, [
    h("span", { class: "microlabel left" }, "NOTHING LEAVES THIS MACHINE"),
    footerMetaEl,
  ]);
  app.append(titlebar, mastheadEl, viewEl, footer, liveEl);

  ctx.config = await initTheme((cfg) => {
    // In-place adopt: views hold references to ctx.config and persist whole-
    // object; a re-render here would drop input focus mid-edit.
    Object.assign(ctx.config, cfg);
    updateFooter();
  });

  const [models, status, devices, version] = await Promise.all([
    api.modelInfo(),
    api.getModelStatus(),
    api.listInputDevices(),
    getVersion().catch(() => "0.1.0"),
  ]);
  ctx.models = models;
  ctx.modelStatus = status;
  ctx.devices = devices;
  ctx.version = version;
  await ctx.reloadHistory(null);

  renderMasthead();
  renderView();
  updateFooter();

  await listen<string>("nav://view", (e) => {
    const v = e.payload as View;
    if (v === "tape" || v === "words" || v === "setup") setView(v);
  });
  await listen<ModelStatus>("model://status", (e) => {
    ctx.modelStatus = e.payload;
    if (currentView === "tape") renderMasthead();
    ctx.modelCardUpdate?.();
  });
  // A new line printed (§5.2): reload; the tape re-renders with ink-dry.
  const onHistoryChanged = debounce(() => {
    void ctx.reloadHistory(searchQuery).then(() => {
      ctx.freshId = ctx.records[0]?.id ?? null;
      if (currentView === "tape") {
        renderMasthead();
        renderView();
      }
    });
  }, 150);
  await listen("history://changed", onHistoryChanged);

  subscribeHudWithRetry(0);
}

main().catch(mountError);
