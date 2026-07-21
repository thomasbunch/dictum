// SETUP view (§5.4) — a printed form: label column left, controls right.
import { api } from "../bindings";
import type { AppOverride, HotkeyMode, Retention, Theme } from "../bindings";
import { h, applyTheme } from "../shared";
import { chordDisplay, retentionLabel, runDownloadFlow, type Ctx } from "./main";

function sect(label: string, note: string | null, body: HTMLElement): HTMLElement {
  const left = h("div", {}, [h("div", { class: "label" }, label)]);
  if (note) left.append(h("div", { class: "value-sm sect-note" }, note));
  return h("div", { class: "sect" }, [left, body]);
}

function toggleRow(label: string, note: string | null, checked: boolean, disabled: boolean, onChange: (on: boolean) => void): HTMLElement {
  const input = h("input", { type: "checkbox" });
  input.checked = checked;
  input.disabled = disabled;
  input.addEventListener("change", () => onChange(input.checked));
  const row = h("div", { class: "toggle-row" }, [
    h("label", { class: disabled ? "toggle disabled" : "toggle" }, [
      input,
      h("span", { class: "track" }, [h("span", { class: "knob" })]),
    ]),
    h("span", { class: "label" }, label),
  ]);
  if (note) row.append(h("span", { class: "value-sm note" }, note));
  return row;
}

// ---------------------------------------------------------------------------
// KEY (§5.4.1)
// ---------------------------------------------------------------------------
const MOD_ORDER = ["Ctrl", "Alt", "Shift", "Super"] as const;
const MODIFIER_KEYS = new Set(["Control", "Alt", "Shift", "Meta"]);

function buildKeySection(ctx: Ctx): HTMLElement {
  const chordEl = h("span", {}, chordDisplay(ctx.config.hotkey));
  const capEl = h("span", { class: "cap" }, "CLICK, THEN PRESS A CHORD");
  const chip = h("button", { class: "key-chip" }, [chordEl, capEl]);
  const note = h("div", { class: "value-sm key-note" });
  note.hidden = true;

  let armed = false;
  let rejectTimer: ReturnType<typeof setTimeout> | undefined;
  let heldMods = new Set<string>();
  let heldMain: string | null = null;

  const showChord = () => { chordEl.textContent = chordDisplay(ctx.config.hotkey); chordEl.className = ""; };

  function disarm() {
    armed = false;
    chip.classList.remove("armed");
    window.removeEventListener("keydown", onKeydown, true);
    window.removeEventListener("keyup", onKeyup, true);
    capEl.textContent = "CLICK, THEN PRESS A CHORD";
    note.hidden = true;
    showChord();
  }

  function arm() {
    if (armed) return;
    armed = true;
    clearTimeout(rejectTimer);
    heldMods = new Set();
    heldMain = null;
    chip.classList.add("armed");
    chordEl.textContent = "PRESS A CHORD…";
    chordEl.className = "";
    capEl.textContent = "";
    note.textContent = "ESC CANCELS.";
    note.className = "value-sm key-note";
    note.hidden = false;
    window.addEventListener("keydown", onKeydown, true);
    window.addEventListener("keyup", onKeyup, true);
  }

  /** Rejection (§5.4.1): offending key struck through 1.2s, stays armed. */
  function reject(offender: string, line: string) {
    clearTimeout(rejectTimer);
    chordEl.textContent = offender;
    chordEl.className = "struck";
    note.textContent = line;
    note.className = "value-sm key-note reject";
    note.hidden = false;
    heldMods = new Set();
    heldMain = null;
    rejectTimer = setTimeout(() => {
      if (!armed) return;
      chordEl.textContent = "PRESS A CHORD…";
      chordEl.className = "";
    }, 1200);
  }

  const preview = () => {
    const toks = [
      ...MOD_ORDER.filter((m) => heldMods.has(m)).map((m) => (m === "Super" ? "WIN" : m.toUpperCase())),
      ...(heldMain ? [heldMain.toUpperCase()] : []),
    ];
    chordEl.textContent = toks.length ? toks.join("+") : "PRESS A CHORD…";
    chordEl.className = "";
  };

  const onKeydown = (e: KeyboardEvent) => {
    e.preventDefault();
    e.stopPropagation();
    if (e.key === "Escape") { disarm(); return; }
    if (e.ctrlKey) heldMods.add("Ctrl");
    if (e.altKey) heldMods.add("Alt");
    if (e.shiftKey) heldMods.add("Shift");
    if (e.metaKey) heldMods.add("Super");
    if (!MODIFIER_KEYS.has(e.key)) heldMain = e.key.length === 1 ? e.key.toUpperCase() : e.key;
    preview();
  };

  const onKeyup = () => {
    if (!armed) return;
    if (heldMods.size === 0 && !heldMain) return;
    const line = "THAT KEY CAN'T CARRY THE WIRE. PRESS A CHORD.";
    if (heldMain === "CapsLock") { reject("CAPS LOCK", line); return; }
    if (!heldMain) { reject([...heldMods].join("+").toUpperCase(), line); return; } // bare modifiers
    if (heldMods.size === 0 && /^F\d{1,2}$/.test(heldMain)) { reject(heldMain, line); return; } // bare F-keys
    const chord = [...MOD_ORDER.filter((m) => heldMods.has(m)), heldMain].join("+");
    api
      .tryHotkey(chord)
      .then(() => {
        ctx.config.hotkey = chord;
        ctx.persistNow();
        disarm();
      })
      .catch(() => reject(chordDisplay(chord), "THAT CHORD IS TAKEN. PRESS ANOTHER."));
  };

  chip.addEventListener("click", arm);

  const modes: [HotkeyMode, string, string][] = [
    ["hold", "HOLD", "PUSH-TO-TALK. RELEASE PRINTS."],
    ["toggle", "TOGGLE", "TAP ON, TAP OFF."],
    ["both", "BOTH", "TAP TOGGLES · HOLD TALKS."],
  ];
  const radios = h("div", { class: "mode-radios", role: "radiogroup", "aria-label": "Hotkey mode" });
  for (const [val, label, desc] of modes) {
    const input = h("input", { type: "radio", name: "hotkey-mode", value: val });
    input.checked = ctx.config.hotkeyMode === val;
    input.addEventListener("change", () => {
      if (!input.checked) return;
      ctx.config.hotkeyMode = val;
      ctx.persistNow();
    });
    radios.append(
      h("label", { class: "radio" }, [
        input,
        h("span", { class: "box" }),
        h("span", { class: "label" }, label),
        h("span", { class: "value-sm desc" }, desc),
      ]),
    );
  }

  const body = h("div", {}, [chip, note, radios]);
  return sect("KEY", "THE ONLY KEY DICTUM OWNS.", body);
}

// ---------------------------------------------------------------------------
// INPUT (§5.4.2)
// ---------------------------------------------------------------------------
function buildInputSection(ctx: Ctx): HTMLElement {
  const select = h("select", { class: "field-input mono" });
  const fillOptions = () => {
    select.innerHTML = "";
    select.append(h("option", { value: "" }, "SYSTEM DEFAULT"));
    for (const d of ctx.devices) select.append(h("option", { value: d }, d.toUpperCase()));
    select.value = ctx.config.inputDevice ?? "";
    if (select.selectedIndex < 0) select.value = ""; // configured device unplugged
  };
  fillOptions();
  // Devices can change between visits — refresh and patch in place.
  void api.listInputDevices().then((d) => {
    ctx.devices = d;
    fillOptions();
  });
  select.addEventListener("change", () => {
    ctx.config.inputDevice = select.value === "" ? null : select.value;
    ctx.persistNow();
  });

  const fill = h("div", { class: "fill" });
  ctx.meterFill = fill; // live ONLY while SETUP is visible; cleared on view exit

  const body = h("div", {}, [
    h("div", { class: "select-wrap device-select" }, [select]),
    h("div", { class: "value-sm sect-note" }, "FOLLOWS SYSTEM DEFAULT WHEN UNSET."),
    h("div", { class: "meter" }, [fill]),
    toggleRow("AUDIO CUES", "CLICKS ON START · STOP · KILL · ERROR. NEVER ON SUCCESS.", ctx.config.audioCues, false, (on) => {
      ctx.config.audioCues = on;
      ctx.persistNow();
    }),
  ]);
  return sect("INPUT", "METER LIVE ONLY WHILE VISIBLE.", body);
}

// ---------------------------------------------------------------------------
// MODEL (§5.4.3)
// ---------------------------------------------------------------------------
function buildModelSection(ctx: Ctx): HTMLElement {
  const name = h("span", { class: "name" }, ctx.models[0]?.display ?? "PARAKEET-TDT 0.6B V2");
  const stat = h("span", { class: "stat" });
  const line2 = h("div", { class: "value-sm line2" });
  const dlSlot = h("div");
  const card = h("div", { class: "model-card" }, [
    h("div", { class: "head" }, [name, stat]),
    line2,
    dlSlot,
  ]);

  const sizeMb = ctx.models[0]?.sizeMb ?? 610;

  function update() {
    const present = ctx.models[0]?.present ?? false;
    line2.textContent = `${sizeMb} MB · SHERPA-ONNX · CPU`;
    if (!present) {
      stat.textContent = "NOT ON THIS MACHINE";
      if (!dlSlot.hasChildNodes()) {
        const slot = h("div", { class: "dl" });
        slot.append(
          h("button", {
            class: "btn",
            onclick: () =>
              runDownloadFlow(slot, sizeMb, () => {
                void api.modelInfo().then((m) => {
                  ctx.models = m;
                  update();
                });
              }),
          }, `FETCH THE MODEL · ${sizeMb} MB`),
        );
        dlSlot.append(slot);
      }
      return;
    }
    dlSlot.innerHTML = "";
    switch (ctx.modelStatus.k) {
      case "ready": stat.textContent = "● LOADED"; break;
      case "unloaded": stat.textContent = "○ IDLE — UNLOADED"; break;
      case "loading": stat.textContent = `WARMING UP · ${ctx.modelStatus.pct}%`; break;
      case "error": stat.textContent = "MODEL ERROR"; break;
      case "missing": stat.textContent = "NOT ON THIS MACHINE"; break;
    }
  }
  update();
  ctx.modelCardUpdate = update;

  const body = h("div", {}, [
    card,
    toggleRow("UNLOAD ON IDLE", "FREES ~600 MB. THE NEXT TAKE PAYS A RELOAD.", ctx.config.unloadOnIdle, false, (on) => {
      ctx.config.unloadOnIdle = on;
      ctx.persistNow();
    }),
  ]);
  return sect("MODEL", "THE ONLY DOWNLOAD DICTUM WILL EVER MAKE.", body);
}

// ---------------------------------------------------------------------------
// TAPE & PRIVACY (§5.4.4)
// ---------------------------------------------------------------------------
const RETENTIONS: Retention[] = ["keepNothing", "hours24", "days7", "days30", "forever"];

function buildPrivacySection(ctx: Ctx): HTMLElement {
  const retention = h("div", { class: "retention", role: "radiogroup", "aria-label": "Retention" });
  const chips: HTMLButtonElement[] = [];
  for (const r of RETENTIONS) {
    const b = h("button", {
      role: "radio",
      "aria-checked": String(ctx.config.retention === r),
      onclick: () => {
        ctx.config.retention = r;
        chips.forEach((c, i) => c.setAttribute("aria-checked", String(RETENTIONS[i] === r)));
        ctx.persistNow();
        ctx.updateFooter();
      },
    }, retentionLabel(r));
    // Arrow keys move within the retention chips (§8).
    b.addEventListener("keydown", (e) => {
      const i = RETENTIONS.indexOf(r);
      if (e.key === "ArrowRight" || e.key === "ArrowDown") chips[(i + 1) % chips.length]?.focus();
      else if (e.key === "ArrowLeft" || e.key === "ArrowUp") chips[(i - 1 + chips.length) % chips.length]?.focus();
    });
    chips.push(b);
    retention.append(b);
  }
  const setRetentionDisabled = (off: boolean) => {
    retention.classList.toggle("disabled", off);
    chips.forEach((c) => (c.disabled = off));
  };
  setRetentionDisabled(!ctx.config.keepTranscripts);

  const body = h("div", {}, [
    toggleRow("KEEP TRANSCRIPTS", null, ctx.config.keepTranscripts, false, (on) => {
      ctx.config.keepTranscripts = on;
      setRetentionDisabled(!on); // §5.4.4: OFF disables retention (and search)
      ctx.persistNow();
      ctx.updateFooter();
    }),
    // v1 never records audio — the toggle is printed, disabled, off (§10.13).
    toggleRow("KEEP AUDIO", "OFF BY DEFAULT. TRANSCRIPTS ONLY.", false, true, () => {}),
    retention,
  ]);
  return sect("TAPE & PRIVACY", "THE STATE IS PRINTED ON EVERY SURFACE.", body);
}

// ---------------------------------------------------------------------------
// INJECTION (§5.4.5)
// ---------------------------------------------------------------------------
const DELAY_STEPS = [5, 12, 25, 50];

function buildInjectionSection(ctx: Ctx): HTMLElement {
  const table = h("div", { class: "inj-table" });

  function cycChip(text: string, title: string, onclick: () => void): HTMLElement {
    return h("button", { class: "cyc", title, onclick }, text);
  }

  function render() {
    table.innerHTML = "";
    table.append(
      h("div", { class: "inj-head" }, [
        h("span", { class: "c-app" }, "APP"),
        h("span", { class: "c-method" }, "METHOD"),
        h("span", { class: "c-paste" }, "PASTE KEY"),
        h("span", { class: "c-delay" }, "DELAY"),
        h("span", { class: "x-col" }),
      ]),
      h("div", { class: "inj-row default" }, [
        h("span", { class: "c-app" }, "DEFAULT — ALL APPS"),
        h("span", { class: "c-method" }, "PASTE"),
        h("span", { class: "c-paste" }, "CTRL+V"),
        h("span", { class: "c-delay dim" }, "—"),
        h("span", { class: "x-col" }),
      ]),
    );

    for (const [exe, ov] of Object.entries(ctx.config.appOverrides)) {
      const typed = ov.backend === "sendInputUnicode";
      const method = cycChip(typed ? "TYPE" : "PASTE", "Click to switch method", () => {
        ov.backend = typed ? "clipboard" : "sendInputUnicode";
        render();
        ctx.persistNow();
      });
      const pasteKey = typed
        ? h("span", { class: "c-paste dim" }, "—")
        : cycChip(ov.pasteShortcut === "ctrlShiftV" ? "CTRL+SHIFT+V" : "CTRL+V", "Click to switch paste key", () => {
            ov.pasteShortcut = ov.pasteShortcut === "ctrlShiftV" ? "ctrlV" : "ctrlShiftV";
            render();
            ctx.persistNow();
          });
      if (!typed) pasteKey.classList.add("c-paste");
      const delayVal = ov.chunkDelayMs ?? DELAY_STEPS[0];
      const delay = typed
        ? cycChip(`${delayVal} MS/CHUNK`, "Click to cycle chunk delay", () => {
            const i = DELAY_STEPS.indexOf(delayVal);
            ov.chunkDelayMs = DELAY_STEPS[(i + 1) % DELAY_STEPS.length];
            render();
            ctx.persistNow();
          })
        : h("span", { class: "c-delay dim" }, "—");
      if (typed) delay.classList.add("c-delay");
      method.classList.add("c-method");

      table.append(
        h("div", { class: "inj-row" }, [
          h("span", { class: "c-app" }, exe),
          method,
          pasteKey,
          delay,
          h("button", {
            class: "x",
            "aria-label": `Remove ${exe} override`,
            onclick: () => {
              delete ctx.config.appOverrides[exe];
              render();
              ctx.persistNow();
            },
          }, "✕"),
        ]),
      );
    }
  }
  render();

  const addLink = h("button", {
    class: "action add-app",
    onclick: () => {
      const input = h("input", { class: "field-input", type: "text", placeholder: "APP.EXE", "aria-label": "Exe name" });
      const row = h("div", { class: "inj-row" }, [input]);
      table.append(row);
      input.focus();
      const commit = () => {
        const exe = input.value.trim().toLowerCase();
        row.remove();
        if (!exe || ctx.config.appOverrides[exe]) return;
        ctx.config.appOverrides[exe] = {} as AppOverride;
        render();
        ctx.persistNow();
      };
      input.addEventListener("keydown", (e) => {
        if (e.key === "Enter") commit();
        else if (e.key === "Escape") row.remove();
      });
      input.addEventListener("blur", commit);
    },
  }, "ADD APP");

  const body = h("div", {}, [table, addLink]);
  return sect("INJECTION", "PER-APP OVERRIDES.", body);
}

// ---------------------------------------------------------------------------
// APPEARANCE (§5.4.6) + ABOUT (§5.4.7)
// ---------------------------------------------------------------------------
const THEMES: Theme[] = ["BONE", "LEDGER", "GLACIER", "LILAC", "OBSIDIAN"];

function buildAppearanceSection(ctx: Ctx): HTMLElement {
  const cards: HTMLButtonElement[] = [];
  const row = h("div", { class: "theme-cards", role: "radiogroup", "aria-label": "Theme" });
  for (const t of THEMES) {
    const card = h("button", {
      class: `theme-card ${t.toLowerCase()}`,
      role: "radio",
      "aria-checked": String(ctx.config.theme === t),
      onclick: () => {
        ctx.config.theme = t;
        applyTheme(ctx.config); // instant, no confirm, no transition (§7 M9)
        cards.forEach((c, i) => c.setAttribute("aria-checked", String(THEMES[i] === t)));
        ctx.persistNow();
      },
    }, t);
    cards.push(card);
    row.append(card);
  }
  return sect("APPEARANCE", "APPLIES INSTANTLY.", row);
}

function buildAboutSection(ctx: Ctx): HTMLElement {
  const body = h("div", {}, [
    h("div", { class: "about-line" }, [
      `DICTUM ${ctx.version} · APACHE-2.0 · `,
      h("a", { href: "https://github.com/dictum-app/dictum", target: "_blank", rel: "noreferrer" }, "SOURCE ↗"),
    ]),
    h("div", { class: "value-sm about-statement" }, [
      "EVERYTHING — AUDIO, TEXT, MODEL — STAYS ON THIS MACHINE.",
      h("br"),
      "DICTUM MAKES NO NETWORK CALLS. THE MODEL FETCH IS THE ONE EXCEPTION, AND IT ONLY HAPPENS WHEN YOU ASK.",
    ]),
    h("div", { class: "value-sm about-fonts" }, "FONTS: IBM PLEX · OFL — SEE THIRD-PARTY-LICENSES.MD"),
  ]);
  return sect("ABOUT", null, body);
}

// ---------------------------------------------------------------------------
export function renderSetup(ctx: Ctx, host: HTMLElement): void {
  // Devices can change between visits — refresh, then patch the select in place.
  void api.listInputDevices().then((d) => { ctx.devices = d; });
  host.append(
    buildKeySection(ctx),
    buildInputSection(ctx),
    buildModelSection(ctx),
    buildPrivacySection(ctx),
    buildInjectionSection(ctx),
    buildAppearanceSection(ctx),
    buildAboutSection(ctx),
  );
}
