// DICTUM Settings window — DESIGN.md §4.
import {
  api,
  type Config,
  type ModelInfo,
  type Theme,
  type HotkeyMode,
  type Retention,
  type DownloadProgress,
  type LevelBar,
} from "../bindings";
import { h, initTheme, applyTheme, mountError } from "../shared";

let config: Config;
let levelLane: ReturnType<typeof makeLevelLane> | undefined;
let saveTimer: ReturnType<typeof setTimeout> | undefined;

/** Debounced full-config save (typed edits). */
function persist(): void {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => void api.setConfig(config), 150);
}
/** Immediate full-config save (selects/toggles/structural changes). */
function persistNow(): void {
  clearTimeout(saveTimer);
  void api.setConfig(config);
}

// ---------------------------------------------------------------------------
// HOTKEY
// ---------------------------------------------------------------------------
const MOD_ORDER = ["Ctrl", "Alt", "Shift", "Super"] as const;
const MOD_DISPLAY: Record<string, string> = { Ctrl: "CTRL", Alt: "ALT", Shift: "SHIFT", Super: "WIN" };
const MODIFIER_KEYS = new Set(["Control", "Alt", "Shift", "Meta"]);

function chordDisplay(chord: string): string {
  return chord.split("+").map((t) => MOD_DISPLAY[t] ?? t.toUpperCase()).join(" + ");
}

function buildHotkeySection(): HTMLElement {
  const sect = h("div", { class: "sect" });
  sect.append(h("div", { class: "sect-title" }, [h("span", { class: "label" }, "HOTKEY")]));

  const errNote = h("div", { class: "note" }, "Not supported. Remap CapsLock with PowerToys.");
  errNote.hidden = true;

  let capturing = false;
  let heldMods = new Set<string>();
  let heldMain: string | null = null;

  const preview = () => {
    const toks = [...MOD_ORDER.filter((m) => heldMods.has(m)).map((m) => MOD_DISPLAY[m]), ...(heldMain ? [heldMain] : [])];
    return toks.length ? toks.join(" + ") : "PRESS KEYS";
  };

  const onCaptureKeydown = (e: KeyboardEvent) => {
    e.preventDefault();
    if (e.key === "Escape") {
      stopCapture();
      chip.textContent = chordDisplay(config.hotkey);
      return;
    }
    if (e.ctrlKey) heldMods.add("Ctrl");
    if (e.altKey) heldMods.add("Alt");
    if (e.shiftKey) heldMods.add("Shift");
    if (e.metaKey) heldMods.add("Super");
    if (!MODIFIER_KEYS.has(e.key)) heldMain = e.key.length === 1 ? e.key.toUpperCase() : e.key;
    chip.textContent = preview();
  };

  const onCaptureKeyup = () => {
    if (!capturing) return;
    if (heldMods.size === 0 && !heldMain) return;
    const chord = [...MOD_ORDER.filter((m) => heldMods.has(m)), ...(heldMain ? [heldMain] : [])].join("+");
    stopCapture();
    api
      .tryHotkey(chord)
      .then(() => {
        config.hotkey = chord;
        chip.textContent = chordDisplay(chord);
        persistNow();
      })
      .catch(() => {
        errNote.hidden = false;
        chip.textContent = chordDisplay(config.hotkey);
      });
  };

  function stopCapture() {
    capturing = false;
    window.removeEventListener("keydown", onCaptureKeydown, true);
    window.removeEventListener("keyup", onCaptureKeyup, true);
  }

  function startCapture() {
    if (capturing) return;
    capturing = true;
    heldMods = new Set();
    heldMain = null;
    errNote.hidden = true;
    chip.textContent = "PRESS KEYS";
    window.addEventListener("keydown", onCaptureKeydown, true);
    window.addEventListener("keyup", onCaptureKeyup, true);
  }

  const chip = h("button", { class: "chip mono", onclick: () => startCapture() }, chordDisplay(config.hotkey));

  const modes: [HotkeyMode, string][] = [
    ["hold", "HOLD"],
    ["toggle", "TOGGLE"],
    ["both", "BOTH"],
  ];
  const modeButtons: HTMLButtonElement[] = [];
  for (const [val, label] of modes) {
    const b = h(
      "button",
      {
        class: config.hotkeyMode === val ? "on" : undefined,
        onclick: () => {
          config.hotkeyMode = val;
          modeButtons.forEach((x) => x.classList.remove("on"));
          b.classList.add("on");
          persistNow();
        },
      },
      label,
    );
    modeButtons.push(b);
  }
  const segmented = h("div", { class: "segmented" });
  modeButtons.forEach((b) => segmented.append(b));

  sect.append(
    h("div", { class: "row" }, [h("span", { class: "label row-label" }, "ACTIVATION"), h("div", { class: "row-value" }, [chip])]),
    h("div", { class: "row" }, [h("span", { class: "label row-label" }, "MODE"), h("div", { class: "row-value" }, [segmented])]),
    errNote,
  );
  return sect;
}

// ---------------------------------------------------------------------------
// MICROPHONE
// ---------------------------------------------------------------------------
const LANE_W = 224;
const LANE_H = 24;
const BAR_W = 3; // 2px bar + 1px gap; matches 80px/s @ 37.5ms/bar (types.rs BAR_SAMPLES)
const HEAD_X = Math.round(LANE_W * 0.75);
const INK_DRY_MS = 120;

function css(varName: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(varName).trim();
}

interface LaneBar {
  amp: number;
  clip: boolean;
  freshUntil: number;
}

function makeLevelLane(canvas: HTMLCanvasElement) {
  const dpr = window.devicePixelRatio || 1;
  canvas.width = LANE_W * dpr;
  canvas.height = LANE_H * dpr;
  canvas.style.width = `${LANE_W}px`;
  canvas.style.height = `${LANE_H}px`;
  const ctx = canvas.getContext("2d");
  if (ctx) ctx.scale(dpr, dpr);

  let bars: LaneBar[] = [];
  let settleTimer: ReturnType<typeof setTimeout> | undefined;
  let visible = false;
  const maxBars = Math.floor(HEAD_X / BAR_W) + 1;

  function draw() {
    if (!ctx) return;
    ctx.clearRect(0, 0, LANE_W, LANE_H);
    ctx.fillStyle = css("--paper");
    ctx.fillRect(0, 0, LANE_W, LANE_H);
    const ink = css("--ink");
    const oxide = css("--oxide");
    const now = performance.now();
    const zero = LANE_H / 2;
    const shown = bars.slice(-maxBars);
    shown.forEach((b, i) => {
      const x = HEAD_X - (shown.length - 1 - i) * BAR_W;
      const barH = Math.max(1, b.amp * (LANE_H / 2));
      const fresh = b.clip || now < b.freshUntil;
      ctx.fillStyle = fresh ? oxide : ink;
      ctx.globalAlpha = fresh ? 1 : 0.88;
      ctx.fillRect(x, zero - barH, 2, barH * 2);
    });
    ctx.globalAlpha = 1;
  }

  function onLevels(newBars: LevelBar[]) {
    if (!visible || newBars.length === 0) return;
    const now = performance.now();
    for (const nb of newBars) bars.push({ amp: nb.amp, clip: nb.clip, freshUntil: now + INK_DRY_MS });
    if (bars.length > maxBars) bars = bars.slice(-maxBars);
    draw();
    clearTimeout(settleTimer);
    // one bounded settle redraw — not a continuous loop (DESIGN.md §7 idle audit)
    settleTimer = setTimeout(draw, INK_DRY_MS + 10);
  }

  function setVisible(v: boolean) {
    visible = v;
    if (!v) {
      bars = [];
      ctx?.clearRect(0, 0, LANE_W, LANE_H);
    }
  }

  return { onLevels, setVisible };
}

async function buildMicSection(): Promise<HTMLElement> {
  const sect = h("div", { class: "sect" });
  sect.append(h("div", { class: "sect-title" }, [h("span", { class: "label" }, "MICROPHONE")]));

  const select = h("select", { class: "flat mono" });
  select.append(h("option", { value: "" }, "SYSTEM DEFAULT"));
  const devices = await api.listInputDevices();
  for (const d of devices) select.append(h("option", { value: d }, d));
  select.value = config.inputDevice ?? "";
  select.addEventListener("change", () => {
    config.inputDevice = select.value === "" ? null : select.value;
    persistNow();
  });
  sect.append(h("div", { class: "row" }, [h("span", { class: "label row-label" }, "INPUT DEVICE"), h("div", { class: "row-value" }, [select])]));

  const canvas = h("canvas", { class: "level-lane" });
  const rowLevel = h("div", { class: "row" }, [h("span", { class: "label row-label" }, "LEVEL"), h("div", { class: "row-value" }, [canvas])]);
  sect.append(rowLevel);

  levelLane = makeLevelLane(canvas);
  const io = new IntersectionObserver(([entry]) => levelLane?.setVisible(entry.isIntersecting), { threshold: 0.01 });
  io.observe(rowLevel);

  return sect;
}

// ---------------------------------------------------------------------------
// MODEL
// ---------------------------------------------------------------------------
function buildStatusRow(m: ModelInfo, body: HTMLElement): HTMLElement {
  if (m.present) {
    return h("div", { class: "row" }, [h("span", { class: "label row-label" }, "STATUS"), h("div", { class: "row-value" }, `LOADED · ${m.sizeMb} MB`)]);
  }
  const progress = h("span", { class: "mono" });
  const getBtn = h("button", { class: "text-btn", onclick: () => startDownload(m.id, progress, getBtn, body) }, "GET");
  const value = h("div", { class: "row-value dim" }, ["NOT DOWNLOADED · "]);
  value.append(getBtn, progress);
  return h("div", { class: "row" }, [h("span", { class: "label row-label" }, "STATUS"), value]);
}

function startDownload(id: string, progress: HTMLElement, getBtn: HTMLButtonElement, body: HTMLElement): void {
  getBtn.disabled = true;
  api
    .downloadModel(id, (p: DownloadProgress) => {
      if (p.t === "progress") progress.textContent = ` ${p.pct}%`;
      else if (p.t === "verifying") progress.textContent = " VERIFYING";
      else if (p.t === "done") void renderModelRows(body);
      else if (p.t === "failed") {
        progress.textContent = ` ${p.error}`;
        getBtn.disabled = false;
      }
    })
    .catch((err: unknown) => {
      progress.textContent = ` ${String(err)}`;
      getBtn.disabled = false;
    });
}

async function renderModelRows(body: HTMLElement): Promise<void> {
  body.innerHTML = "";
  const models = await api.modelInfo();
  for (const m of models) {
    body.append(h("div", { class: "row" }, [h("span", { class: "label row-label" }, "MODEL"), h("div", { class: "row-value" }, m.display)]));
    body.append(buildStatusRow(m, body));
  }
  body.append(h("div", { class: "row" }, [h("span", { class: "label row-label" }, "RUNTIME"), h("div", { class: "row-value" }, "SHERPA-ONNX · CPU")]));
}

async function buildModelSection(): Promise<HTMLElement> {
  const sect = h("div", { class: "sect" });
  sect.append(h("div", { class: "sect-title" }, [h("span", { class: "label" }, "MODEL")]));
  const body = h("div");
  sect.append(body);
  await renderModelRows(body);
  return sect;
}

// ---------------------------------------------------------------------------
// VOCABULARY + REPLACEMENTS
// ---------------------------------------------------------------------------
function buildReplacementsSubsection(): HTMLElement {
  const wrap = h("div");
  wrap.append(
    h("div", { class: "sect-title" }, [h("span", { class: "label" }, "REPLACEMENTS"), h("span", { class: "badge" }, "CASE-INSENSITIVE")]),
    h("div", { class: "repl-head" }, [h("span", { class: "label" }, "HEARD"), h("span", { class: "label" }, "PRINTED"), h("span", {}, "")]),
  );

  const table = h("div", { class: "repl-table" });
  function renderTable() {
    table.innerHTML = "";
    config.replacements.forEach((r, i) => {
      const heardInput = h("input", { class: "flat mono", type: "text", value: r.heard });
      heardInput.addEventListener("input", () => {
        r.heard = heardInput.value;
        persist();
      });
      const printedInput = h("input", { class: "flat mono", type: "text", value: r.printed });
      printedInput.addEventListener("input", () => {
        r.printed = printedInput.value;
        persist();
      });
      const removeBtn = h(
        "button",
        {
          class: "text-btn",
          onclick: () => {
            config.replacements.splice(i, 1);
            renderTable();
            persistNow();
          },
        },
        "REMOVE",
      );
      table.append(h("div", { class: "repl-row" }, [heardInput, printedInput, removeBtn]));
    });
  }
  renderTable();

  const addBtn = h(
    "button",
    {
      class: "text-btn",
      onclick: () => {
        config.replacements.push({ heard: "", printed: "" });
        renderTable();
      },
    },
    "ADD ROW",
  );

  const fileInput = h("input", { type: "file", accept: ".txt,.json" });
  fileInput.hidden = true;
  fileInput.addEventListener("change", () => {
    const file = fileInput.files?.[0];
    if (!file) return;
    const format: "txt" | "json" = file.name.toLowerCase().endsWith(".json") ? "json" : "txt";
    void file
      .text()
      .then((text) => api.importReplacements(text, format))
      .then(async () => {
        config = await api.getConfig();
        renderTable();
      });
    fileInput.value = "";
  });
  const importBtn = h("button", { class: "text-btn", onclick: () => fileInput.click() }, "IMPORT");

  function download(filename: string, content: string) {
    const blob = new Blob([content], { type: "text/plain" });
    const url = URL.createObjectURL(blob);
    const a = h("a", { href: url, download: filename });
    document.body.append(a);
    a.click();
    a.remove();
    URL.revokeObjectURL(url);
  }
  const exportTxtBtn = h(
    "button",
    { class: "text-btn", onclick: () => void api.exportReplacements("txt").then((c) => download("replacements.txt", c)) },
    "EXPORT TXT",
  );
  const exportJsonBtn = h(
    "button",
    { class: "text-btn", onclick: () => void api.exportReplacements("json").then((c) => download("replacements.json", c)) },
    "EXPORT JSON",
  );

  wrap.append(table, h("div", { class: "btn-row" }, [addBtn, importBtn, exportTxtBtn, exportJsonBtn]), fileInput);
  return wrap;
}

function buildVocabSection(): HTMLElement {
  const sect = h("div", { class: "sect" });
  const counter = h("span", { class: "mono ink2" });
  sect.append(h("div", { class: "sect-title" }, [h("span", { class: "label" }, "VOCABULARY"), counter]));

  const list = h("div", { class: "list" });
  function renderList() {
    list.innerHTML = "";
    config.vocabulary.forEach((term, i) => {
      list.append(
        h("div", { class: "row" }, [
          h("span", { class: "row-value" }, term),
          h(
            "button",
            {
              class: "text-btn",
              onclick: () => {
                config.vocabulary.splice(i, 1);
                renderList();
                persistNow();
              },
            },
            "REMOVE",
          ),
        ]),
      );
    });
    counter.textContent = `${config.vocabulary.length} / 50`;
  }
  renderList();

  const input = h("input", { class: "flat", type: "text", placeholder: "Add a term, press Enter" });
  input.addEventListener("keydown", (e) => {
    if (e.key !== "Enter") return;
    const val = input.value.trim();
    if (!val || config.vocabulary.length >= 50) return;
    config.vocabulary.push(val);
    input.value = "";
    renderList();
    persistNow();
  });

  sect.append(
    h("div", { class: "row" }, [h("span", { class: "label row-label" }, "ADD"), h("div", { class: "row-value" }, [input])]),
    list,
    h("div", { class: "note" }, "Hints bias recognition. Many entries reduce accuracy."),
    buildReplacementsSubsection(),
  );
  return sect;
}

// ---------------------------------------------------------------------------
// HISTORY (settings toggles)
// ---------------------------------------------------------------------------
function buildToggle(checked: boolean, onChange: (checked: boolean) => void, disabled = false): HTMLLabelElement {
  const input = h("input", { type: "checkbox" });
  input.checked = checked;
  input.disabled = disabled;
  input.addEventListener("change", () => onChange(input.checked));
  return h("label", { class: disabled ? "toggle disabled" : "toggle" }, [input, h("span", { class: "track" }, [h("span", { class: "thumb" })])]);
}

function buildHistorySettingsSection(): HTMLElement {
  const sect = h("div", { class: "sect" });
  sect.append(h("div", { class: "sect-title" }, [h("span", { class: "label" }, "HISTORY")]));

  const keepToggle = buildToggle(config.keepTranscripts, (checked) => {
    config.keepTranscripts = checked;
    persistNow();
  });
  const audioToggle = buildToggle(false, () => {}, true); // v1 never records audio (§types.rs Config has no field for it)

  const retentionSelect = h("select", { class: "flat mono" });
  const options: [Retention, string][] = [
    ["keepNothing", "KEEP NOTHING"],
    ["hours24", "24 H"],
    ["days7", "7 D"],
    ["days30", "30 D"],
    ["forever", "FOREVER"],
  ];
  for (const [val, label] of options) retentionSelect.append(h("option", { value: val }, label));
  retentionSelect.value = config.retention;
  retentionSelect.addEventListener("change", () => {
    config.retention = retentionSelect.value as Retention;
    persistNow();
  });

  sect.append(
    h("div", { class: "row" }, [h("span", { class: "label row-label" }, "KEEP TRANSCRIPTS"), h("div", { class: "row-value" }, [keepToggle])]),
    h("div", { class: "row dim" }, [h("span", { class: "label row-label" }, "KEEP AUDIO"), h("div", { class: "row-value" }, [audioToggle])]),
    h("div", { class: "row" }, [h("span", { class: "label row-label" }, "RETENTION"), h("div", { class: "row-value" }, [retentionSelect])]),
  );
  return sect;
}

// ---------------------------------------------------------------------------
// APPEARANCE
// ---------------------------------------------------------------------------
const THEMES: Theme[] = ["LEDGER", "BONE", "PLASTER", "GLACIER", "LILAC", "OBSIDIAN"];
const swatchEls: Partial<Record<Theme, HTMLElement>> = {};

function updateSwatchSelection() {
  for (const theme of THEMES) swatchEls[theme]?.classList.toggle("selected", config.theme === theme);
}

function buildAppearanceSection(): HTMLElement {
  const sect = h("div", { class: "sect" });
  sect.append(h("div", { class: "sect-title" }, [h("span", { class: "label" }, "APPEARANCE")]));
  const row = h("div", { class: "swatch-row" });
  for (const theme of THEMES) {
    const swatch = h("div", { class: config.theme === theme ? "swatch selected" : "swatch", "data-field": theme });
    swatch.addEventListener("click", () => {
      config.theme = theme;
      applyTheme(config);
      updateSwatchSelection();
      persistNow();
    });
    swatchEls[theme] = swatch;
    row.append(h("div", { class: "swatch-col" }, [swatch, h("span", { class: "swatch-name" }, theme)]));
  }
  sect.append(row);
  return sect;
}

// ---------------------------------------------------------------------------
// ABOUT
// ---------------------------------------------------------------------------
function buildAboutSection(): HTMLElement {
  const sect = h("div", { class: "sect" });
  sect.append(
    h("div", { class: "sect-title" }, [h("span", { class: "label" }, "DICTUM")]),
    h("div", { class: "row" }, [h("span", { class: "row-value ink2" }, "DICTUM · v0.1.0 · LOCAL ONLY · ZERO EGRESS")]),
    h("div", { class: "row" }, [h("span", { class: "label row-label" }, "LICENSE"), h("div", { class: "row-value" }, "APACHE-2.0")]),
    h("div", { class: "row" }, [
      h("span", { class: "label row-label" }, "SOURCE"),
      h("a", { class: "row-value", href: "https://github.com/dictum-app/dictum", target: "_blank", rel: "noreferrer" }, "GITHUB.COM/DICTUM-APP/DICTUM"),
    ]),
  );
  return sect;
}

// ---------------------------------------------------------------------------
async function main() {
  config = await initTheme((cfg) => {
    if (!config) return; // race guard: change arrived before initial fetch settled
    config.theme = cfg.theme;
    updateSwatchSelection();
  });

  const app = document.getElementById("app");
  if (!app) return;
  const cert = h("div", { class: "cert" });
  cert.append(
    h("div", { class: "cert-header" }, [h("span", { class: "wordmark" }, "DICTUM"), h("span", { class: "mono ink2" }, "SETTINGS")]),
    buildHotkeySection(),
    await buildMicSection(),
    await buildModelSection(),
    buildVocabSection(),
    buildHistorySettingsSection(),
    buildAppearanceSection(),
    buildAboutSection(),
  );
  app.append(cert);

  await api.subscribeHud((e) => {
    if (e.t === "levels") levelLane?.onLevels(e.bars);
  });
}

main().catch(mountError);
