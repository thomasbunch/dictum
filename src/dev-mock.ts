// Browser-only harness: `vite` + `main.html?mock` / `overlay.html?mock` renders
// the UI in a plain browser with fake IPC so design work never needs the Rust
// shell. Dead code in production: the entry points import it only under
// import.meta.env.DEV, and Vite drops the branch.
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import type { Config, HistoryRecord, HudEvent, ModelInfo } from "./bindings";

const DAY = 86_400_000;
const now = Date.now();
const params = new URLSearchParams(location.search);

// Deterministic pseudo-envelope so fixtures stay short.
function env(n: number, seed: number, peak = 0.8): number[] {
  const out: number[] = [];
  let x = seed;
  for (let i = 0; i < n; i++) {
    x = (x * 9301 + 49297) % 233280;
    const r = x / 233280;
    const shape = Math.sin((i / n) * Math.PI) * 0.6 + 0.4;
    out.push(Math.min(1, r * peak * shape + 0.05));
  }
  return out;
}

const TEXTS: [string, string | null, string, number][] = [
  ["Can you refactor the coordinator so the reformat step runs on its own worker thread and the HUD state machine never blocks on it.", "code.exe", "pasted", 9200],
  ["Add a test for the guardrail that checks a dropped negation flips the polarity gate.", "code.exe", "pasted", 5100],
  ["ok so the thing I want is a scrollbar that matches the paper, not the chrome default, and it should be thin, no arrows.", "claude.exe", "pasted", 8400],
  ["git commit dash m fix the radio input escaping the view overflow", "windowsterminal.exe", "typed", 3900],
  ["Three things for tomorrow. Ship the vulkan build. Write the changelog. Ping the winget reviewer.", "notion.exe", "pasted", 6800],
  ["what is the difference between DwmGetWindowAttribute extended frame bounds and GetWindowRect on windows eleven", "chrome.exe", "pasted", 6100],
  ["Move the sprocket margin to the left of the day rule and keep the hairline running under the timestamp.", "figma.exe", "pasted", 5600],
  ["yeah go with the second option, the one with the inline undo, and drop the modal entirely", "slack.exe", "pasted", 4700],
  ["Open the tape, search for vulkan, and strike every line from before the merge.", "code.exe", "pasted", 4300],
  ["Remind me to renew the code signing cert before the fourteenth.", "outlook.exe", "pasted", 3300],
  ["The model card should read loaded when the recognizer is warm and standby when it is on disk but cold.", "code.exe", "pasted", 6400],
  ["short one", "notepad.exe", "typed", 900],
];

const records: HistoryRecord[] = TEXTS.map(([text, exe, method, durMs], i) => {
  const dayOff = i < 4 ? 0 : i < 8 ? 1 : 3;
  const ts = now - dayOff * DAY - i * 47 * 60_000 - 120_000;
  return {
    id: 300 - i,
    ts,
    raw: text,
    text,
    exe,
    durMs,
    clipped: i === 2 || i === 6,
    envelope: env(48 + (i % 3) * 8, 17 + i, i === 2 || i === 6 ? 1 : 0.8),
    method,
  };
});

let config: Config = {
  hotkey: "Ctrl+Alt+D",
  hotkeyMode: "hold",
  inputDevice: null,
  audioCues: true,
  unloadOnIdle: false,
  theme: (params.get("theme")?.toUpperCase() as Config["theme"]) || "BONE",
  keepTranscripts: true,
  retention: "days7",
  vocabulary: ["Parakeet", "sherpa-onnx", "Tauri", "llama.cpp", "Vulkan"],
  replacements: [
    { heard: "open brace", printed: "{" },
    { heard: "close brace", printed: "}" },
    { heard: "arrow", printed: "=>" },
  ],
  removeFillers: true,
  appOverrides: {
    "mstsc.exe": { backend: "sendInputUnicode", pasteShortcut: null, chunkDelayMs: null },
    "windowsterminal.exe": { backend: null, pasteShortcut: "ctrlShiftV", chunkDelayMs: null },
  },
  projectRoots: ["C:\\Users\\honorr\\Documents\\DEV\\dictum"],
  modelId: "parakeet-tdt-0.6b-v2-int8",
  reformat: "on",
  reformatDevice: "gpu",
};

const models: ModelInfo[] = [
  { id: "parakeet-tdt-0.6b-v2-int8", display: "PARAKEET-TDT 0.6B V2 INT8", present: true, sizeMb: 630, langs: "ENGLISH", kind: "asr" },
  { id: "parakeet-tdt-0.6b-v3-int8", display: "PARAKEET-TDT 0.6B V3 INT8", present: false, sizeMb: 641, langs: "25 LANGUAGES · AUTO-DETECT", kind: "asr" },
  { id: "dictum-reformat-3b", display: "DICTUM REFORMAT 3B", present: true, sizeMb: 1930, langs: "3B · Q4_K_M · GPU (4GB+ VRAM)", kind: "llm" },
  { id: "dictum-reformat-1.5b", display: "DICTUM REFORMAT 1.5B", present: false, sizeMb: 986, langs: "1.5B · Q4_K_M · CPU", kind: "llm" },
];

let deleted: HistoryRecord | null = null;
const listeners = new Map<string, number>(); // event name -> handler id
let hudSink: ((e: HudEvent) => void) | null = null;

/** Fire a Tauri event into the app (from devtools: `__mock.emit("history://changed")`). */
function emit(event: string, payload?: unknown) {
  const id = listeners.get(event);
  if (id == null) return;
  (window as unknown as Record<string, (e: unknown) => void>)[`_${id}`]?.({ event, id, payload });
}

const isOverlay = location.pathname.includes("overlay");
mockWindows(isOverlay ? "overlay" : "main");

// `?frame=880x700` pins the body to the real window size, centered on a
// neutral backdrop, so browser screenshots match the Tauri window 1:1.
const frame = params.get("frame");
if (frame) {
  const [w, h] = frame.split("x").map(Number);
  const st = document.createElement("style");
  st.textContent = `html{background:#6b6b6b;display:grid;place-items:center;height:100vh}body{width:${w}px;height:${h}px}`;
  document.head.append(st);
}
mockIPC((cmd, payload) => {
  const p = (payload ?? {}) as Record<string, unknown>;
  switch (cmd) {
    case "plugin:event|listen": listeners.set(p.event as string, p.handler as number); return 1;
    case "plugin:event|unlisten": return;
    case "plugin:app|version": return "0.3.0";
    case "plugin:window|minimize":
    case "plugin:window|toggle_maximize":
    case "plugin:window|close": return;
    case "get_config": return structuredClone(config);
    case "set_config": config = structuredClone(p.config as Config); emit("config://changed", structuredClone(config)); return;
    case "try_hotkey": return;
    case "list_input_devices": return params.has("nomic") ? [] : ["Yeti X", "Realtek HD Audio Mic"];
    case "model_info": return params.has("nomodel") ? models.map((m) => ({ ...m, present: false })) : models;
    case "get_model_status": return params.has("nomodel") ? { k: "missing" } : { k: "ready" };
    case "get_reformat_status": return { k: "unloaded" };
    case "get_gpu_info": return { vramMb: 32187, offerGpu3b: true };
    case "download_model": {
      const ch = p.progress as { onmessage: (m: unknown) => void };
      let pct = 0;
      const t = setInterval(() => {
        pct += 7;
        if (pct < 100) ch.onmessage({ t: "progress", pct, mb_done: Math.round(6.41 * pct), mb_total: 641 });
        else { clearInterval(t); ch.onmessage({ t: "verifying" }); setTimeout(() => ch.onmessage({ t: "done" }), 900); }
      }, 120);
      return;
    }
    case "history_list": {
      const q = (p.search as string | null)?.toLowerCase();
      return records.filter((r) => !q || r.text.toLowerCase().includes(q));
    }
    case "history_count": return records.length;
    case "history_delete": {
      const i = records.findIndex((r) => r.id === p.id);
      if (i >= 0) deleted = records.splice(i, 1)[0];
      return;
    }
    case "history_undo_delete": if (deleted) { records.push(deleted); records.sort((a, b) => b.ts - a.ts); deleted = null; } return;
    case "paste_last": case "toggle_dictation": case "copy_text": return;
    case "import_replacements": return 0;
    case "export_replacements": return config.replacements.map((r) => `${r.heard} -> ${r.printed}`).join("\n");
    case "subscribe_hud": hudSink = (p.channel as { onmessage: (e: HudEvent) => void }).onmessage; if (isOverlay) hudLoop(); return;
  }
  throw new Error(`dev-mock: unhandled command ${cmd}`);
});

/** HUD demo loop: listen (fake bars) → print → reformat → printed → hide, forever.
 * `?state=<name>` pins one state for screenshots. */
function hudLoop() {
  const send = (e: HudEvent) => hudSink?.(e);
  const pin = params.get("state");
  let i = 0;
  const bars = () => send({ t: "levels", bars: Array.from({ length: 4 }, () => ({ amp: Math.random() * 0.7 + 0.1, clip: Math.random() < 0.02 })) });
  const listen = () => { send({ t: "state", s: { k: "listening" } }); const t = setInterval(bars, 150); return () => clearInterval(t); };
  if (pin) {
    const states: Record<string, HudEvent> = {
      listening: { t: "state", s: { k: "listening" } },
      warming: { t: "state", s: { k: "loading_model", pct: 62 } },
      printing: { t: "state", s: { k: "transcribing" } },
      reformatting: { t: "state", s: { k: "reformatting" } },
      printed: { t: "state", s: { k: "injected", chars: 142 } },
      killed: { t: "state", s: { k: "cancelled" } },
      holdon: { t: "state", s: { k: "confirm_discard" } },
      error: { t: "state", s: { k: "error", label: "PROTECTED WINDOW", detail: "SENT TO CLIPBOARD — PASTE IT" } },
    };
    if (pin === "printing" || pin === "reformatting") { const stop = listen(); setTimeout(() => { stop(); send(states[pin]); }, 1500); }
    else if (pin === "listening") listen();
    else send(states[pin] ?? states.listening);
    return;
  }
  const step = () => {
    i++;
    const stop = listen();
    setTimeout(() => { stop(); send({ t: "state", s: { k: "transcribing" } }); }, 3500);
    setTimeout(() => send({ t: "state", s: { k: "reformatting" } }), 4300);
    setTimeout(() => send({ t: "state", s: { k: "injected", chars: 80 + i * 13 } }), 5200);
    setTimeout(() => send({ t: "state", s: { k: "hidden" } }), 6100);
    setTimeout(step, 7500);
  };
  step();
}

(window as unknown as { __mock: unknown }).__mock = { emit, records, get config() { return config; } };
