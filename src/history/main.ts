// DICTUM History window — DESIGN.md §5.
import { invoke } from "@tauri-apps/api/core";
import { api, type HistoryRecord } from "../bindings";
import { h, initTheme, debounce, mountError } from "../shared";

let records: HistoryRecord[] = [];
let footerTimer: ReturnType<typeof setTimeout> | undefined;

function formatTs(ts: number): string {
  const d = new Date(ts);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

async function copyText(text: string): Promise<void> {
  await invoke<void>("copy_text", { text });
}

async function refreshFooter(): Promise<void> {
  clearTimeout(footerTimer);
  const footer = document.getElementById("footer");
  if (!footer) return;
  footer.textContent = await api.historyMeta();
}

function showUndoFooter(): void {
  clearTimeout(footerTimer);
  const footer = document.getElementById("footer");
  if (!footer) return;
  footer.innerHTML = "";
  footer.append(
    "RECORD DELETED · ",
    h(
      "button",
      {
        class: "text-btn",
        onclick: () => {
          clearTimeout(footerTimer);
          void api.historyUndoDelete().then(() => reload());
        },
      },
      "UNDO",
    ),
  );
  footerTimer = setTimeout(() => void refreshFooter(), 4000);
}

async function doDelete(id: number, removeLocal: () => void): Promise<void> {
  await api.historyDelete(id);
  removeLocal();
  showUndoFooter();
}

function buildRow(rec: HistoryRecord, removeLocal: () => void): HTMLElement {
  const transcript = h("div", { class: "transcript mono" }, rec.text);
  const expandCopy = h(
    "button",
    { class: "text-btn", onclick: (e: Event) => { e.stopPropagation(); void copyText(rec.text); } },
    "COPY",
  );
  const expandDelete = h(
    "button",
    { class: "text-btn", onclick: (e: Event) => { e.stopPropagation(); void doDelete(rec.id, removeLocal); } },
    "DELETE",
  );
  const expandInner = h("div", { class: "expand-inner" }, [transcript, h("div", { class: "expand-actions" }, [expandCopy, expandDelete])]);
  const expand = h("div", { class: "expand" }, [expandInner]);

  const rowCopy = h(
    "button",
    { class: "text-btn", onclick: (e: Event) => { e.stopPropagation(); void copyText(rec.text); } },
    "COPY",
  );
  const rowDelete = h(
    "button",
    { class: "text-btn", onclick: (e: Event) => { e.stopPropagation(); void doDelete(rec.id, removeLocal); } },
    "DELETE",
  );
  const actions = h("span", { class: "actions" }, [rowCopy, h("span", { class: "ink2" }, " · "), rowDelete]);
  const count = h("span", { class: "count mono" }, String(rec.text.length));
  const rightSlot = h("div", { class: "right-slot" }, [count, actions]);

  const row = h("div", { class: "row", onclick: () => expand.classList.toggle("open") }, [
    h("span", { class: "ts" }, formatTs(rec.ts)),
    h("span", { class: "preview" }, rec.text.split("\n")[0] ?? ""),
    rightSlot,
  ]);

  return h("div", { class: "item" }, [row, expand]);
}

function renderList(): void {
  const list = document.getElementById("list");
  const empty = document.getElementById("empty");
  if (!list || !empty) return;
  list.innerHTML = "";
  if (records.length === 0) {
    list.hidden = true;
    empty.hidden = false;
    return;
  }
  list.hidden = false;
  empty.hidden = true;
  for (const rec of records) {
    list.append(
      buildRow(rec, () => {
        records = records.filter((r) => r.id !== rec.id);
        renderList();
      }),
    );
  }
}

async function reload(search: string | null = null): Promise<void> {
  records = await api.historyList(search);
  renderList();
  await refreshFooter();
}

async function main() {
  await initTheme();

  const app = document.getElementById("app");
  if (!app) return;

  const search = h("input", { class: "search mono", type: "text", placeholder: "SEARCH" });
  const runSearch = debounce(() => void reload(search.value.trim() || null), 150);
  search.addEventListener("input", runSearch);

  const cert = h("div", { class: "cert" });
  cert.append(
    h("div", { class: "cert-header" }, [h("span", { class: "label" }, "HISTORY"), search]),
    h("div", { id: "list" }),
    h("div", { id: "empty", class: "empty label", hidden: true }, "NO RECORDS"),
    h("div", { id: "footer" }),
  );
  app.append(cert);

  await reload(null);
}

main().catch(mountError);
