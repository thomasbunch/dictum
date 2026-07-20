// Shared across settings + history windows: theme applier, tiny DOM helper, fonts.
import "@fontsource/ibm-plex-sans/400.css";
import "@fontsource/ibm-plex-sans/600.css";
import "@fontsource/ibm-plex-mono/400.css";
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
// Theme: dataset.field drives src/tokens.css [data-field="X"] selectors.
// ---------------------------------------------------------------------------
export function applyTheme(cfg: Config): void {
  document.documentElement.dataset.field = cfg.theme;
}

/** Fetches config, applies theme, and keeps it live via the config://changed event. */
export async function initTheme(onChange?: (cfg: Config) => void): Promise<Config> {
  const cfg = await api.getConfig();
  applyTheme(cfg);
  await listen<Config>("config://changed", (e) => {
    applyTheme(e.payload);
    onChange?.(e.payload);
  });
  return cfg;
}

// ---------------------------------------------------------------------------
// debounce — used by history search-as-you-type.
// ---------------------------------------------------------------------------
export function debounce<A extends unknown[]>(fn: (...a: A) => void, ms: number): (...a: A) => void {
  let t: ReturnType<typeof setTimeout> | undefined;
  return (...a: A) => {
    clearTimeout(t);
    t = setTimeout(() => fn(...a), ms);
  };
}
