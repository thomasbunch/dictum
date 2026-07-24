// WORDS view (§5.3) — two ledgers: vocabulary (canonical casing) and
// replacements (typeset rules), plus the filler strike strip.
import { api, type Replacement } from "../bindings";
import { h } from "../shared";
import type { Ctx } from "./main";
import { CODE_SYMBOLS, CODING_TERMS, addPack } from "./packs";

const VOCAB_MAX = 50;

function buildToggle(checked: boolean, disabled: boolean, onChange: (on: boolean) => void): HTMLElement {
  const input = h("input", { type: "checkbox" });
  input.checked = checked;
  input.disabled = disabled;
  input.addEventListener("change", () => onChange(input.checked));
  const label = h("label", { class: disabled ? "toggle disabled" : "toggle" }, [
    input,
    h("span", { class: "track" }, [h("span", { class: "knob" })]),
  ]);
  return label;
}

function buildVocabulary(ctx: Ctx): HTMLElement {
  const col = h("div", { class: "words-col left" });
  const counter = h("span", { class: "value-sm counter" });
  col.append(
    h("div", { class: "col-head" }, [h("span", { class: "label" }, "VOCABULARY"), counter]),
    h("div", { class: "value-sm col-note" }, "PROPER NOUNS AND JARGON — FIXES THEIR CASING."),
  );

  const input = h("input", { class: "field-input", type: "text", placeholder: "ADD A TERM…", "aria-label": "Add a vocabulary term" });
  const addBtn = h("button", { class: "btn" }, "ADD");
  const chips = h("div", { class: "chips" });

  function updateCounter() {
    const n = ctx.config.vocabulary.length;
    counter.textContent = n >= VOCAB_MAX ? `${n} / ${VOCAB_MAX} — FULL` : `${n} / ${VOCAB_MAX}`;
    const full = n >= VOCAB_MAX;
    input.disabled = full;
    addBtn.disabled = full;
  }

  function renderChips() {
    chips.innerHTML = "";
    ctx.config.vocabulary.forEach((term, i) => {
      chips.append(
        h("span", { class: "chip" }, [
          term,
          h("button", {
            class: "x",
            "aria-label": `Remove ${term}`,
            onclick: () => {
              ctx.config.vocabulary.splice(i, 1);
              renderChips();
              ctx.persistNow();
            },
          }, "✕"),
        ]),
      );
    });
    updateCounter();
  }

  const add = () => {
    const val = input.value.trim();
    if (!val || ctx.config.vocabulary.length >= VOCAB_MAX) return;
    ctx.config.vocabulary.push(val);
    input.value = "";
    renderChips();
    ctx.persistNow();
  };
  addBtn.addEventListener("click", add);
  input.addEventListener("keydown", (e) => { if (e.key === "Enter") add(); });

  col.append(
    h("div", { class: "vocab-add" }, [input, addBtn]),
    chips,
    h("div", { class: "value-sm vocab-warn" }, "CASING ONLY. FOR SPELLING OR EXPANSION, ADD A RULE."),
  );
  renderChips();
  return col;
}

function buildReplacements(ctx: Ctx): HTMLElement {
  const col = h("div", { class: "words-col" });
  col.append(
    h("div", { class: "col-head" }, [
      h("span", { class: "label" }, "REPLACEMENTS"),
      h("span", { class: "value-sm counter" }, "CASE-INSENSITIVE"),
    ]),
    h("div", { class: "value-sm col-note" }, "HEARD ON THE LEFT, PRINTED ON THE RIGHT."),
    h("div", { class: "repl-head" }, [
      h("span", { class: "microlabel" }, "HEARD"),
      h("span", { class: "microlabel" }, "PRINTED"),
      h("span", { class: "x-col" }),
    ]),
  );

  const table = h("div");
  let ghostHeard: HTMLInputElement;

  function buildRealRow(i: number): HTMLElement {
    const r = ctx.config.replacements[i];
    const heard = h("input", { type: "text", value: r.heard, "aria-label": "Heard" });
    heard.addEventListener("input", () => { r.heard = heard.value; ctx.persist(); });
    // Printed is a textarea so snippets can be multi-line. h() sets value via a
    // child text node (setAttribute("value") no-ops on textarea); .value reads back.
    const printed = h("textarea", { rows: 1, "aria-label": "Printed" }, r.printed);
    printed.addEventListener("input", () => { r.printed = printed.value; ctx.persist(); });
    const x = h("button", {
      class: "x",
      "aria-label": "Remove rule",
      onclick: () => {
        ctx.config.replacements.splice(i, 1);
        renderTable();
        ctx.persistNow();
      },
    }, "✕");
    return h("div", { class: "repl-row" }, [heard, printed, x]);
  }

  /** Ghost add row (§5.3): typing into it makes it real. */
  function buildGhostRow(): HTMLElement {
    const heard = h("input", { type: "text", placeholder: "HEARD…", "aria-label": "New rule heard" });
    const printed = h("textarea", { rows: 1, placeholder: "PRINTED…", "aria-label": "New rule printed" });
    ghostHeard = heard;
    const commit = () => {
      if (!heard.value.trim() && !printed.value.trim()) return;
      ctx.config.replacements.push({ heard: heard.value, printed: printed.value });
      renderTable();
      ctx.persistNow();
      // Focus the new row's heard input so typing continues uninterrupted.
      // `.repl-row input` now matches only heard inputs (printed is a textarea),
      // so the new row's heard is the second-to-last input (last is the ghost's).
      const rows = table.querySelectorAll<HTMLInputElement>(".repl-row input");
      rows[rows.length - 2]?.focus();
    };
    heard.addEventListener("change", commit);
    printed.addEventListener("change", commit);
    return h("div", { class: "repl-row" }, [heard, printed, h("span", { class: "x-col" })]);
  }

  function renderTable() {
    table.innerHTML = "";
    ctx.config.replacements.forEach((_, i) => table.append(buildRealRow(i)));
    table.append(buildGhostRow());
  }
  renderTable();

  // Import via the OS file picker; export downloads the chosen format.
  const fileInput = h("input", { type: "file", accept: ".txt,.json" });
  fileInput.hidden = true;
  fileInput.addEventListener("change", () => {
    const file = fileInput.files?.[0];
    if (!file) return;
    const format: "txt" | "json" = file.name.toLowerCase().endsWith(".json") ? "json" : "txt";
    void file
      .text()
      .then((text) => api.importReplacements(text, format))
      .then(() => api.getConfig())
      .then((cfg) => {
        Object.assign(ctx.config, cfg);
        renderTable();
      });
    fileInput.value = "";
  });

  function download(filename: string, content: string) {
    const blob = new Blob([content], { type: "text/plain" });
    const url = URL.createObjectURL(blob);
    const a = h("a", { href: url, download: filename });
    document.body.append(a);
    a.click();
    a.remove();
    URL.revokeObjectURL(url);
  }
  const exportAs = (format: "txt" | "json") =>
    void api.exportReplacements(format).then((c) => download(`replacements.${format}`, c));

  // Preset packs: opt-in, merged client-side (addPack dedupes by heard so a
  // re-click is a no-op and never wipes user rules — unlike importReplacements).
  const packNote = h("span", { class: "value-sm fmt" });
  const addAndRender = (pack: Replacement[], name: string) => {
    const n = addPack(ctx.config.replacements, pack);
    renderTable();
    ctx.persistNow();
    packNote.textContent = n ? `+${n}` : `${name} ALL PRESENT`;
  };

  col.append(
    table,
    h("div", { class: "repl-links" }, [
      h("button", { class: "action", onclick: () => ghostHeard.focus() }, "ADD ROW"),
      h("div", { class: "right" }, [
        h("button", { class: "action", onclick: () => fileInput.click() }, "IMPORT"),
        h("button", { class: "action", onclick: () => exportAs("txt") }, "EXPORT"),
        h("span", { class: "value-sm fmt" }, [
          h("button", { class: "action", onclick: () => exportAs("txt") }, "TXT"),
          " · ",
          h("button", { class: "action", onclick: () => exportAs("json") }, "JSON"),
        ]),
      ]),
    ]),
    h("div", { class: "repl-links" }, [
      h("span", { class: "microlabel" }, "PACKS"),
      h("button", { class: "action", onclick: () => addAndRender(CODE_SYMBOLS, "SYMBOLS") }, "CODE SYMBOLS"),
      h("button", { class: "action", onclick: () => addAndRender(CODING_TERMS, "TERMS") }, "CODING TERMS"),
      packNote,
    ]),
    fileInput,
  );
  return col;
}

export function renderWords(ctx: Ctx, host: HTMLElement): void {
  host.append(
    h("div", { class: "words-grid" }, [buildVocabulary(ctx), buildReplacements(ctx)]),
    h("div", { class: "filler-strip" }, [
      buildToggle(ctx.config.removeFillers, false, (on) => {
        ctx.config.removeFillers = on;
        ctx.persistNow();
      }),
      h("span", { class: "label" }, "STRIKE FILLER WORDS"),
      h("span", { class: "value-sm note" }, "UM, UH, ER NEVER REACH THE TAPE."),
    ]),
  );
}
