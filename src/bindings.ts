// TS mirror of src-tauri/src/types.rs DTOs + typed invoke wrappers.
// Owned by the orchestrator — keep in sync with types.rs.
import { invoke, Channel } from "@tauri-apps/api/core";

export type Theme = "BONE" | "LEDGER" | "GLACIER" | "LILAC" | "OBSIDIAN";
export type HotkeyMode = "hold" | "toggle" | "both";
export type ReformatMode = "auto" | "on" | "off";
export type ReformatDevice = "auto" | "gpu" | "cpu";
export type ModelKind = "asr" | "llm";
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
  /** Folders indexed for spoken file-name tagging (FILE TAG). Empty = off. */
  projectRoots: string[];
  /** Active ASR model id (see modelInfo()). Unknown ids fall back to default. */
  modelId: string;
  /** LLM reformatter mode ("auto" GPU-gated | "on" | "off"). Default "auto". */
  reformat: ReformatMode;
  /** Reformat compute device ("auto" follows the GPU gate | "gpu" | "cpu").
   *  Only meaningful on a Vulkan build; a CPU build always runs on CPU. Default "auto". */
  reformatDevice: ReformatDevice;
}

export interface LevelBar { amp: number; clip: boolean }

export type HudState =
  | { k: "hidden" }
  | { k: "loading_model"; pct: number }
  | { k: "listening" }
  | { k: "transcribing" }
  | { k: "injected"; chars: number }
  | { k: "cancelled" }
  | { k: "reformatting" }
  | { k: "error"; label: string; detail: string }
  | { k: "confirm_discard" };

export type HudEvent =
  | { t: "state"; s: HudState }
  | { t: "levels"; bars: LevelBar[] };

export interface HistoryRecord {
  id: number;
  ts: number;
  raw: string;
  text: string;
  exe: string | null;
  /** Take length in ms; 0 for pre-TAPE rows. */
  durMs: number;
  clipped: boolean;
  /** ≤64-point amplitude envelope; empty for pre-TAPE rows. */
  envelope: number[];
  /** "pasted" | "typed" | null for pre-TAPE rows. */
  method: string | null;
}

export interface ModelInfo {
  id: string; display: string; present: boolean; sizeMb: number;
  /** SETUP card line-2 fragment ("ENGLISH" / "25 LANGUAGES · AUTO-DETECT"). */
  langs: string;
  /** "asr" | "llm" — SETUP renders each kind in its own section. */
  kind: ModelKind;
}

export interface GpuInfoDto {
  vramMb: number;
  /** GPU gate result: true => reformatter picks the 3B GPU SKU, else 1.5B CPU. */
  offerGpu3b: boolean;
}

export type ModelStatus =
  | { k: "missing" }
  | { k: "loading"; pct: number }
  | { k: "ready" }
  | { k: "unloaded" }
  | { k: "error"; msg: string };

export type DownloadProgress =
  // snake_case fields: types.rs's enum-level rename_all renames only the variant
  // tag, not struct-variant fields, so the wire keys stay mb_done / mb_total.
  | { t: "progress"; pct: number; mb_done: number; mb_total: number }
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
  getModelStatus: () => invoke<ModelStatus>("get_model_status"),
  getReformatStatus: () => invoke<ModelStatus>("get_reformat_status"),
  getGpuInfo: () => invoke<GpuInfoDto>("get_gpu_info"),
  downloadModel: (id: string, onProgress: (p: DownloadProgress) => void) => {
    const ch = new Channel<DownloadProgress>();
    ch.onmessage = onProgress;
    return invoke<void>("download_model", { id, progress: ch });
  },
  historyList: (search: string | null) => invoke<HistoryRecord[]>("history_list", { search }),
  historyDelete: (id: number) => invoke<void>("history_delete", { id }),
  historyUndoDelete: () => invoke<void>("history_undo_delete"),
  historyCount: () => invoke<number>("history_count"),
  pasteLast: () => invoke<void>("paste_last"),
  toggleDictation: () => invoke<void>("toggle_dictation"),
  copyText: (text: string) => invoke<void>("copy_text", { text }),
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
