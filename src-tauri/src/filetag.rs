//! FILE TAG — spoken file names in the transcript print as `@relative/path`
//! agent mentions ("look at coordinator dot rs" -> "look @src/coordinator.rs").
//! Deterministic, no LLM — same contract as replacements.rs. Runs after
//! replacements, before inject.
//!
//! Firing rules (review-hardened — never corrupt normal prose):
//! - Runs only when the focused window title names a configured project root
//!   (word-bounded match); lookups search those roots only. No title match =
//!   feature inert for that take.
//! - A name spoken WITH its extension tags automatically: "coordinator dot rs",
//!   ASR-punctuated "coordinator.rs", or the extension as its own word
//!   ("package json").
//! - A bare stem tags only after a spoken bare "at" ("at coordinator"), which
//!   becomes the "@" — and only when the stem isn't ordinary English (STOP
//!   list): "looking at state management" stays prose; state.rs needs
//!   "state dot rs". Extensions that are English words ("go", "in", "am")
//!   likewise need their "dot" spoken — "main go" must not tag main.go.
//! - Separator words ("dot"/"underscore"/"dash") glue name parts — never at a
//!   gram edge, and a sep word right after a match kills it ("coordinator dot
//!   py" must not tag coordinator.rs). Grams never cross a sentence boundary.
//! - Unique-or-nothing; ambiguity leaves the words alone. Duplicate basenames
//!   are reachable with a parent qualifier: "audio mod dot rs" -> audio/mod.rs.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::OnceLock;

const MAX_FILES_PER_ROOT: usize = 20_000;
/// Stems shorter than this never index ("db", "ui") — too collision-prone.
const MIN_STEM: usize = 3;
/// Longest spoken form considered, in words. Multi-part names spoken with
/// their separators run long: "use dash local dash storage dot ts" = 7.
const MAX_NGRAM: usize = 10;

// --- repo-vocab: source-symbol harvesting (§repovocab) ---------------------
/// Source-file extensions scanned for definition identifiers. Sorted; binary_searched.
const SRC_EXTS: &[&str] = &[
    "c", "cc", "cjs", "cpp", "cs", "cxx", "dart", "ex", "exs", "go", "h", "hh", "hpp",
    "java", "js", "jsx", "kt", "kts", "lua", "mjs", "php", "py", "rb", "rs", "scala",
    "swift", "ts", "tsx",
];
/// Skip minified/generated blobs — a per-file read ceiling.
const MAX_FILE_BYTES: u64 = 512 * 1024;
/// Per-Index read budget: bounds the backgrounded content scan on huge repos.
const MAX_SCAN_BYTES: usize = 64 * 1024 * 1024;
/// Per-Index symbol-key cap (~2 MB of by_symbol at ~40 B each).
const MAX_SYMBOLS: usize = 50_000;
/// A distinctive (non-STOP) word-part must be at least this long to count.
const MIN_PART: usize = 2;

/// One multi-language definition-keyword alternation (NOT a parser): captures
/// the identifier after fn/func/def/class/struct/... or an `export` form.
fn symbol_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(
            r"(?m)\b(?:fn|func|def|class|struct|enum|trait|interface|impl|type|module|namespace|export\s+(?:default\s+)?(?:async\s+)?(?:function|class|interface|enum|type|const))\s+([A-Za-z_][A-Za-z0-9_]*)",
        )
        .unwrap()
    })
}

/// Split a snake/camel/kebab/acronym identifier into lowercased word-parts.
/// "handle_download"->[handle,download] "getUserConfig"->[get,user,config]
/// "HudState"->[hud,state] "HTTPServer"->[http,server] "v2"->[v] (digit dropped)
fn id_parts(id: &str) -> Vec<String> {
    let chars: Vec<char> = id.chars().collect();
    let mut parts: Vec<String> = Vec::new();
    let mut cur = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if !c.is_alphanumeric() {
            if !cur.is_empty() {
                parts.push(std::mem::take(&mut cur));
            }
            continue;
        }
        if !cur.is_empty() {
            let prev = chars[i - 1];
            // lower|digit -> Upper (camelCase), or a char-class flip (alpha<->digit),
            // or Upper -> Upper+lower (acronym tail: HTTPServer -> HTTP | Server).
            let camel = c.is_uppercase() && (prev.is_lowercase() || prev.is_numeric());
            let digit_edge = c.is_numeric() != prev.is_numeric();
            let acronym = c.is_uppercase()
                && prev.is_uppercase()
                && chars.get(i + 1).is_some_and(|n| n.is_lowercase());
            if camel || digit_edge || acronym {
                parts.push(std::mem::take(&mut cur));
            }
        }
        cur.push(c);
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    parts
        .into_iter()
        .filter(|p| !p.chars().all(|c| c.is_numeric())) // drop pure-digit parts
        .map(|p| p.to_lowercase())
        .filter(|p| !p.is_empty())
        .collect()
}

/// An identifier is worth indexing only if it has a distinctive (non-STOP) part
/// of real length — all-common ids (getUserConfig, handle_download) are dropped,
/// so ordinary prose can never resolve to them even under a cue.
fn is_correction_worthy(id: &str) -> bool {
    id_parts(id).iter().any(|p| p.len() >= MIN_PART && !is_common(p))
}

/// Common English words (and ubiquitous dev-prose nouns) that are also common
/// file stems. A bare-stem match where EVERY word is on this list never fires
/// ("at state management"), and an extension on this list never counts as
/// spoken-extension evidence ("main go"). Spelling the name out ("state dot
/// rs", "main dot go") always works — the list gates only the shortcut forms.
/// Sorted; binary-searched; a test enforces order.
const STOP: &[&str] = &[
    "about", "access", "account", "action", "actions", "active", "add", "address", "admin",
    "after", "agent", "alert", "all", "am", "amount", "analysis", "and", "animation", "answer", "any",
    "app", "apply", "apps", "archive", "area", "args", "around", "array", "arrays", "article",
    "articles", "asset", "assets", "audio", "auth", "author", "auto", "back", "backup", "badge",
    "balance", "banner", "bar", "bars", "base", "basic", "batch", "before", "best", "between",
    "bin", "binary", "block", "blocks", "blog", "board", "body", "book", "books", "boot",
    "border", "both", "box", "boxes", "branch", "browser", "buffer", "bug", "bugs", "build",
    "builder", "builds", "button", "buttons", "cache", "calendar", "call", "calls", "camera",
    "cancel", "capture", "card", "cards", "cart", "case", "cases", "catalog", "category",
    "chain", "change", "changes", "channel", "chart", "charts", "chat", "check", "checkout",
    "child", "children", "choice", "class", "classes", "clean", "clear", "click", "client",
    "clients", "clip", "clock", "close", "cloud", "code", "codes", "color", "colors", "column",
    "columns", "command", "commands", "comment", "comments", "common", "compare", "config",
    "configs", "connect", "connection", "console", "contact", "container", "content", "control",
    "controls", "cookie", "copy", "core", "count", "counter", "country", "cover", "create",
    "credit", "current", "cursor", "custom", "dash", "dashboard", "data", "database", "date",
    "dates", "day", "days", "debug", "default", "delete", "demo", "deploy", "design", "desktop",
    "detail", "details", "dev", "device", "devices", "dialog", "diff", "digest", "dir",
    "direct", "display", "dist", "do", "doc", "docs", "document", "documents", "done", "down",
    "download", "downloads", "draft", "drag", "draw", "driver", "drivers", "drop", "dump",
    "edit", "editor", "element", "elements", "email", "empty", "end", "engine", "enter",
    "entries", "entry", "env", "error", "errors", "event", "events", "example", "examples",
    "exit", "export", "extra", "fail", "false", "faq", "fast", "favorite", "feature",
    "features", "feed", "feedback", "fetch", "field", "fields", "file", "files", "fill",
    "filter", "filters", "final", "find", "first", "fix", "flag", "flags", "flat", "flow",
    "focus", "folder", "folders", "font", "fonts", "footer", "form", "format", "forms",
    "forward", "frame", "free", "front", "full", "function", "functions", "game", "games",
    "gate", "general", "get", "global", "go", "goal", "good", "graph", "grid", "group", "groups",
    "guide", "handle", "handler", "handlers", "hash", "head", "header", "headers", "health",
    "hello", "help", "helper", "helpers", "hero", "hidden", "hide", "high", "history", "home",
    "hook", "hooks", "host", "hot", "hour", "hours", "house", "icon", "icons", "image",
    "images", "import", "in", "inbox", "index", "info", "input", "inputs", "install", "is",
    "it", "item", "items", "job", "jobs", "join", "key", "keys", "label", "labels", "land",
    "landing",
    "language", "last", "layer", "layout", "left", "level", "levels", "library", "light",
    "like", "line", "lines", "link", "links", "list", "lists", "load", "loader", "loading",
    "local", "lock", "log", "login", "logo", "logout", "logs", "long", "loop", "low",
    "machine", "macro", "macros", "mail", "main", "make", "manager", "manual", "map", "maps",
    "mark", "market", "master", "match", "me", "media", "member", "members", "memory", "menu",
    "message", "messages", "meta", "method", "methods", "metric", "metrics", "middle", "mini",
    "mixin", "mobile", "mock", "mocks", "modal", "mode", "model", "models", "module",
    "modules", "money", "monitor", "month", "more", "most", "mouse", "move", "music",
    "mutation", "my", "name", "names", "nav", "network", "new", "news", "next", "night", "no",
    "node", "nodes", "normal", "note", "notes", "notification", "notifications", "number",
    "numbers", "object", "objects", "of", "offer", "office", "offline", "old", "on", "online",
    "open", "option", "options", "or", "order", "orders", "other", "out", "output", "over",
    "overlay", "owner",
    "pack", "package", "packages", "page", "pages", "panel", "paper", "parent", "parse",
    "parser", "part", "parts", "password", "past", "patch", "path", "paths", "pay", "payment",
    "payments", "people", "person", "phone", "photo", "photos", "picker", "pipe", "place",
    "plan", "plans", "play", "player", "plugin", "plugins", "point", "points", "poll", "pool",
    "popup", "port", "post", "posts", "power", "present", "preview", "price", "prices",
    "print", "private", "process", "product", "products", "profile", "program", "progress",
    "project", "projects", "prompt", "props", "proxy", "public", "pull", "push", "queries",
    "query", "queue", "quick", "radio", "random", "range", "rate", "read", "reader", "real",
    "record", "records", "redirect", "reduce", "region", "register", "release", "remote",
    "remove", "render", "report", "reports", "request", "requests", "reset", "resource",
    "resources", "response", "rest", "result", "results", "review", "reviews", "right",
    "ring", "role", "roles", "room", "root", "route", "router", "routes", "row", "rows",
    "rule", "rules", "run", "runner", "safe", "sample", "samples", "save", "scale", "scan",
    "schema", "screen", "script", "scripts", "scroll", "search", "second", "section",
    "sections", "secure", "security", "select", "send", "server", "servers", "service",
    "services", "session", "sessions", "set", "setting", "settings", "setup", "share",
    "shared", "sheet", "shell", "shift", "ship", "shop", "short", "show", "side", "sidebar",
    "sign", "signal", "simple", "single", "site", "size", "skill", "skills", "slide",
    "slider", "slow", "small", "smart", "so", "social", "sort", "sound", "sounds", "source",
    "sources", "space", "spec", "specs", "speed", "split", "sport", "stack", "staff", "stage",
    "start", "startup", "stat", "state", "states", "static", "stats", "status", "step",
    "steps", "stock", "stop", "storage", "store", "stores", "story", "stream", "string",
    "strings", "strip", "style", "styles", "submit", "sum", "summary", "support", "switch",
    "sync", "system", "tab", "table", "tables", "tabs", "tag", "tags", "tail", "task",
    "tasks", "team", "teams", "temp", "template", "templates", "terminal", "test", "tests",
    "text", "texts", "theme", "themes", "thread", "threads", "ticket", "tickets", "time",
    "timer", "times", "tip", "tips", "title", "titles", "to", "todo", "toggle", "token", "tokens",
    "tool", "toolbar", "tools", "top", "total", "touch", "trace", "track", "tracks", "train",
    "transfer", "tray", "tree", "trigger", "true", "type", "types", "under", "unit", "units",
    "up", "update", "updates", "upload", "uploads", "url", "urls", "use", "user", "users",
    "util", "utils", "value", "values", "version", "versions", "video", "videos", "view",
    "views", "wallet", "watch", "water", "we", "web", "week", "welcome", "widget", "widgets",
    "window", "windows", "word", "words", "work", "worker", "workers", "world", "wrap",
    "wrapper", "write", "year", "zone",
];

fn is_common(normalized: &str) -> bool {
    STOP.binary_search(&normalized).is_ok()
}

// ---------------------------------------------------------------------------
// Index
// ---------------------------------------------------------------------------

struct RootIndex {
    /// Last path component of the root, lowercased — matched (word-bounded)
    /// against the focused window title to activate this root.
    name: String,
    /// Relative paths, forward slashes.
    files: Vec<String>,
    /// normalized file name ("coordinatorrs") -> indices into `files`; also
    /// parent-qualified ("audiomodrs") so duplicate basenames stay reachable.
    by_name: HashMap<String, Vec<u32>>,
    /// normalized stem ("coordinator", "audiomod") -> indices into `files`.
    by_stem: HashMap<String, Vec<u32>>,
    /// Extensions present in this root ("rs", "json") — lets an extension
    /// spoken as its own word ("package json") count as a full name.
    exts: HashSet<String>,
    /// normalize(id) -> distinct canonical spellings of harvested repo symbols
    /// (repo-vocab). Two spellings under one key => cue lookup returns nothing.
    by_symbol: HashMap<String, Vec<String>>,
}

impl RootIndex {
    fn from_files(name: &str, files: Vec<String>, symbols: Vec<String>) -> Self {
        let mut by_name: HashMap<String, Vec<u32>> = HashMap::new();
        let mut by_stem: HashMap<String, Vec<u32>> = HashMap::new();
        let mut exts: HashSet<String> = HashSet::new();
        for (i, rel) in files.iter().enumerate() {
            let p = Path::new(rel);
            let parent = p
                .parent()
                .and_then(|d| d.file_name())
                .map(|d| normalize(&d.to_string_lossy()))
                .unwrap_or_default();
            if let Some(f) = p.file_name() {
                let k = normalize(&f.to_string_lossy());
                if !k.is_empty() {
                    by_name.entry(k.clone()).or_default().push(i as u32);
                    if !parent.is_empty() {
                        by_name.entry(format!("{parent}{k}")).or_default().push(i as u32);
                    }
                }
            }
            if let Some(s) = p.file_stem() {
                let k = normalize(&s.to_string_lossy());
                if k.len() >= MIN_STEM {
                    by_stem.entry(k.clone()).or_default().push(i as u32);
                }
                if !parent.is_empty() && !k.is_empty() {
                    by_stem.entry(format!("{parent}{k}")).or_default().push(i as u32);
                }
            }
            if let Some(e) = p.extension() {
                let k = normalize(&e.to_string_lossy());
                if !k.is_empty() {
                    exts.insert(k);
                }
            }
        }
        let mut by_symbol: HashMap<String, Vec<String>> = HashMap::new();
        for id in symbols {
            let key = normalize(&id);
            if key.len() < MIN_STEM {
                continue;
            }
            let v = by_symbol.entry(key).or_default();
            if !v.iter().any(|s| s == &id) {
                v.push(id); // distinct-on-insert; repeats of one spelling dedup to 1
            }
        }
        Self { name: name.to_lowercase(), files, by_name, by_stem, exts, by_symbol }
    }
}

#[derive(Default)]
pub struct Index {
    roots: Vec<RootIndex>,
}

impl Index {
    /// Harvested repo symbols worth biasing the recogniser toward.
    ///
    /// Deliberately a small subset. A hotword boosts a spelling the acoustic
    /// model was already considering, so it only helps for an identifier the
    /// user SPEAKS AS ONE WORD — `zipformer`, `axum`, `serde`. A multi-part
    /// identifier is spoken as its parts ("hud state" for `HudState`), and the
    /// model emits those parts correctly already; biasing toward the glued
    /// spelling asks for a token speech never produces, while biasing toward the
    /// parts would boost ordinary English. Those are `apply_symbols`' job, on the
    /// text side, where the cue supplies the evidence biasing can't.
    ///
    /// So: single-part, distinctive, and long enough to be worth a graph node.
    pub fn hotwords(&self) -> Vec<String> {
        let mut out = Vec::new();
        for root in &self.roots {
            for spellings in root.by_symbol.values() {
                // Ambiguous keys (two spellings) are exactly what the text-side
                // pass refuses to guess at; don't guess here either.
                let [id] = spellings.as_slice() else { continue };
                let parts = id_parts(id);
                let [part] = parts.as_slice() else { continue };
                if part.len() >= MIN_HOTWORD && !is_common(part) {
                    out.push(id.clone());
                }
            }
        }
        out
    }
}

/// Shorter than this and a hotword boosts far more than it corrects.
const MIN_HOTWORD: usize = 4;

impl Index {
    /// Walk each root (gitignore-aware, hidden files skipped) and index every
    /// file name. A repo walk is ms-scale; callers rebuild per session.
    pub fn build(roots: &[String]) -> Index {
        let mut out = Index::default();
        let mut scanned = 0usize; // per-Index content-read budget (MAX_SCAN_BYTES)
        for root in roots {
            let mut files = Vec::new();
            let mut syms: Vec<String> = Vec::new();
            let walker = ignore::WalkBuilder::new(root)
                // Honor .gitignore even in folders that aren't git repos.
                .require_git(false)
                .filter_entry(|e| {
                    let is_dir = e.file_type().is_some_and(|t| t.is_dir());
                    let n = e.file_name().to_string_lossy();
                    // Junk dirs that dominate walks when a root has no .gitignore.
                    !(is_dir && ["node_modules", "target", "dist", "build", ".git"].contains(&n.as_ref()))
                })
                .build();
            for entry in walker.flatten() {
                if files.len() >= MAX_FILES_PER_ROOT {
                    break; // ponytail: hard cap, no warning surface; add one if real roots hit it
                }
                if !entry.file_type().is_some_and(|t| t.is_file()) {
                    continue;
                }
                if let Ok(rel) = entry.path().strip_prefix(root) {
                    files.push(rel.to_string_lossy().replace('\\', "/"));
                }
                // repo-vocab: harvest definition identifiers from allowlisted
                // source files. Bounded by byte/file/symbol caps; over-capture
                // (comments/strings) is safe — the cue + distinctive + unique
                // gates contain it. // ponytail: rebuild-per-session, no watcher.
                if scanned < MAX_SCAN_BYTES && syms.len() < MAX_SYMBOLS {
                    if let Some(ext) = entry.path().extension().and_then(|e| e.to_str()) {
                        let e = ext.to_ascii_lowercase();
                        if SRC_EXTS.binary_search(&e.as_str()).is_ok()
                            && entry.metadata().map(|m| m.len() <= MAX_FILE_BYTES).unwrap_or(false)
                        {
                            if let Ok(bytes) = std::fs::read(entry.path()) {
                                scanned += bytes.len();
                                let text = String::from_utf8_lossy(&bytes);
                                for cap in symbol_re().captures_iter(&text) {
                                    if syms.len() >= MAX_SYMBOLS {
                                        break;
                                    }
                                    let id = &cap[1];
                                    if is_correction_worthy(id) {
                                        syms.push(id.to_string());
                                    }
                                }
                            }
                        }
                    }
                }
            }
            let name = Path::new(root)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            out.roots.push(RootIndex::from_files(&name, files, syms));
        }
        out
    }
}

/// Lowercase, alphanumerics only — "Level_Bar.rs" and "level bar dot rs" both
/// normalize to "levelbarrs".
fn normalize(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
}

/// Spoken separator words: contribute nothing to the normalized key but join
/// their neighbors ("level bar dot rs" -> levelbarrs).
fn is_sep_word(w: &str) -> bool {
    matches!(w.to_lowercase().as_str(), "dot" | "period" | "underscore" | "dash" | "hyphen")
}

// ---------------------------------------------------------------------------
// Matching
// ---------------------------------------------------------------------------

struct Tok<'a> {
    raw_start: usize,
    raw_end: usize,
    /// Byte offsets of `word` inside the text (wrapping punctuation excluded —
    /// it survives outside the replacement span).
    word_start: usize,
    trimmed_end: usize,
    /// Raw token ended with . ! ? — a sentence boundary grams must not cross.
    sentence_end: bool,
    /// A pure-punctuation token followed this one and was dropped ("at . Bar",
    /// "at !!! name"). Grams and the at-absorb must not silently cross it.
    gap_after: bool,
    word: &'a str,
}

const LEAD_TRIM: &[char] = &['(', '[', '{', '"', '\'', '\u{201C}', '\u{2018}', '\u{00AB}', '\u{2039}', '\u{300C}', '\u{300E}', '\u{FF08}'];
const TRAIL_TRIM: &[char] = &[
    ',', '.', ';', ':', '!', '?', ')', ']', '}', '"', '\'', '\u{201D}', '\u{2019}', '\u{00BB}',
    '\u{203A}', '\u{300D}', '\u{300F}', '\u{FF09}', '\u{2026}',
];

fn tokenize(text: &str) -> Vec<Tok<'_>> {
    let mut toks = Vec::new();
    let mut start: Option<usize> = None;
    for (i, c) in text.char_indices() {
        if c.is_whitespace() {
            if let Some(s) = start.take() {
                push_tok(text, s, i, &mut toks);
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(s) = start {
        push_tok(text, s, text.len(), &mut toks);
    }
    toks
}

fn push_tok<'a>(text: &'a str, s: usize, e: usize, toks: &mut Vec<Tok<'a>>) {
    // ASR punctuation stays outside the tag: "(coordinator.rs)," matches
    // coordinator.rs and the parens + comma survive in place.
    let raw = &text[s..e];
    let lead = raw.trim_start_matches(LEAD_TRIM);
    let word = lead.trim_end_matches(TRAIL_TRIM);
    if word.is_empty() {
        // Detached punctuation ("." "!!!" "…"): keep its signal on the previous
        // token instead of silently vanishing — its bytes must never end up
        // inside a replacement span.
        if let Some(prev) = toks.last_mut() {
            prev.gap_after = true;
            prev.sentence_end |= raw.contains(['.', '!', '?']);
        }
        return;
    }
    let word_start = s + (raw.len() - lead.len());
    let trimmed_end = word_start + word.len();
    let sentence_end = text[trimmed_end..e].contains(['.', '!', '?']);
    toks.push(Tok { raw_start: s, raw_end: e, word_start, trimmed_end, sentence_end, gap_after: false, word });
}

/// A token that can be part of a spoken file name: letters, digits, and name
/// punctuation only. Anything else glued on (emoji, em-dash) disqualifies the
/// token rather than getting silently deleted by the replacement span.
fn is_nameish(word: &str) -> bool {
    word.chars().all(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '-' | '\''))
}

/// `needle` appears in `hay` bounded by non-alphanumerics — root "app" must not
/// activate on a "WhatsApp" title. Both args lowercased by the caller.
fn contains_word(hay: &str, needle: &str) -> bool {
    let mut from = 0;
    while let Some(pos) = hay[from..].find(needle) {
        let start = from + pos;
        let end = start + needle.len();
        let before = hay[..start].chars().next_back().map_or(true, |c| !c.is_alphanumeric());
        let after = hay[end..].chars().next().map_or(true, |c| !c.is_alphanumeric());
        if before && after {
            return true;
        }
        from = end;
    }
    false
}

/// Roots whose folder name appears (word-bounded) in the focused window title.
/// VS Code, Cursor, and terminals all print the folder there. Empty = the
/// feature stays inert for this take.
fn active_roots(index: &Index, title: Option<&str>) -> Vec<usize> {
    let Some(t) = title else { return Vec::new() };
    let t = t.to_lowercase();
    index
        .roots
        .iter()
        .enumerate()
        .filter(|(_, r)| !r.name.is_empty() && contains_word(&t, &r.name))
        .map(|(i, _)| i)
        .collect()
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Name,
    Stem,
}

/// Resolve one n-gram within the active roots — `Some` only on a unique hit.
fn lookup(index: &Index, active: &[usize], gram: &[Tok]) -> Option<(String, Kind)> {
    let mut key = String::new();
    for t in gram {
        if !is_sep_word(t.word) {
            key.push_str(&normalize(t.word));
        }
    }
    if key.len() < MIN_STEM {
        return None;
    }
    let mut hits: Vec<(usize, u32, Kind)> = Vec::new();
    for &ri in active {
        let root = &index.roots[ri];
        for &fi in root.by_name.get(&key).into_iter().flatten() {
            hits.push((ri, fi, Kind::Name));
        }
        // Extensionless files ("LICENSE") land in both maps — keep the Name entry.
        for &fi in root.by_stem.get(&key).into_iter().flatten() {
            if !hits.iter().any(|&(r, f, _)| (r, f) == (ri, fi)) {
                hits.push((ri, fi, Kind::Stem));
            }
        }
    }
    // ponytail: unique-or-nothing; add ranking (recency, path depth) only if
    // ambiguity proves common in real use.
    match hits[..] {
        [(ri, fi, kind)] => Some((index.roots[ri].files[fi as usize].clone(), kind)),
        _ => None,
    }
}

/// A spoken word with no wrapping/adjacent punctuation and not sentence-final —
/// the shape that lets "at" become "@" (and a cue word arm a symbol rewrite).
fn is_bare_word(t: &Tok) -> bool {
    t.word_start == t.raw_start && t.trimmed_end == t.raw_end && !t.gap_after && !t.sentence_end
}

/// The spoken "@": a bare "at" with no punctuation on either side and no
/// dropped punctuation between it and what follows ("at . Coordinator").
fn is_bare_at(t: &Tok) -> bool {
    t.word.eq_ignore_ascii_case("at") && is_bare_word(t)
}

/// An extension spoken as its own word counts as full-name evidence only when
/// it's a real extension in the active roots AND not an English word — "main
/// go" must not tag main.go, but "package json" tags package.json.
fn last_is_ext(index: &Index, active: &[usize], word: &str) -> bool {
    let k = normalize(word);
    k.len() >= 2 && !is_common(&k) && active.iter().any(|&ri| index.roots[ri].exts.contains(&k))
}

/// At least one non-separator word in the gram is not ordinary English — the
/// gate that keeps "at state management" prose while "at coordinator" tags.
fn has_distinctive_word(gram: &[Tok]) -> bool {
    gram.iter().filter(|t| !is_sep_word(t.word)).any(|t| !is_common(&normalize(t.word)))
}

/// Rewrite spoken file names in `text` as `@relpath` tags per the module rules.
/// A directly preceding bare "at" is absorbed into the "@".
pub fn apply(text: &str, index: &Index, window_title: Option<&str>) -> String {
    let active = active_roots(index, window_title);
    if active.is_empty() || active.iter().all(|&ri| index.roots[ri].files.is_empty()) {
        return text.to_string();
    }
    let toks = tokenize(text);
    let mut out = String::with_capacity(text.len() + 32);
    let mut cursor = 0usize;
    let mut i = 0usize;
    'outer: while i < toks.len() {
        // Typed tags pass through; sep words can't start a name; a sep word
        // right BEFORE this token was glue for something that didn't match.
        if toks[i].word.starts_with('@')
            || is_sep_word(toks[i].word)
            || (i > 0 && is_sep_word(toks[i - 1].word))
        {
            i += 1;
            continue;
        }
        for n in (1..=MAX_NGRAM.min(toks.len() - i)).rev() {
            let gram = &toks[i..i + n];
            // Sep words only glue name parts (never a gram edge); grams never
            // cross an ASR-printed sentence boundary or dropped punctuation,
            // and every token must be name-shaped (no glued emoji/dashes).
            if is_sep_word(gram[n - 1].word)
                || gram[..n - 1].iter().any(|t| t.sentence_end || t.gap_after)
                || gram.iter().any(|t| !is_nameish(t.word))
            {
                continue;
            }
            // A sep word right AFTER the gram is a spoken extension that did
            // not match — tagging would name the wrong file ("coordinator dot py").
            if toks.get(i + n).is_some_and(|t| is_sep_word(t.word)) {
                continue;
            }
            let Some((path, kind)) = lookup(index, &active, gram) else { continue };
            // Absorbing the "at" must not delete bytes between it and the gram
            // (a leading paren/quote on the target keeps the "at" in place).
            let at_prev = i > 0
                && is_bare_at(&toks[i - 1])
                && toks[i - 1].raw_start >= cursor
                && gram[0].word_start == gram[0].raw_start;
            // Automatic only with an extension signal; bare stems need the
            // spoken "at" AND a word that isn't ordinary English — dictionary
            // stems litter normal prose ("looking at state management").
            let auto = kind == Kind::Name
                && (gram.iter().any(|t| is_sep_word(t.word))
                    || (n == 1 && gram[0].word.contains('.'))
                    || (n > 1 && last_is_ext(index, &active, gram[n - 1].word)));
            if !auto && !(at_prev && has_distinctive_word(gram)) {
                continue;
            }
            let span_start = if at_prev { toks[i - 1].word_start } else { gram[0].word_start };
            out.push_str(&text[cursor..span_start]);
            out.push('@');
            out.push_str(&path);
            cursor = gram[n - 1].trimmed_end;
            i += n;
            continue 'outer;
        }
        i += 1;
    }
    out.push_str(&text[cursor..]);
    out
}

// ---------------------------------------------------------------------------
// repo-vocab: cue-gated identifier correction (runs LAST, after apply)
// ---------------------------------------------------------------------------

/// Unique canonical spelling for `key` across the active roots, or None on
/// ambiguity (two spellings) or miss (filetag's Vec slice-match idiom).
fn lookup_symbol(index: &Index, active: &[usize], key: &str) -> Option<String> {
    let mut hits: Vec<&str> = Vec::new();
    for &ri in active {
        for s in index.roots[ri].by_symbol.get(key).into_iter().flatten() {
            if !hits.iter().any(|h| *h == s.as_str()) {
                hits.push(s);
            }
        }
    }
    match hits[..] {
        [one] => Some(one.to_string()),
        _ => None,
    }
}

/// Rewrite a spoken identifier to its exact repo-symbol spelling, but ONLY the
/// run of words immediately following a spoken `cue` word — mirroring filetag's
/// "at". Distinctive + unique-or-nothing + boundary-safe. No cue token in the
/// utterance => `text` is returned unchanged (prose corruption impossible). The
/// cue is absorbed (dropped) on a successful match, like "at" -> "@".
pub fn apply_symbols(text: &str, index: &Index, window_title: Option<&str>, cue: &str) -> String {
    let cue_n = normalize(cue);
    if cue_n.is_empty() {
        return text.to_string();
    }
    let active = active_roots(index, window_title);
    if active.is_empty() || active.iter().all(|&ri| index.roots[ri].by_symbol.is_empty()) {
        return text.to_string();
    }
    let toks = tokenize(text);
    let mut out = String::with_capacity(text.len() + 16);
    let mut cursor = 0usize;
    let mut i = 0usize;
    'outer: while i < toks.len() {
        // A bare cue word (no wrapping punctuation, directly adjacent to what
        // follows) arms a rewrite of the run of words after it.
        if normalize(toks[i].word) == cue_n && is_bare_word(&toks[i]) {
            let avail = toks.len() - (i + 1);
            for n in (1..=MAX_NGRAM.min(avail)).rev() {
                let gram = &toks[i + 1..i + 1 + n];
                // Sep words never sit at a gram edge; grams never cross a
                // sentence boundary or dropped punctuation; every token must be
                // name-shaped (skips filetag's @path tokens — they contain '/').
                if is_sep_word(gram[0].word)
                    || is_sep_word(gram[n - 1].word)
                    || gram[..n - 1].iter().any(|t| t.sentence_end || t.gap_after)
                    || gram.iter().any(|t| !is_nameish(t.word))
                {
                    continue;
                }
                // Redundant with the index's distinctive filter, kept as
                // defense-in-depth so a common-word cue can't misfire.
                if !has_distinctive_word(gram) {
                    continue;
                }
                let key: String = gram
                    .iter()
                    .filter(|t| !is_sep_word(t.word))
                    .map(|t| normalize(t.word))
                    .collect();
                let Some(canon) = lookup_symbol(index, &active, &key) else { continue };
                // Absorb the cue; keep any leading punctuation on the target so
                // "symbol (zipformer)" -> "(zipformer)" (bytes never dropped).
                out.push_str(&text[cursor..toks[i].word_start]);
                out.push_str(&text[gram[0].raw_start..gram[0].word_start]);
                out.push_str(&canon);
                cursor = gram[n - 1].trimmed_end;
                i += 1 + n;
                continue 'outer;
            }
        }
        i += 1;
    }
    out.push_str(&text[cursor..]);
    out
}

// ---------------------------------------------------------------------------
// Win32: focused window title (root activation)
// ---------------------------------------------------------------------------

pub fn window_title(hwnd: isize) -> Option<String> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::GetWindowTextW;
    let mut buf = [0u16; 512];
    let len = unsafe { GetWindowTextW(HWND(hwnd as *mut core::ffi::c_void), &mut buf) };
    if len <= 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buf[..len as usize]))
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const TITLE: Option<&str> = Some("main.rs — Dictum — Visual Studio Code");

    fn idx(roots: &[(&str, &[&str])]) -> Index {
        Index {
            roots: roots
                .iter()
                .map(|(name, files)| {
                    RootIndex::from_files(name, files.iter().map(|s| s.to_string()).collect(), vec![])
                })
                .collect(),
        }
    }

    /// Like `idx` but seeds one root with harvested repo symbols, for the
    /// cue-gated `apply_symbols` tests.
    fn idx_syms(name: &str, files: &[&str], symbols: &[&str]) -> Index {
        Index {
            roots: vec![RootIndex::from_files(
                name,
                files.iter().map(|s| s.to_string()).collect(),
                symbols.iter().map(|s| s.to_string()).collect(),
            )],
        }
    }

    fn dictum() -> Index {
        idx(&[(
            "dictum",
            &[
                "src-tauri/src/coordinator.rs",
                "src-tauri/src/level_bar.rs",
                "package.json",
                "README.md",
                "LICENSE",
                "db.rs",
                "main.html",
            ],
        )])
    }

    #[test]
    fn spoken_dot_extension() {
        assert_eq!(
            apply("open coordinator dot rs please", &dictum(), TITLE),
            "open @src-tauri/src/coordinator.rs please"
        );
    }

    #[test]
    fn asr_punctuated_name_with_comma() {
        assert_eq!(
            apply("Open coordinator.rs, then run it", &dictum(), TITLE),
            "Open @src-tauri/src/coordinator.rs, then run it"
        );
    }

    #[test]
    fn bare_stem_requires_spoken_at() {
        // Prose stays prose — "coordinator" is also an English word.
        assert_eq!(apply("open coordinator now", &dictum(), TITLE), "open coordinator now");
        // The spoken "at" is the opt-in, and it becomes the "@".
        assert_eq!(
            apply("look at coordinator", &dictum(), TITLE),
            "look @src-tauri/src/coordinator.rs"
        );
        assert_eq!(apply("at readme", &dictum(), TITLE), "@README.md");
    }

    #[test]
    fn wrong_extension_never_mistagged() {
        // coordinator.py is not in the index — the .rs must NOT be tagged.
        assert_eq!(
            apply("open coordinator dot py", &dictum(), TITLE),
            "open coordinator dot py"
        );
        assert_eq!(
            apply("at coordinator dot py", &dictum(), TITLE),
            "at coordinator dot py"
        );
        assert_eq!(
            apply("check level bar dot ts", &dictum(), TITLE),
            "check level bar dot ts"
        );
    }

    #[test]
    fn snake_case_multiword() {
        assert_eq!(
            apply("check level bar dot rs", &dictum(), TITLE),
            "check @src-tauri/src/level_bar.rs"
        );
        // Bare multi-word stem: needs the spoken "at" AND a distinctive word —
        // "level" and "bar" are both ordinary English, so even "at level bar"
        // stays prose. The extension form always works.
        assert_eq!(apply("check level bar", &dictum(), TITLE), "check level bar");
        assert_eq!(apply("at level bar", &dictum(), TITLE), "at level bar");
    }

    #[test]
    fn at_common_english_stem_stays_prose() {
        let i = idx(&[(
            "dictum",
            &["src/error.rs", "src/state.rs", "src/startup.rs", "src/main.rs"],
        )]);
        for s in [
            "look at error handling in the retry loop",
            "we are looking at state management",
            "look at startup performance",
            "the bottleneck is at startup takes ten seconds",
        ] {
            assert_eq!(apply(s, &i, TITLE), s);
        }
        // The explicit forms still reach those files.
        assert_eq!(apply("open state dot rs", &i, TITLE), "open @src/state.rs");
    }

    #[test]
    fn wordlike_extension_requires_dot() {
        let i = idx(&[("dictum", &["cmd/main.go", "Makefile.am", "Makefile.in", "configure.ac"])]);
        for s in [
            "let's make main go faster",
            "check the makefile in the root",
            "which makefile am I supposed to edit",
        ] {
            assert_eq!(apply(s, &i, TITLE), s);
        }
        assert_eq!(apply("open main dot go", &i, TITLE), "open @cmd/main.go");
        assert_eq!(apply("open makefile dot am", &i, TITLE), "open @Makefile.am");
    }

    #[test]
    fn parent_qualified_common_words_stay_prose() {
        let i = idx(&[("dictum", &["src/main/setup.ts", "src/main/index.ts", "README.md"])]);
        assert_eq!(
            apply("let's look at main setup logic", &i, TITLE),
            "let's look at main setup logic"
        );
        // The qualified extension form still resolves the duplicate.
        assert_eq!(apply("open main setup dot ts", &i, TITLE), "open @src/main/setup.ts");
    }

    #[test]
    fn long_multi_separator_names_fire() {
        let i = idx(&[(
            "dictum",
            &["src/use-local-storage.ts", "src-tauri/src/audio_level_bar.rs"],
        )]);
        assert_eq!(
            apply("open use dash local dash storage dot ts", &i, TITLE),
            "open @src/use-local-storage.ts"
        );
        assert_eq!(
            apply("at use dash local dash storage dot ts", &i, TITLE),
            "@src/use-local-storage.ts"
        );
        assert_eq!(
            apply("check audio underscore level underscore bar dot rs", &i, TITLE),
            "check @src-tauri/src/audio_level_bar.rs"
        );
    }

    #[test]
    fn detached_punctuation_blocks_and_survives() {
        // Detached "." — sentence boundary must hold and bytes must survive.
        assert_eq!(
            apply("that is what I am looking at . Coordinator handles retries", &dictum(), TITLE),
            "that is what I am looking at . Coordinator handles retries"
        );
        assert_eq!(
            apply("at level . Bar none it works", &dictum(), TITLE),
            "at level . Bar none it works"
        );
        // Detached "!!!" between "at" and an auto match: preserved, not absorbed.
        assert_eq!(
            apply("look at !!! coordinator dot rs", &dictum(), TITLE),
            "look at !!! @src-tauri/src/coordinator.rs"
        );
    }

    #[test]
    fn at_never_absorbs_across_wrapping_punctuation() {
        assert_eq!(
            apply("look at (coordinator.rs) now", &dictum(), TITLE),
            "look at (@src-tauri/src/coordinator.rs) now"
        );
        assert_eq!(
            apply("see \u{AB}coordinator.rs\u{BB} for details", &dictum(), TITLE),
            "see \u{AB}@src-tauri/src/coordinator.rs\u{BB} for details"
        );
    }

    #[test]
    fn glued_junk_disqualifies_the_token() {
        // Emoji glued to the name: leave the token entirely alone rather than
        // silently deleting user content.
        assert_eq!(
            apply("open \u{1F680}coordinator.rs now", &dictum(), TITLE),
            "open \u{1F680}coordinator.rs now"
        );
    }

    #[test]
    fn stop_list_is_sorted_and_deduped() {
        assert!(
            STOP.windows(2).all(|w| w[0] < w[1]),
            "STOP must be strictly sorted for binary_search"
        );
    }

    #[test]
    fn extension_as_spoken_word() {
        assert_eq!(apply("update package json", &dictum(), TITLE), "update @package.json");
        assert_eq!(apply("open main html", &dictum(), TITLE), "open @main.html");
    }

    #[test]
    fn extensionless_file_requires_at() {
        // "license" is ordinary prose ("under the MIT license").
        assert_eq!(apply("read the license", &dictum(), TITLE), "read the license");
        assert_eq!(apply("at license", &dictum(), TITLE), "@LICENSE");
    }

    #[test]
    fn sentence_punctuation_survives() {
        assert_eq!(
            apply("open coordinator dot rs.", &dictum(), TITLE),
            "open @src-tauri/src/coordinator.rs."
        );
    }

    #[test]
    fn wrapping_punctuation_survives() {
        assert_eq!(
            apply("see (coordinator.rs) for details", &dictum(), TITLE),
            "see (@src-tauri/src/coordinator.rs) for details"
        );
        assert_eq!(
            apply("the file \"coordinator.rs\" is big", &dictum(), TITLE),
            "the file \"@src-tauri/src/coordinator.rs\" is big"
        );
    }

    #[test]
    fn at_with_punctuation_is_not_absorbed() {
        // "at," is prose punctuation, not the spoken "@".
        assert_eq!(
            apply("look at, coordinator dot rs", &dictum(), TITLE),
            "look at, @src-tauri/src/coordinator.rs"
        );
        // Sentence-final "at." followed by a capitalized noun must not merge.
        assert_eq!(
            apply("that is what I am looking at. Coordinator handles retries", &dictum(), TITLE),
            "that is what I am looking at. Coordinator handles retries"
        );
    }

    #[test]
    fn prose_common_words_untouched() {
        // Index shaped like this actual repo — every phrase is everyday prose
        // that previously misfired (adversarial review findings).
        let i = idx(&[(
            "dictum",
            &[
                "src/main/setup.ts",
                "README.md",
                "src-tauri/src/filetag.rs",
                "src-tauri/src/inject/sendinput.rs",
                "src-tauri/build.rs",
                "package.json",
                "PLAN.md",
                "DESIGN.md",
            ],
        )]);
        for s in [
            "let me set up the meeting",
            "read me the last message",
            "the file tag feature",
            "send input to the window",
            "the build is failing",
            "install the package",
            "the plan is to ship Friday",
            "the design looks good",
        ] {
            assert_eq!(apply(s, &i, TITLE), s);
        }
    }

    #[test]
    fn period_and_dash_nouns_survive() {
        let i = idx(&[("dictum", &["docs/plan.md", "src/salt.rs"])]);
        assert_eq!(apply("trial period plan review", &i, TITLE), "trial period plan review");
        assert_eq!(apply("review the plan period done", &i, TITLE), "review the plan period done");
        assert_eq!(apply("add a dash of salt here", &i, TITLE), "add a dash of salt here");
    }

    #[test]
    fn gram_never_crosses_sentence_boundary() {
        // "level. Bar" is two sentences, not level_bar.rs.
        assert_eq!(
            apply("at level. Bar none it works", &dictum(), TITLE),
            "at level. Bar none it works"
        );
    }

    #[test]
    fn duplicate_basenames_reachable_via_parent() {
        let i = idx(&[(
            "dictum",
            &["src-tauri/src/audio/mod.rs", "src-tauri/src/inject/mod.rs"],
        )]);
        // Bare "mod dot rs" is ambiguous — untouched.
        assert_eq!(apply("open mod dot rs", &i, TITLE), "open mod dot rs");
        // One spoken path segment resolves it.
        assert_eq!(
            apply("open audio mod dot rs", &i, TITLE),
            "open @src-tauri/src/audio/mod.rs"
        );
    }

    #[test]
    fn no_title_match_means_inert() {
        // No title at all, or a title that names no root: never tag.
        assert_eq!(
            apply("open coordinator dot rs", &dictum(), None),
            "open coordinator dot rs"
        );
        assert_eq!(
            apply("open coordinator dot rs", &dictum(), Some("Inbox — Gmail — Chrome")),
            "open coordinator dot rs"
        );
    }

    #[test]
    fn title_match_is_word_bounded() {
        let i = idx(&[("app", &["src/coordinator.rs"])]);
        assert_eq!(
            apply("open coordinator dot rs", &i, Some("WhatsApp — Chrome")),
            "open coordinator dot rs"
        );
        assert_eq!(
            apply("open coordinator dot rs", &i, Some("main.rs — app — Code")),
            "open @src/coordinator.rs"
        );
    }

    #[test]
    fn lookups_stay_inside_the_active_root() {
        let i = idx(&[("alpha", &["src/coordinator.rs"]), ("beta", &["lib/other.rs"])]);
        // Focused on beta: alpha's files are out of scope — a relative path
        // from another root would resolve against the wrong cwd.
        assert_eq!(
            apply("open coordinator dot rs", &i, Some("shell — beta")),
            "open coordinator dot rs"
        );
        assert_eq!(
            apply("open coordinator dot rs", &i, Some("shell — alpha")),
            "open @src/coordinator.rs"
        );
    }

    #[test]
    fn title_disambiguates_across_roots() {
        let i = idx(&[
            ("alpha", &["src/coordinator.rs"]),
            ("beta", &["lib/coordinator.rs"]),
        ]);
        assert_eq!(
            apply("open coordinator dot rs", &i, Some("x — beta — Visual Studio Code")),
            "open @lib/coordinator.rs"
        );
        // Title naming both roots -> ambiguous -> untouched.
        assert_eq!(
            apply("open coordinator dot rs", &i, Some("alpha and beta")),
            "open coordinator dot rs"
        );
    }

    #[test]
    fn already_typed_tag_untouched() {
        assert_eq!(apply("@README.md is fine", &dictum(), TITLE), "@README.md is fine");
    }

    #[test]
    fn multiple_tags_in_one_take() {
        assert_eq!(
            apply("compare coordinator dot rs with main dot html", &dictum(), TITLE),
            "compare @src-tauri/src/coordinator.rs with @main.html"
        );
    }

    #[test]
    fn short_stem_never_fires_bare() {
        assert_eq!(apply("at db", &dictum(), TITLE), "at db"); // < MIN_STEM
        assert_eq!(apply("check db dot rs now", &dictum(), TITLE), "check @db.rs now");
    }

    #[test]
    fn walk_respects_gitignore_and_junk_dirs() {
        let root = std::env::temp_dir().join(format!("dictum-filetag-{}", std::process::id()));
        let make = |p: &str| {
            let f = root.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, "x").unwrap();
        };
        make("src/coordinator.rs");
        make("node_modules/junk/pkg.js");
        make("secret.log");
        std::fs::write(root.join(".gitignore"), "*.log\n").unwrap();

        let i = Index::build(&[root.to_string_lossy().into_owned()]);
        let files = &i.roots[0].files;
        assert!(files.contains(&"src/coordinator.rs".to_string()), "{files:?}");
        assert!(!files.iter().any(|f| f.contains("node_modules")), "{files:?}");
        assert!(!files.iter().any(|f| f.ends_with(".log")), "{files:?}");

        std::fs::remove_dir_all(&root).unwrap();
    }

    // -----------------------------------------------------------------------
    // repo-vocab: symbol harvest + cue-gated apply_symbols
    // -----------------------------------------------------------------------

    #[test]
    fn src_exts_sorted() {
        assert!(SRC_EXTS.windows(2).all(|w| w[0] < w[1]), "SRC_EXTS must be sorted for binary_search");
    }

    #[test]
    fn id_parts_and_worthiness() {
        assert_eq!(id_parts("handle_download"), ["handle", "download"]);
        assert_eq!(id_parts("getUserConfig"), ["get", "user", "config"]);
        assert_eq!(id_parts("HudState"), ["hud", "state"]);
        assert_eq!(id_parts("HTTPServer"), ["http", "server"]);
        assert_eq!(id_parts("parse_zipformer"), ["parse", "zipformer"]);
        assert_eq!(id_parts("zipformer"), ["zipformer"]);
        assert_eq!(id_parts("v2"), ["v"]); // pure-digit part dropped

        // Distinctive part => worthy.
        assert!(is_correction_worthy("parse_zipformer"));
        assert!(is_correction_worthy("HudState"));
        // DataProcessor is worthy because "processor" is NOT in STOP — this locks
        // in that the index filter alone does NOT save "the data processor was
        // slow"; only the cue does.
        assert!(is_correction_worthy("DataProcessor"));
        assert!(is_correction_worthy("fileWatcher")); // "watcher" not in STOP
        // All-common ids never index.
        assert!(!is_correction_worthy("getUserConfig"));
        assert!(!is_correction_worthy("handle_download"));
        assert!(!is_correction_worthy("build_index"));
        assert!(!is_correction_worthy("v2")); // [v] is 1 char < MIN_PART
    }

    #[test]
    fn harvest_indexes_distinctive_symbols_only() {
        let root = std::env::temp_dir().join(format!("dictum-symvocab-{}", std::process::id()));
        let make = |p: &str, body: &str| {
            let f = root.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, body).unwrap();
        };
        make("a.rs", "pub fn parse_zipformer() {}\nstruct DataProcessor;\nfn handle_download() {}\n");
        make("b.ts", "export class HudState {}\nexport function getUserConfig() {}\n");
        make("c.py", "def build_index(): pass\n");
        make("d.png", "fn not_source_here() {}\n"); // non-source ext skipped
        make("clash.rs", "fn parse_zip() {}\n");
        make("clash2.ts", "export function parseZip() {}\n");
        // Oversized source blob (> MAX_FILE_BYTES) must be skipped.
        make("huge.rs", &format!("fn oversized_symbolz() {{}}\n{}", "x".repeat(600 * 1024)));

        let i = Index::build(&[root.to_string_lossy().into_owned()]);
        let bs = &i.roots[0].by_symbol;
        let keys: Vec<&String> = bs.keys().collect();
        assert!(bs.contains_key("parsezipformer"), "{keys:?}");
        assert!(bs.contains_key("dataprocessor"), "{keys:?}");
        assert!(bs.contains_key("hudstate"), "{keys:?}");
        // All-common ids never index.
        assert!(!bs.contains_key("handledownload"), "{keys:?}");
        assert!(!bs.contains_key("getuserconfig"), "{keys:?}");
        assert!(!bs.contains_key("buildindex"), "{keys:?}");
        // Non-source and oversized files skipped.
        assert!(!bs.contains_key("notsourcehere"), "{keys:?}");
        assert!(!bs.contains_key("oversizedsymbolz"), "{keys:?}");
        // Two distinct spellings under one key => lookup returns nothing.
        assert_eq!(bs.get("parsezip").map(|v| v.len()), Some(2), "{keys:?}");
        assert_eq!(lookup_symbol(&i, &[0], "parsezip"), None);

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn hotwords_are_single_word_distinctive_symbols_only() {
        let i = idx_syms(
            "dictum",
            &["src/lib.rs"],
            &["zipformer", "HudState", "try_finalize", "buf", "config", "Segmenter"],
        );
        let mut hw = i.hotwords();
        hw.sort();
        // zipformer/segmenter are spoken as one word and are misspellable.
        // HudState and try_finalize are spoken as their parts — text-side work.
        // "buf" is under MIN_HOTWORD; "config" is a common word.
        assert_eq!(hw, vec!["Segmenter".to_string(), "zipformer".to_string()]);
    }

    #[test]
    fn ambiguous_symbol_spellings_are_never_biased() {
        // Two spellings under one normalized key: the text pass returns nothing
        // rather than guess, and biasing must not guess either.
        let i = idx_syms("dictum", &["src/lib.rs"], &["zipformer", "ZipFormer"]);
        assert!(i.hotwords().is_empty());
    }

    #[test]
    fn apply_symbols_fires_on_cue() {
        let i = idx_syms("dictum", &[], &["parse_zipformer", "HudState", "zipformer"]);
        assert_eq!(
            apply_symbols("call symbol parse zipformer now", &i, TITLE, "symbol"),
            "call parse_zipformer now"
        );
        assert_eq!(
            apply_symbols("update the symbol hud state here", &i, TITLE, "symbol"),
            "update the HudState here"
        );
        // n=1: a single distinctive token is allowed (the highest-value ASR-miss).
        assert_eq!(apply_symbols("the symbol zipformer buffer", &i, TITLE, "symbol"), "the zipformer buffer");
    }

    #[test]
    fn apply_symbols_absorbs_cue_and_keeps_punctuation() {
        let i = idx_syms("dictum", &[], &["zipformer"]);
        // Cue + its trailing space consumed; single spacing preserved.
        assert_eq!(apply_symbols("the symbol zipformer here", &i, TITLE, "symbol"), "the zipformer here");
        // Leading punctuation on the target survives (bytes never dropped).
        assert_eq!(apply_symbols("call symbol (zipformer) now", &i, TITLE, "symbol"), "call (zipformer) now");
        // Trailing punctuation on the target survives.
        assert_eq!(apply_symbols("the symbol zipformer.", &i, TITLE, "symbol"), "the zipformer.");
        // A detached "." between cue and target blocks firing (gap_after).
        assert_eq!(apply_symbols("the symbol . zipformer", &i, TITLE, "symbol"), "the symbol . zipformer");
        // A cue with trailing punctuation ("symbol,") is not the bare cue.
        assert_eq!(apply_symbols("the symbol, zipformer", &i, TITLE, "symbol"), "the symbol, zipformer");
    }

    #[test]
    fn cardinal_sin_cueless_is_identity() {
        // Dictum-shaped index INCLUDING DataProcessor/fileWatcher — the ones the
        // STOP filter alone cannot save. With no cue token, every take is a pure
        // passthrough. This is the proof the #1 corruption residual is closed.
        let i = idx_syms(
            "dictum",
            &[],
            &["DataProcessor", "fileWatcher", "handle_download", "build_index", "parse_zipformer", "HudState"],
        );
        for s in [
            "the build is failing",
            "the data processor was slow",
            "the file watcher died",
            "please handle the download queue",
            "get the user config ready",
            "coordinate the message flow",
            "apply the replacements now",
            "let's build the index by hand",
        ] {
            assert_eq!(apply_symbols(s, &i, TITLE, "symbol"), s, "cue-less must be identity: {s}");
        }
    }

    #[test]
    fn cued_prose_distinctive_backstop() {
        let i = idx_syms("dictum", &[], &["parse_zipformer", "HudState", "zipformer", "DataProcessor"]);
        // Cue present in ordinary prose, but the following words are all-common
        // (or don't resolve) => the distinctive/index gate no-ops the rewrite.
        for s in [
            "the symbol table is corrupt",
            "resolve the symbol name first",
            "a symbol for the build step",
            "the symbol of our team",
        ] {
            assert_eq!(apply_symbols(s, &i, TITLE, "symbol"), s, "cued-prose must be identity: {s}");
        }
        // The accepted boundary the design deliberately allows: a cue immediately
        // before a distinctive repo symbol in genuine prose DOES fire.
        assert_eq!(apply_symbols("the symbol zipformer is fast", &i, TITLE, "symbol"), "the zipformer is fast");
    }

    #[test]
    fn apply_symbols_gates() {
        let i = idx_syms("dictum", &[], &["zipformer", "parse_zip", "parseZip"]);
        // No title / non-matching title => inert.
        assert_eq!(apply_symbols("the symbol zipformer here", &i, None, "symbol"), "the symbol zipformer here");
        assert_eq!(
            apply_symbols("the symbol zipformer here", &i, Some("Inbox — Gmail"), "symbol"),
            "the symbol zipformer here"
        );
        // Ambiguous key (parse_zip + parseZip) => no-op.
        assert_eq!(apply_symbols("the symbol parse zip now", &i, TITLE, "symbol"), "the symbol parse zip now");
        // Empty/blank cue => passthrough.
        assert_eq!(apply_symbols("the symbol zipformer here", &i, TITLE, ""), "the symbol zipformer here");
        assert_eq!(apply_symbols("the symbol zipformer here", &i, TITLE, "   "), "the symbol zipformer here");
        // Empty input.
        assert_eq!(apply_symbols("", &i, TITLE, "symbol"), "");
        // A filetag @path token after the cue is skipped (contains '/').
        assert_eq!(
            apply_symbols("the symbol @src/zipformer.rs here", &i, TITLE, "symbol"),
            "the symbol @src/zipformer.rs here"
        );
    }

    #[test]
    fn filetag_then_symbols_no_crosstalk() {
        // filetag tags the file; apply_symbols corrects the identifier; the
        // emitted @path token is skipped by is_nameish — no competition.
        let i = idx_syms("dictum", &["src-tauri/src/coordinator.rs"], &["parse_zipformer"]);
        let after_tag = apply("open coordinator dot rs then call symbol parse zipformer", &i, TITLE);
        assert_eq!(after_tag, "open @src-tauri/src/coordinator.rs then call symbol parse zipformer");
        assert_eq!(
            apply_symbols(&after_tag, &i, TITLE, "symbol"),
            "open @src-tauri/src/coordinator.rs then call parse_zipformer"
        );
    }

    #[test]
    fn shotgun_real_index_cueless_and_cued_prose_safe() {
        // Build an Index from the repo's own source and run a general-English +
        // dev-chat corpus through the cue: the cue-less path is identity by
        // construction, and common-word cued prose stays prose.
        let root = format!("{}/src", env!("CARGO_MANIFEST_DIR"));
        let i = Index::build(&[root]);
        let total: usize = i.roots.iter().map(|r| r.by_symbol.len()).sum();
        assert!(total > 0, "expected to harvest distinctive symbols from src-tauri/src");
        let title = Some("filetag.rs — src — Visual Studio Code");
        for s in [
            "the build is failing and I need to handle download timeouts",
            "let's refactor the coordinator and clean up the audio pipeline",
            "the file watcher keeps missing changes on save",
            "we should process the config before the first render",
            "the data processor was slow on the last run",
            "please apply the replacements and then inject the text",
            "the model status is loading and the overlay is hidden",
            "run the tests and check the results in the terminal",
            "reset the session and warm the model again",
            "the level bar animation looks smooth on this machine",
        ] {
            assert_eq!(apply_symbols(s, &i, title, "symbol"), s, "cue-less must be identity: {s}");
        }
        // Cue present but followed by ordinary common words => still prose.
        for s in [
            "the symbol table is corrupt",
            "resolve the symbol name first",
            "check the symbol count in the report",
        ] {
            assert_eq!(apply_symbols(s, &i, title, "symbol"), s, "cued common-word prose must be identity: {s}");
        }
    }

    // Cue-grammar abuse matrix: the cue must arm a rewrite only in the exact
    // bare-adjacent shape, and never drop bytes it didn't intend to absorb.
    #[test]
    fn apply_symbols_cue_grammar_is_byte_safe() {
        let i = idx_syms(
            "dictum",
            &[],
            &["zipformer", "parse_zipformer", "HudState", "parse_zip", "parseZip"],
        );
        // Fires: cue as the first word (nothing before it to preserve).
        assert_eq!(apply_symbols("symbol zipformer is fast", &i, TITLE, "symbol"), "zipformer is fast");
        // No-op: cue as the last word — no run to arm.
        assert_eq!(apply_symbols("call the symbol", &i, TITLE, "symbol"), "call the symbol");
        // Cue twice: the first cue can't resolve itself/next-cue, so it stays
        // literal prose; the SECOND cue arms the real symbol.
        assert_eq!(apply_symbols("symbol symbol zipformer", &i, TITLE, "symbol"), "symbol zipformer");
        // Detached punctuation between cue and target (gap_after) blocks firing.
        assert_eq!(apply_symbols("call symbol . zipformer", &i, TITLE, "symbol"), "call symbol . zipformer");
        // Cue then an all-STOP run: no distinctive word, no fire.
        assert_eq!(apply_symbols("the symbol state handler config", &i, TITLE, "symbol"), "the symbol state handler config");
        // Cue then a term that resolves to TWO spellings: unique-or-nothing no-op.
        assert_eq!(apply_symbols("the symbol parse zip now", &i, TITLE, "symbol"), "the symbol parse zip now");
        // Cue glued to a leading quote is not the bare cue — whole take is prose.
        assert_eq!(
            apply_symbols("he said \"symbol zipformer\" today", &i, TITLE, "symbol"),
            "he said \"symbol zipformer\" today"
        );
        // Punctuated cue ("symbol,") is not the bare cue.
        assert_eq!(apply_symbols("the symbol, zipformer here", &i, TITLE, "symbol"), "the symbol, zipformer here");
        // A sentence boundary on the cue token blocks it (cue is not bare).
        assert_eq!(
            apply_symbols("look at the symbol. Zipformer runs", &i, TITLE, "symbol"),
            "look at the symbol. Zipformer runs"
        );
        // A filetag-emitted @path directly after the cue is non-nameish -> skipped.
        assert_eq!(
            apply_symbols("the symbol @src/zipformer.rs here", &i, TITLE, "symbol"),
            "the symbol @src/zipformer.rs here"
        );
        // Cue normalized case-insensitively: a capitalized cue at a sentence start
        // still fires (mirrors filetag's case-insensitive "at"). Documented boundary.
        assert_eq!(apply_symbols("Symbol zipformer here", &i, TITLE, "symbol"), "zipformer here");
    }

    // Item 4: the emitted spelling is the repo's exact casing, byte-for-byte,
    // for every identifier convention and for multi-token composition.
    #[test]
    fn symbol_casing_emitted_verbatim() {
        let i = idx_syms(
            "dictum",
            &[],
            &["zipformer", "parse_zipformer", "HudState", "MAX_BUFFER", "NEMOTRON", "reformat_id_for_gpu"],
        );
        // snake_case, single and multi-token composition.
        assert_eq!(apply_symbols("call symbol zipformer now", &i, TITLE, "symbol"), "call zipformer now");
        assert_eq!(apply_symbols("call symbol parse zipformer now", &i, TITLE, "symbol"), "call parse_zipformer now");
        assert_eq!(
            apply_symbols("call symbol reformat id for gpu now", &i, TITLE, "symbol"),
            "call reformat_id_for_gpu now"
        );
        // CamelCase composed from spoken parts.
        assert_eq!(apply_symbols("the symbol hud state here", &i, TITLE, "symbol"), "the HudState here");
        // SCREAMING_SNAKE, single and multi-token — casing is not lowered.
        assert_eq!(apply_symbols("call symbol nemotron now", &i, TITLE, "symbol"), "call NEMOTRON now");
        assert_eq!(apply_symbols("the symbol max buffer here", &i, TITLE, "symbol"), "the MAX_BUFFER here");
    }

    // Cardinal-sin at scale, with the DANGEROUS vocabulary: a real harvested
    // index of THIS repo, and a cue-less corpus dense with the repo's own
    // not-in-STOP identifiers (coordinator, reformat, inject, decode, ensure,
    // probe, persist, dispatch, guardrail...). No cue token => byte-identity is
    // the invariant. A single diff would be a critical prose-corruption bug.
    #[test]
    fn cardinal_sin_cueless_repo_vocab_is_identity() {
        // Canonicalize so the root has a real folder name; a path ending in ".."
        // has no file_name and would go inert — passing identity for the wrong
        // reason instead of exercising the live matching loop.
        let root_path = std::fs::canonicalize(format!("{}/..", env!("CARGO_MANIFEST_DIR"))).unwrap();
        // Take the folder name from the checkout rather than hardcoding "Dictum":
        // a worktree or a differently-named clone would otherwise make the root
        // inert and trip the liveness assert below for an environmental reason.
        let folder = root_path.file_name().unwrap().to_string_lossy().into_owned();
        let root = root_path.to_string_lossy().into_owned();
        let i = Index::build(&[root]);
        let total: usize = i.roots.iter().map(|r| r.by_symbol.len()).sum();
        assert!(total > 50, "expected a rich harvested index, got {total}");
        let title_owned = format!("coordinator.rs — {folder} — Visual Studio Code");
        let title = Some(title_owned.as_str());
        // Liveness: the root MUST be active and its symbol map non-empty, so a
        // cue WOULD fire here — identity below is the gate holding, not inertness.
        assert!(!active_roots(&i, title).is_empty(), "root must be active for this test to mean anything");
        for s in [
            "the coordinator should reset the session before the next decode",
            "we need to reformat the transcript and then inject the text",
            "the gpu probe gate is too conservative on this machine",
            "ensure the model is loaded before we dispatch the audio",
            "the hotkey manager keeps missing the escape rearm",
            "persist the history record even when the take is abandoned",
            "the guardrail tripped on a bad reformat so we fell back",
            "the coordinator dispatch queue is stuck awaiting the tail",
            "please probe the gpu and decide whether to offload the reformat",
            "the reformat engine postprocess strips the trailing marker",
            "inject the clipboard payload and then restore the text buffer",
            "the stream feed is off by default and warms on the first take",
            "reconstruct the spoken forms and check the identifier overlap",
            "the audio cue plays before we start the capture",
            "download the model and verify the directory hash",
            "the coordinator arms an idle unload after the session ends",
            "normalize the newlines before the reformat prompt is built",
            "the level bar envelope is downsampled to preserve the peaks",
            "spawn the resume detector so a suspend does not wedge the state",
            "the reformat device resolves to gpu only when the probe is sane",
            "handle the download timeout and retry the partial install",
            "the model spec picks the single hashed gguf for the reformat sku",
            "the coordinator cancels the timer and clears the pending audio",
            "ensure the reformat model is present before we announce ready",
            "the tray shows recording while the overlay stays hidden",
            "decode the tail and try to finalize the inject",
            "the history purge retention drops the rows outside the window",
            "check the guardrail ratio and the content word floor",
            "the hotkey rebind parses the chord and rejects garbage",
            "warm the stream model again after the idle unload fires",
            "the coordinator loading percent drives the overlay animation",
            "persist the config and save without leaving a stray temp file",
            "the reformat falls back to deterministic output when it trips",
            "probe is sane so the gpu gate opens for this session",
            "the inject path is clipboard only on the elevated window",
            "reset the session state and abort the current capture",
        ] {
            assert_eq!(apply_symbols(s, &i, title, "symbol"), s, "cue-less prose must be byte-identical: {s}");
        }
    }
}
