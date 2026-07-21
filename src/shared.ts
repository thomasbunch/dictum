// Shared across windows: theme applier, tiny DOM helper, fonts.
import "@fontsource/ibm-plex-sans/400.css";
import "@fontsource/ibm-plex-sans/600.css";
import "@fontsource/ibm-plex-mono/400.css";
import "@fontsource/ibm-plex-mono/500.css";
import { listen } from "@tauri-apps/api/event";
import { api, type Config } from "./bindings";

// ---------------------------------------------------------------------------
// h() — tiny element builder. attrs values starting with "on" + a function
// become listeners; "class" sets className; everything else is setAttribute.
// ---------------------------------------------------------------------------
type Attrs = Record<string, string | number | boolean | ((e: Event) => void) | undefined | null>;

export function h<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  attrs?: Attrs,
  children?: (Node | string)[] | string,
): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);
  if (attrs) {
    for (const [k, v] of Object.entries(attrs)) {
      if (v === undefined || v === null || v === false) continue;
      if (k.startsWith("on") && typeof v === "function") {
        el.addEventListener(k.slice(2).toLowerCase(), v as EventListener);
      } else if (k === "class") {
        el.className = String(v);
      } else if (v === true) {
        el.setAttribute(k, "");
      } else {
        el.setAttribute(k, String(v));
      }
    }
  }
  if (children != null) {
    for (const c of Array.isArray(children) ? children : [children]) el.append(c);
  }
  return el;
}

// ---------------------------------------------------------------------------
// Theme: a class on <html> swaps all 10 tokens (DESIGN §9). Instant — no
// transition (§7 M9).
// ---------------------------------------------------------------------------
export function applyTheme(cfg: Config): void {
  document.documentElement.className = `theme-${cfg.theme.toLowerCase()}`;
}

/** get_config, retried until the backend has managed AppState. Every window's
 * webview boots at startup — before .setup() finishes managing state (the same
 * race the overlay's subscribeWithRetry handles). A hard await on the first
 * stateful call would reject and blank the window forever, since main() never
 * re-runs when the window is later shown. ~4s ceiling, then let the caller
 * surface it. */
async function getConfigWhenReady(): Promise<Config> {
  for (let attempt = 0; ; attempt++) {
    try {
      return await api.getConfig();
    } catch (e) {
      if (attempt >= 40) throw e;
      await new Promise((r) => setTimeout(r, 100));
    }
  }
}

/** Fetches config, applies theme, and keeps it live via the config://changed event. */
export async function initTheme(onChange?: (cfg: Config) => void): Promise<Config> {
  const cfg = await getConfigWhenReady();
  applyTheme(cfg);
  await listen<Config>("config://changed", (e) => {
    applyTheme(e.payload);
    onChange?.(e.payload);
  });
  return cfg;
}

/** Last-resort visible error: a failed window shows a reason, not a blank page. */
export function mountError(err: unknown): void {
  const app = document.getElementById("app");
  if (!app) return;
  app.innerHTML = "";
  app.append(
    h("div", {
      style: "font-family:'IBM Plex Mono',monospace;font-size:13px;color:var(--ink);padding:16px",
    }, String(err)),
  );
}

// ---------------------------------------------------------------------------
// debounce — used by tape search-as-you-type.
// ---------------------------------------------------------------------------
export function debounce<A extends unknown[]>(fn: (...a: A) => void, ms: number): (...a: A) => void {
  let t: ReturnType<typeof setTimeout> | undefined;
  return (...a: A) => {
    clearTimeout(t);
    t = setTimeout(() => fn(...a), ms);
  };
}
