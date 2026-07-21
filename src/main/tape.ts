// TAPE view (§5.2) — the home IS the history. Toolbar + sprocket feed.
import { api } from "../bindings";
import type { HistoryRecord } from "../bindings";
import { h, debounce } from "../shared";
import { chordDisplay, retentionLabel, type Ctx } from "./main";

const DAYS = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];
const MONTHS = ["JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"];

let expandedId: number | null = null;
let undoTimer: ReturnType<typeof setInterval> | undefined;

function fmtTime(ts: number): string {
  const d = new Date(ts);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

function dayLabel(ts: number): string {
  const d = new Date(ts);
  const now = new Date();
  let base = `${DAYS[d.getDay()]} ${MONTHS[d.getMonth()]} ${d.getDate()}`;
  if (d.getFullYear() !== now.getFullYear()) base += ` ${d.getFullYear()}`;
  const today = d.toDateString() === now.toDateString();
  return today ? `TODAY — ${base}` : base;
}

/** Envelope trace (§5.2): 280x24 canvas, pen polyline in ink2 1.4px, zero-line
 * dots. Oxide ticks only where the take clipped (envelope at full scale). */
function buildTrace(rec: HistoryRecord): HTMLCanvasElement {
  const W = 280, H = 24, MID = 12;
  const canvas = h("canvas", { class: "trace", width: W, height: H });
  const dpr = window.devicePixelRatio || 1;
  canvas.width = W * dpr;
  canvas.height = H * dpr;
  canvas.style.width = `${W}px`;
  canvas.style.height = `${H}px`;
  const ctx2d = canvas.getContext("2d");
  if (!ctx2d) return canvas;
  ctx2d.setTransform(dpr, 0, 0, dpr, 0, 0);
  const cs = getComputedStyle(document.documentElement);
  const ink2 = cs.getPropertyValue("--ink2").trim();
  const dots = cs.getPropertyValue("--dots").trim();
  const oxide = cs.getPropertyValue("--oxide").trim();

  ctx2d.fillStyle = dots;
  ctx2d.fillRect(0, MID, W, 1);
  const env = rec.envelope;
  if (env.length < 2) return canvas;

  ctx2d.strokeStyle = ink2;
  ctx2d.lineWidth = 1.4;
  ctx2d.lineJoin = "round";
  ctx2d.beginPath();
  env.forEach((amp, i) => {
    const x = (i / (env.length - 1)) * (W - 2) + 1;
    const y = MID - amp * 10 * Math.sin(i * 1.9);
    if (i === 0) ctx2d.moveTo(x, y);
    else ctx2d.lineTo(x, y);
  });
  ctx2d.stroke();

  if (rec.clipped) {
    ctx2d.fillStyle = oxide;
    env.forEach((amp, i) => {
      if (amp < 0.98) return; // clip positions survive the peak-preserving downsample
      const x = (i / (env.length - 1)) * (W - 2) + 1;
      ctx2d.fillRect(x - 1.2, 0, 2.4, 4);
    });
  }
  return canvas;
}

export function renderTape(ctx: Ctx, host: HTMLElement): void {
  clearInterval(undoTimer);
  expandedId = null;
  let query = "";

  // ---- Toolbar ----
  const search = h("input", {
    class: "field-input search",
    type: "text",
    placeholder: "SEARCH THE TAPE…",
    "aria-label": "Search the tape",
  });
  if (!ctx.config.keepTranscripts) search.disabled = true; // §5.4.4
  const meta = h("span", { class: "value-sm meta" });
  host.append(h("div", { class: "toolbar" }, [search, meta]));

  // ---- Feed ----
  const feed = h("div", { class: "feed" });
  host.append(h("div", { class: "feed-wrap" }, [h("div", { class: "sprocket" }), feed]));

  const audioLabel = "AUDIO OFF";
  function updateMeta() {
    meta.textContent = query
      ? `${ctx.records.length} LINES MATCH · ESC CLEARS`
      : `${ctx.totalLines} LINES · KEPT ${ctx.config.keepTranscripts ? retentionLabel(ctx.config.retention) : "NOTHING"} · ${audioLabel}`;
  }

  function copyRec(rec: HistoryRecord) {
    void api.copyText(rec.text);
  }

  /** STRIKE (§5.2): the row is replaced in place by the 6s undo bar. */
  function strike(rec: HistoryRecord, rowEl: HTMLElement) {
    clearInterval(undoTimer); // a second strike finalizes the previous bar
    void api.historyDelete(rec.id).then(() => {
      ctx.records = ctx.records.filter((r) => r.id !== rec.id);
      ctx.totalLines = Math.max(0, ctx.totalLines - 1);
      updateMeta();
      let left = 6;
      const count = h("span", { class: "value count" }, `${left} S`);
      const bar = h("div", { class: "undo-bar", role: "status" }, [
        h("div", { class: "square" }),
        h("span", { class: "label" }, "LINE PULLED"),
        h("span", { class: "value-sm ink2" }, `#${rec.id} · ${rec.text.length} CH`),
        h("button", {
          class: "action undo",
          onclick: () => {
            clearInterval(undoTimer);
            void api.historyUndoDelete().then(() => reload());
          },
        }, "UNDO"),
        count,
      ]);
      rowEl.replaceWith(bar);
      undoTimer = setInterval(() => {
        left -= 1;
        count.textContent = `${left} S`;
        if (left <= 0) {
          clearInterval(undoTimer);
          bar.remove(); // removal is instant (§5.2)
        }
      }, 1000);
    });
  }

  function buildRow(rec: HistoryRecord): HTMLElement {
    const expanded = expandedId === rec.id;
    const row = h("div", {
      class: expanded ? "trow expanded" : "trow",
      role: "button",
      tabindex: 0,
      "aria-expanded": String(expanded),
    });

    const metaLine = h("div", { class: "meta-line" });
    metaLine.append(h("span", { class: "value ts" }, fmtTime(rec.ts)));
    if (expanded) {
      const parts = [rec.exe ?? "—", `LINE #${rec.id}`];
      if (rec.durMs > 0) parts.push(`${(rec.durMs / 1000).toFixed(1)} S`);
      parts.push(rec.clipped ? "CLIPPED" : "NO CLIPPING");
      metaLine.append(h("span", { class: "value-sm exp-meta" }, parts.join(" · ")));
    } else if (rec.exe) {
      metaLine.append(h("span", { class: "exe-chip" }, rec.exe));
    }
    metaLine.append(h("span", { class: "fill-space" }));

    const actions = h("div", { class: "actions" }, [
      h("button", { class: "action", onclick: (e: Event) => { e.stopPropagation(); copyRec(rec); } }, "COPY"),
      h("button", { class: "action", onclick: (e: Event) => { e.stopPropagation(); strike(rec, row); } }, "STRIKE"),
    ]);
    if (expanded) {
      actions.append(
        h("button", {
          class: "action",
          onclick: (e: Event) => { e.stopPropagation(); expandedId = null; renderFeed(); },
        }, "CLOSE ✕"),
      );
    }
    const slot = h("div", { class: "slot" }, [
      h("span", { class: "value-sm count" }, `${rec.text.length} CH · PRINTED`),
      actions,
    ]);
    metaLine.append(slot);
    row.append(metaLine);

    const text = h("div", { class: "body text" }, rec.text);
    if (!expanded && ctx.freshId === rec.id) text.classList.add("fresh"); // M6 ink-dry
    row.append(text);

    if (expanded) {
      if (rec.envelope.length > 1) row.append(buildTrace(rec));
      if (rec.method) {
        row.append(
          h("div", { class: "value-xs inject-line" },
            `PRINTED TO ${rec.exe ?? "—"} · ${rec.method === "typed" ? "TYPED" : "PASTED"} · ${rec.text.length} CHARS`),
        );
      }
    }

    const toggle = () => {
      expandedId = expanded ? null : rec.id;
      renderFeed(); // instant — no height animation (§5.2)
    };
    row.addEventListener("click", () => { if (!expanded) toggle(); });
    row.addEventListener("keydown", (e) => {
      if (e.key === "Enter" || e.key === " ") { e.preventDefault(); toggle(); }
    });
    return row;
  }

  function renderFeed() {
    clearInterval(undoTimer);
    feed.innerHTML = "";
    updateMeta();

    if (ctx.records.length === 0) {
      const blank = !query;
      feed.append(
        h("div", { class: "empty" }, [
          h("div", { class: "label" }, blank ? "THE TAPE IS BLANK." : "NOTHING ON THE TAPE MATCHES."),
          h("div", { class: "value sub" },
            blank
              ? ctx.models[0]?.present === false
                ? "FETCH THE MODEL, THEN HOLD " + chordDisplay(ctx.config.hotkey) + " AND SPEAK."
                : `HOLD ${chordDisplay(ctx.config.hotkey)} AND SPEAK.`
              : "ESC CLEARS."),
          ...(blank && ctx.models[0]?.present === false
            ? [h("div", { class: "value sub" }, "WHAT YOU SAY PRINTS INTO WHATEVER WINDOW HAS FOCUS.")]
            : []),
        ]),
      );
      return;
    }

    let lastDay = "";
    for (const rec of ctx.records) {
      const day = new Date(rec.ts).toDateString();
      if (day !== lastDay) {
        lastDay = day;
        feed.append(
          h("div", { class: "dayrule" }, [
            h("span", { class: "microlabel cap" }, dayLabel(rec.ts)),
            h("div", { class: "rule" }),
          ]),
        );
      }
      feed.append(buildRow(rec));
    }
    ctx.freshId = null; // ink-dry runs once
  }

  async function reload() {
    await ctx.reloadHistory(query || null);
    expandedId = null;
    renderFeed();
  }

  const runSearch = debounce(() => {
    query = search.value.trim();
    void reload();
  }, 150);
  search.addEventListener("input", runSearch);
  search.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && search.value) {
      search.value = "";
      query = "";
      void reload();
    }
  });
  // Esc anywhere in the feed collapses the expanded row (§8).
  host.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && expandedId !== null) {
      expandedId = null;
      renderFeed();
    }
  });

  renderFeed();
}
