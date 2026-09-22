//! Built-in coding vocabulary: ONE table, two consumers.
//!
//! A term added here does both jobs at once — it biases the recogniser toward
//! the right spelling (ASR hotwords, `asr.rs`) and rewrites the mis-hearings
//! that biasing can't reach (replacement rules, `replacements.rs`). Curating
//! those two lists separately is how they drift apart, so they don't exist
//! separately.
//!
//! The split between the two columns carries real judgement:
//!
//! - `canonical` becomes a hotword. Biasing is acoustic and conservative — it
//!   nudges the decoder toward a spelling it was already considering, so it is
//!   safe for terms whose spoken form collides with ordinary English.
//! - `spoken` becomes a word-bounded rewrite rule, which fires on TEXT with no
//!   acoustic evidence at all. Every entry here must be a phrase that
//!   essentially never occurs in ordinary prose.
//!
//! That is why `tokio` and `kwargs` carry no `spoken` forms: a `tokyo`->`tokio`
//! or `quarks`->`kwargs` rule would corrupt a sentence about the city or the
//! particle. Biasing gets them from the audio instead, where the evidence is.

use crate::types::Replacement;

pub struct Term {
    /// What should end up in the text — and what the recogniser is biased toward.
    pub canonical: &'static str,
    /// Spoken mis-hearings a rule must rewrite. Empty when biasing plus the
    /// model's own spelling is enough, or when a rule would be unsafe in prose.
    pub spoken: &'static [&'static str],
}

const fn t(canonical: &'static str, spoken: &'static [&'static str]) -> Term {
    Term { canonical, spoken }
}

/// ponytail: ~120 terms, hand-picked for "the ear gets this wrong AND the fix is
/// unambiguous". The curated 250-300 with 3-5 variants each is PLAN-0.5 item 4 —
/// do that against the eval fixtures, not from a blank page.
pub const TERMS: &[Term] = &[
    // --- Rust ------------------------------------------------------------
    t("tokio", &[]),            // "Tokyo" is a city — biasing only, never a rule
    t("serde", &["sir dee", "ser day"]),
    t("clippy", &[]),
    t("rustfmt", &["rust format"]),
    t("cargo", &[]),
    t("crates.io", &["crates io"]),
    t("reqwest", &[]),
    t("axum", &[]),
    t("anyhow", &[]),
    t("thiserror", &[]),        // "this error is confusing" — biasing only
    t("rayon", &[]),
    t("tauri", &[]),
    t("wasm", &[]),
    t("async", &[]),
    t("await", &[]),
    t("enum", &[]),
    t("struct", &[]),
    t("impl", &[]),
    t("mutex", &[]),
    t("stdout", &["standard out", "standard output"]),
    t("stderr", &["standard error", "standard err"]),
    t("stdin", &["standard input"]),  // not "standard in" — ordinary English
    // --- Python ----------------------------------------------------------
    t("pytest", &["py test"]),
    t("kwargs", &[]),           // "quarks" is a particle — biasing only
    t("argparse", &["arg parse"]),
    t("numpy", &["num pie", "numb pie"]),
    t("scipy", &["sigh pie"]),
    t("pandas", &[]),
    t("matplotlib", &["mat plot lib"]),
    t("pydantic", &[]),
    t("asyncio", &["async io"]),
    t("virtualenv", &["virtual env"]),
    t("pyproject", &["py project"]),
    t("PyPI", &[]),
    t("pipx", &[]),
    t("uv", &[]),
    t("ruff", &[]),
    t("mypy", &[]),             // "my pie" — biasing only
    t("Django", &[]),
    t("FastAPI", &["fast api"]),
    t("Flask", &[]),
    // --- JS / TS ---------------------------------------------------------
    t("TypeScript", &["type script", "typescript"]),
    t("JavaScript", &["java script", "javascript"]),
    t("Node.js", &["node js"]),
    t("Next.js", &["next js"]),
    t("NestJS", &["nest js"]),
    t("React", &["react js"]),
    t("Vue", &[]),
    t("Svelte", &[]),
    t("npm", &[]),
    t("pnpm", &[]),
    t("nvm", &[]),
    t("eslint", &["es lint"]),
    t("prettier", &[]),
    t("vite", &[]),
    t("webpack", &["web pack"]),
    t("esbuild", &["es build"]),
    t("tsconfig", &["ts config"]),
    t("jsx", &[]),
    t("tsx", &[]),
    t("Deno", &[]),
    t("Bun", &[]),
    // --- Web / protocol --------------------------------------------------
    t("localhost", &["local host"]),
    t("WebSocket", &["web socket"]),
    t("GraphQL", &["graph ql", "graphql"]),
    t("OAuth", &["o auth", "oauth"]),
    t("JSON", &["json"]),
    t("YAML", &["yaml"]),
    t("TOML", &["toml"]),
    t("API", &["api"]),
    t("URL", &["url"]),
    t("URI", &["uri"]),
    t("HTML", &["html"]),
    t("CSS", &["css"]),
    t("HTTP", &["http"]),
    t("HTTPS", &["https"]),
    t("SQL", &["sql"]),
    t("CORS", &[]),
    t("JWT", &[]),              // "jot it down" — biasing only
    t("CRUD", &[]),
    t("REST", &[]),
    t("gRPC", &["g rpc"]),
    t("MIME", &[]),
    t("UUID", &[]),
    // --- Infra / tooling -------------------------------------------------
    t("nginx", &["engine x"]),
    t("kubectl", &["kube control", "cube control", "kube cuddle"]),
    t("Kubernetes", &["kubernetes"]),
    t("Docker", &[]),
    t("Dockerfile", &["docker file"]),
    t("Terraform", &["terra form"]),
    t("Ansible", &[]),
    t("systemd", &["system d"]),
    t("cron", &[]),
    t("crontab", &["cron tab"]),
    t("SSH", &[]),
    t("TLS", &[]),
    t("DNS", &[]),
    t("CIDR", &[]),
    t("Redis", &["redis"]),
    t("Postgres", &["post gres", "postgres"]),
    t("PostgreSQL", &["postgre sql"]),
    t("MongoDB", &["mongo db"]),
    t("SQLite", &["sqlite", "sequel lite"]),
    t("MySQL", &["my sql"]),
    t("ClickHouse", &["click house"]),
    t("Kafka", &[]),
    t("GraphViz", &["graph viz"]),
    // --- Git / platforms -------------------------------------------------
    t("GitHub", &["get hub", "git hub", "github"]),
    t("GitLab", &["get lab", "git lab", "gitlab"]),
    t("Bitbucket", &[]),        // "bit bucket" is an idiom — biasing only
    t("git", &[]),
    t("rebase", &["re base"]),
    t("cherry-pick", &["cherry pick"]),
    t("monorepo", &["mono repo"]),
    t("repo", &[]),
    t("CI/CD", &["ci cd"]),
    // --- Shell / OS ------------------------------------------------------
    t("SIGKILL", &["sig kill"]),
    t("SIGTERM", &["sig term"]),
    t("SIGINT", &["sig int"]),
    t("chmod", &["ch mod"]),
    t("chown", &["ch own"]),
    t("grep", &[]),
    t("regex", &["reg ex"]),
    t("stdlib", &["standard lib"]),
    t("PowerShell", &["power shell"]),
    t("WSL", &[]),
    t("PATH", &[]),
    // --- Languages / misc ------------------------------------------------
    t("C++", &["c plus plus"]),
    t("C#", &["c sharp"]),
    t(".NET", &["dot net"]),
    t("Go", &["golang"]),
    t("Kotlin", &[]),
    t("Swift", &[]),
    t("LLVM", &[]),
    t("CMake", &["c make"]),    // not "see make" — ordinary English
    t("ONNX", &[]),             // "onyx" is a stone — biasing only
    t("CUDA", &["cuda"]),
    t("Vulkan", &["vulkan"]),
    t("GGUF", &[]),
    t("LLM", &[]),
];

/// Built-in terms as replacement rules, compound-first.
///
/// Rule order is load-bearing — `apply` runs them in sequence, so a single-word
/// rule that fires first can eat a compound's tail ("net" before "dot net").
/// Sorting by word count then length descending is the same ordering discipline
/// `packs.ts` documents for the user-facing presets.
/// ponytail: built once. The table is static, so the sort and the allocation have
/// no business running per utterance — unlike the user's rules, which change.
pub fn rules() -> &'static [Replacement] {
    static RULES: std::sync::OnceLock<Vec<Replacement>> = std::sync::OnceLock::new();
    RULES.get_or_init(|| {
        let mut out: Vec<Replacement> = TERMS
            .iter()
            .flat_map(|t| {
                t.spoken.iter().map(|h| Replacement {
                    heard: (*h).to_string(),
                    printed: t.canonical.to_string(),
                })
            })
            .collect();
        out.sort_by(|a, b| {
            let wa = a.heard.split_whitespace().count();
            let wb = b.heard.split_whitespace().count();
            wb.cmp(&wa).then(b.heard.len().cmp(&a.heard.len()))
        });
        out
    })
}

/// Canonical spellings worth biasing the recogniser toward.
///
/// Skips anything the BPE context graph can't usefully encode from speech:
/// entries under 3 chars (a 2-char hotword boosts far too much — "Go", "uv"),
/// and entries carrying punctuation the model would never emit as one token
/// (`C++`, `.NET`, `Node.js`, `CI/CD`).
pub fn hotwords() -> Vec<String> {
    TERMS
        .iter()
        .map(|t| t.canonical)
        .filter(|c| c.chars().count() >= 3 && c.chars().all(|ch| ch.is_ascii_alphanumeric()))
        .map(|c| c.to_string())
        .collect()
}

/// Entries sherpa's context graph can accept, joined into its wire format.
///
/// Separator is `/` because the C++ side does
/// `regex_replace(hotwords, std::regex("/"), "\n")` — which means a `/` INSIDE an
/// entry silently becomes two hotwords (`@anthropic-ai/sdk` -> `@anthropic-ai`
/// and `sdk`). Strip them rather than split them.
///
/// Dedupes case-insensitively, since the same term arrives from the built-in
/// table, the user's vocabulary and the repo symbol harvest.
pub fn join_hotwords(entries: impl IntoIterator<Item = String>) -> String {
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for e in entries {
        // NUL too: the crate hands this straight to CString::new().unwrap(), so
        // one interior NUL from a hand-edited config would panic the ASR thread.
        let e = e.replace(['/', ' '], " ");
        let e = e.trim();
        // Too short to bias safely, or nothing for the BPE encoder to hold onto.
        if e.chars().count() < 3 || !e.chars().any(|c| c.is_alphabetic()) {
            continue;
        }
        if seen.insert(e.to_lowercase()) {
            out.push(e.to_string());
        }
        if out.len() >= MAX_HOTWORDS {
            eprintln!("asr: hotword list capped at {MAX_HOTWORDS} entries — later terms dropped");
            break;
        }
    }
    out.join("/")
}

/// ponytail: no measured ceiling exists for the context graph's size. This is a
/// runaway guard, not a tuned value — raise it once the eval fixtures can show
/// what large lists cost in decode time and false boosts.
const MAX_HOTWORDS: usize = 2000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_terms_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for t in TERMS {
            assert!(seen.insert(t.canonical.to_lowercase()), "duplicate term: {}", t.canonical);
        }
    }

    /// A spoken form that equals its own canonical spelling is a casing fix and is
    /// fine ("json" -> "JSON"). One that matches a DIFFERENT term's canonical form
    /// is a rule fighting another rule.
    #[test]
    fn spoken_forms_never_collide_with_another_term() {
        let canon: std::collections::HashMap<String, &str> =
            TERMS.iter().map(|t| (t.canonical.to_lowercase(), t.canonical)).collect();
        for t in TERMS {
            for s in t.spoken {
                if let Some(other) = canon.get(&s.to_lowercase()) {
                    assert_eq!(
                        *other, t.canonical,
                        "spoken form {s:?} of {} collides with term {other}",
                        t.canonical
                    );
                }
            }
        }
    }

    #[test]
    fn rules_are_compound_first() {
        let r = rules();
        let words: Vec<usize> = r.iter().map(|x| x.heard.split_whitespace().count()).collect();
        assert!(
            words.windows(2).all(|w| w[0] >= w[1]),
            "multi-word rules must precede single-word ones or they eat each other's tails"
        );
    }

    /// The prose-safety bar: a rule fires on text with no acoustic evidence, so
    /// no single-word spoken form may be an ordinary English word.
    #[test]
    fn single_word_rules_are_not_english_prose() {
        // Words that would corrupt ordinary dictation if rewritten.
        const PROSE: &[&str] = &[
            "tokyo", "quarks", "go", "rust", "react", "swift", "class", "type", "state", "main",
            "file", "build", "test", "run", "check", "make", "design", "menu", "cargo", "bun",
        ];
        for r in rules() {
            if r.heard.split_whitespace().count() == 1 {
                assert!(
                    !PROSE.contains(&r.heard.as_str()),
                    "{:?} is ordinary prose — bias it acoustically instead (leave `spoken` empty)",
                    r.heard
                );
            }
        }
    }

    /// The real prose-safety bar. A single-word check cannot see a collision like
    /// "this error" -> thiserror or "my pie" -> mypy, and every one of those was
    /// present in the first draft of this table.
    #[test]
    fn ordinary_prose_survives_every_rule() {
        const PROSE: &[&str] = &[
            "this error is confusing and i cannot reproduce it",
            "i ate my pie and went back to work",
            "jot it down before you forget",
            "the ring had an onyx set into it",
            "throw it in the bit bucket",
            "that is the standard in this industry",
            "did you see make fail again",
            "i flew to tokyo last spring",
            "the paper was about quarks and gluons",
            "go and check the repo for me",
            "the engine ran hot and the pipe leaked",
            "we need a faster api response",
        ];
        for line in PROSE {
            let mut out = (*line).to_string();
            for r in rules() {
                out = crate::replacements::apply_rule_for_test(&out, r);
            }
            // "a faster api response" is the one deliberate exception: `api` ->
            // `API` is a casing fix and is meant to fire in prose.
            let want = line.replace(" api ", " API ");
            assert_eq!(out, want, "a built-in rule corrupted prose: {line:?}");
        }
    }

    #[test]
    fn hotwords_are_encodable() {
        for h in hotwords() {
            assert!(h.chars().count() >= 3, "{h} too short to bias safely");
            assert!(h.chars().all(|c| c.is_ascii_alphanumeric()), "{h} carries unencodable punctuation");
        }
        // The punctuation-carrying terms are excluded, not silently mangled.
        let hw = hotwords();
        for skipped in ["C++", "C#", ".NET", "Node.js", "CI/CD", "Go", "uv"] {
            assert!(!hw.iter().any(|h| h == skipped), "{skipped} should not be a hotword");
        }
        assert!(hw.iter().any(|h| h == "tokio"), "tokio is the whole point");
        assert!(hw.iter().any(|h| h == "kwargs"));
    }

    /// The C++ splits the wire string on '/', so an entry containing one would
    /// silently become two hotwords.
    #[test]
    fn slashes_never_reach_the_wire_format() {
        let joined = join_hotwords(vec!["@anthropic-ai/sdk".into(), "src/main.rs".into()]);
        assert!(!joined.contains("/sdk"), "a '/' inside an entry split it: {joined}");
        assert_eq!(joined, "@anthropic-ai sdk/src main.rs");
    }

    #[test]
    fn interior_nul_never_reaches_the_ffi() {
        let joined = join_hotwords(vec!["tok io".into()]);
        assert!(!joined.contains(' '));
        assert_eq!(joined, "tok io");
    }

    #[test]
    fn join_dedupes_case_insensitively_and_drops_unusable() {
        let joined =
            join_hotwords(vec!["tokio".into(), "Tokio".into(), "uv".into(), "42".into(), "  ".into()]);
        assert_eq!(joined, "tokio");
    }

    #[test]
    fn built_in_rules_cover_the_shipped_preset() {
        // The TS CODING_TERMS pack's whole job moves here; these are the entries
        // whose loss a user would actually notice.
        let r = rules();
        let has = |h: &str, p: &str| r.iter().any(|x| x.heard == h && x.printed == p);
        assert!(has("engine x", "nginx"));
        assert!(has("kube control", "kubectl"));
        assert!(has("get hub", "GitHub"));
        assert!(has("dot net", ".NET"));
        assert!(has("c plus plus", "C++"));
        assert!(has("json", "JSON"));
        assert!(has("golang", "Go"));
    }
}
