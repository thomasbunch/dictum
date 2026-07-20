// TS mirror of src-tauri/src/types.rs DTOs + typed invoke wrappers.
// Owned by the orchestrator — keep in sync with types.rs.
import { invoke, Channel } from "@tauri-apps/api/core";

export type Theme = "LEDGER" | "BONE" | "PLASTER" | "GLACIER" | "LILAC" | "OBSIDIAN";
export type HotkeyMode = "hold" | "toggle" | "both";
export type Retention = "keepNothing" | "hours24" | "days7" | "days30" | "forever";
export type InjectBackend = "clipboard" | "sendInputUnicode";
export type PasteShortcut = "ctrlV" | "ctrlShiftV";

export interface Replacement { heard: string; printed: string }
export interface AppOverride {
  backend?: InjectBackend | null;
  pasteShortcut?: PasteShortcut | null;
  chunkDelayMs?: number | null;
}

export interface Config {
  hotkey: string;
  hotkeyMode: HotkeyMode;
  inputDevice: string | null;
  audioCues: boolean;
  unloadOnIdle: boolean;
  theme: Theme;
  keepTranscripts: boolean;
  retention: Retention;
  vocabulary: string[];
  replacements: Replacement[];
  removeFillers: boolean;
  appOverrides: Record<string, AppOverride>;
}

export interface LevelBar { amp: number; clip: boolean }

export type HudState =
  | { k: "hidden" }
  | { k: "loading_model"; pct: number }
  | { k: "listening" }
  | { k: "transcribing" }
  | { k: "injected"; chars: number }
  | { k: "cancelled" }
  | { k: "error"; msg: string }
  | { k: "confirm_discard" };

export type HudEvent =
  | { t: "state"; s: HudState }
  | { t: "levels"; bars: LevelBar[] };

export interface HistoryRecord {
  id: number; ts: number; raw: string; text: string; exe: string | null;
}

export interface ModelInfo {
  id: string; display: string; present: boolean; sizeMb: number;
}

export type DownloadProgress =
  | { t: "progress"; pct: number; mbDone: number; mbTotal: number }
  | { t: "verifying" }
  | { t: "done" }
  | { t: "failed"; error: string };

// 16k samples per HUD bar => 37.5 ms of audio per bar (types.rs BAR_SAMPLES).
export const MS_PER_BAR = 37.5;

export const api = {
  getConfig: () => invoke<Config>("get_config"),
  setConfig: (config: Config) => invoke<void>("set_config", { config }),
  tryHotkey: (chord: string) => invoke<void>("try_hotkey", { chord }),
  listInputDevices: () => invoke<string[]>("list_input_devices"),
  modelInfo: () => invoke<ModelInfo[]>("model_info"),
  downloadModel: (id: string, onProgress: (p: DownloadProgress) => void) => {
    const ch = new Channel<DownloadProgress>();
    ch.onmessage = onProgress;
    return invoke<void>("download_model", { id, progress: ch });
  },
  historyList: (search: string | null) => invoke<HistoryRecord[]>("history_list", { search }),
  historyDelete: (id: number) => invoke<void>("history_delete", { id }),
  historyUndoDelete: () => invoke<void>("history_undo_delete"),
  historyMeta: () => invoke<string>("history_meta"),
  pasteLast: () => invoke<void>("paste_last"),
  importReplacements: (text: string, format: "txt" | "json") =>
    invoke<number>("import_replacements", { text, format }),
  exportReplacements: (format: "txt" | "json") =>
    invoke<string>("export_replacements", { format }),
  subscribeHud: (onEvent: (e: HudEvent) => void) => {
    const ch = new Channel<HudEvent>();
    ch.onmessage = onEvent;
    return invoke<void>("subscribe_hud", { channel: ch });
  },
};
