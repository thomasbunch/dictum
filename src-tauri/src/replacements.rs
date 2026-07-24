//! Deterministic post-ASR text transforms: filler-word removal + replacement
//! rules. Runs between ASR and inject (CONTRACTS.md). No LLM.

use crate::types::{Config, Replacement};
use regex::{NoExpand, Regex};

pub fn apply(raw: &str, cfg: &Config) -> String {
    apply_with_cursor(raw, cfg).0
}

/// Sentinel inside a replacement value marking where the caret should land.
const CURSOR: &str = "{cursor}";

/// Full transform plus caret hint. Snippets are just replacement rules with
/// bigger values (multi-line, or bearing a `{cursor}` sentinel) — the pipeline
/// is unchanged: the sentinel rides through `apply_rule` as a literal (NoExpand)
/// and is stripped here. Returns the injected text with every `{cursor}` removed
/// and, if any was present, how many chars from the END of that text the caret
/// should sit (LAST sentinel wins; None when there was none). Injection consumes
/// the offset later; this stays pure.
pub fn apply_with_cursor(raw: &str, cfg: &Config) -> (String, Option<usize>) {
    let mut text = raw.to_string();
    if cfg.remove_fillers {
        text = remove_fillers(&text);
    }
    // Canonical casing runs BEFORE the replacements loop: it re-cases only what
    // the ear heard, never a rule's/snippet's literal output (a vocab term
    // "GitHub" must not rewrite a rule-emitted "github.com" -> "GitHub.com").
    // Sentinel-safe by construction — {cursor} only enters via rule values in
    // the loop below, so none exists in the text yet.
    text = canonicalize_vocab(&text, &cfg.vocabulary);
    for rule in &cfg.replacements {
        text = apply_rule(&text, rule);
    }
    let offset = cursor_back_offset(&text);
    (text.replace(CURSOR, ""), offset)
}

/// Canonical casing for vocabulary terms: each term is matched case-insensitively
/// on word boundaries and rewritten to the term exactly as typed — "github" ->
/// "GitHub". Casing only: a case-insensitive match spans the same characters, so
/// this never changes spelling or length. Treats each term as a self-canonical
/// rule (heard == printed) and reuses apply_rule's word-boundary + symbol logic.
/// ponytail: recompiles a regex per term per utterance, exactly like the
/// replacements loop already does; add a lazy cache only if profiling says so.
/// ponytail: assumes simple 1:1 case folding (length-preserving, keeps the caret
/// offset valid); vocab is proper nouns, so exotic full-fold pairs (ß↔SS,
/// Turkish i) are out of scope.
pub fn canonicalize_vocab(text: &str, vocab: &[String]) -> String {
    let mut out = text.to_string();
    for term in vocab {
        if term.trim().is_empty() {
            continue;
        }
        out = apply_rule(&out, &Replacement { heard: term.clone(), printed: term.clone() });
    }
    out
}

/// Chars between the LAST `{cursor}` sentinel and the end of the final text, or
/// None if none is present. Pass the expanded (pre-strip) text. Nothing after
/// the last sentinel can be another sentinel, so that tail's char count is
/// exactly the caret's distance from the end once every sentinel is stripped.
pub fn cursor_back_offset(expanded: &str) -> Option<usize> {
    let last = expanded.rfind(CURSOR)?;
    Some(expanded[last + CURSOR.len()..].chars().count())
}

fn remove_fillers(text: &str) -> String {
    // Standalone um/uh/uhm/erm, case-insensitive, swallowing a trailing comma.
    let filler = Regex::new(r"(?i)\b(?:um|uh|uhm|erm)\b,?").unwrap();
    let mut s = filler.replace_all(text, "").into_owned();
    s = Regex::new(r" {2,}").unwrap().replace_all(&s, " ").into_owned();
    s = s.replace(" ,", ",").replace(" .", ".");
    s.trim().to_string()
}

fn apply_rule(text: &str, rule: &Replacement) -> String {
    if rule.heard.is_empty() {
        return text.to_string();
    }
    // Only assert a word boundary at an end that is itself a word char. A `\b`
    // after a symbol (e.g. "c++", "c#", ".net") can never match — the boundary
    // needs a word char that isn't there — so those rules would silently no-op.
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let lead = if rule.heard.starts_with(is_word) { r"\b" } else { "" };
    let trail = if rule.heard.ends_with(is_word) { r"\b" } else { "" };
    let pattern = format!("(?i){lead}{}{trail}", regex::escape(&rule.heard));
    let re = Regex::new(&pattern).expect("escaped pattern is always valid");
    // NoExpand: printed is a literal, not a $-group template.
    re.replace_all(text, NoExpand(&rule.printed)).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with(replacements: Vec<Replacement>, remove_fillers: bool) -> Config {
        let mut cfg = Config::default();
        cfg.replacements = replacements;
        cfg.remove_fillers = remove_fillers;
        cfg
    }

    fn rule(heard: &str, printed: &str) -> Replacement {
        Replacement { heard: heard.into(), printed: printed.into() }
    }

    #[test]
    fn word_boundary_no_partial_hit() {
        let cfg = cfg_with(vec![rule("cat", "dog")], false);
        assert_eq!(apply("concatenate the cat", &cfg), "concatenate the dog");
    }

    #[test]
    fn case_insensitive_match() {
        let cfg = cfg_with(vec![rule("smash", "SMASH(tm)")], false);
        assert_eq!(apply("Smash and SMASH and smash", &cfg), "SMASH(tm) and SMASH(tm) and SMASH(tm)");
    }

    #[test]
    fn multi_word_phrase() {
        let cfg = cfg_with(vec![rule("point break", "Point Break")], false);
        assert_eq!(apply("i love point break so much", &cfg), "i love Point Break so much");
    }

    #[test]
    fn non_word_char_heard_matches() {
        // "c++" ends in a non-word char: a trailing \b can never match there, so
        // the boundary must be dropped on that end (else this silently no-ops).
        let cfg = cfg_with(vec![rule("c++", "cpp")], false);
        assert_eq!(apply("i love c++ here", &cfg), "i love cpp here");
        // Leading boundary still protects the word-char end: no partial hit.
        let cfg2 = cfg_with(vec![rule(".net", "dotnet")], false);
        assert_eq!(apply("use .net today", &cfg2), "use dotnet today");
    }

    #[test]
    fn unicode_word_boundary() {
        // Non-ASCII word chars still get boundary assertions on both ends.
        let cfg = cfg_with(vec![rule("naïve", "naive")], false);
        assert_eq!(apply("a naïve idea", &cfg), "a naive idea");
    }

    #[test]
    fn dollar_sign_in_printed_is_literal() {
        let cfg = cfg_with(vec![rule("five bucks", "$5")], false);
        assert_eq!(apply("that costs five bucks", &cfg), "that costs $5");
    }

    #[test]
    fn deterministic_rule_order_cascades() {
        let cfg = cfg_with(vec![rule("a", "b"), rule("b", "c")], false);
        assert_eq!(apply("a", &cfg), "c");
    }

    #[test]
    fn filler_cleanup_punctuation() {
        let cfg = cfg_with(vec![], true);
        assert_eq!(apply("well, um, I think uh that works", &cfg), "well, I think that works");
        assert_eq!(apply("Um, so it begins.", &cfg), "so it begins.");
    }

    #[test]
    fn filler_no_partial_hit() {
        let cfg = cfg_with(vec![], true);
        assert_eq!(apply("the alumni erm gathered", &cfg), "the alumni gathered");
    }

    // --- item 1: vocabulary canonical casing ---------------------------

    fn cfg_vocab(vocab: Vec<&str>, replacements: Vec<Replacement>) -> Config {
        let mut cfg = cfg_with(replacements, false);
        cfg.vocabulary = vocab.into_iter().map(String::from).collect();
        cfg
    }

    #[test]
    fn vocab_canonicalizes_casing() {
        let cfg = cfg_vocab(vec!["GitHub"], vec![]);
        assert_eq!(apply("push to github today", &cfg), "push to GitHub today");
    }

    #[test]
    fn vocab_word_boundary() {
        let cfg = cfg_vocab(vec!["Go"], vec![]);
        // no hit inside "good" or "golang".
        assert_eq!(apply("go is good and golang rocks", &cfg), "Go is good and golang rocks");
    }

    #[test]
    fn vocab_multiword() {
        let cfg = cfg_vocab(vec!["Visual Studio"], vec![]);
        assert_eq!(apply("open visual studio now", &cfg), "open Visual Studio now");
    }

    #[test]
    fn vocab_symbol_term() {
        let cfg = cfg_vocab(vec!["C++"], vec![]);
        assert_eq!(apply("i code in c++ daily", &cfg), "i code in C++ daily");
    }

    #[test]
    fn vocab_runs_before_replacements() {
        // R1: vocab re-cases only what the ear heard, never a rule's literal
        // output. A rule emitting a URL must survive verbatim even though a
        // vocab term would otherwise re-case it (the URL-corruption hazard).
        let cfg = cfg_vocab(vec!["GitHub"], vec![rule("my repo", "github.com/me")]);
        assert_eq!(apply("push my repo", &cfg), "push github.com/me");
    }

    #[test]
    fn vocab_empty_noop() {
        let cfg = cfg_vocab(vec![], vec![]);
        assert_eq!(apply("nothing changes here", &cfg), "nothing changes here");
    }

    #[test]
    fn vocab_whitespace_entry_skipped() {
        let cfg = cfg_vocab(vec!["   "], vec![]);
        assert_eq!(apply("leave it alone", &cfg), "leave it alone");
    }

    #[test]
    fn vocab_preserves_cursor_sentinel() {
        // A vocab term "Cursor" re-cases the heard word, and the rule's {cursor}
        // sentinel (added AFTER vocab) is untouched — length-preserved offset.
        let cfg = cfg_vocab(vec!["Cursor"], vec![rule("hi", "hi{cursor}")]);
        let (text, off) = apply_with_cursor("hi in cursor", &cfg);
        assert_eq!(text, "hi in Cursor");
        assert_eq!(off, Some(10)); // " in Cursor" tail
    }

    // --- item 3: preset-pack curation safety ---------------------------

    #[test]
    fn pack_entries_are_boundary_safe() {
        // Sampled from packs.ts CODE_SYMBOLS / CODING_TERMS: prove the risky
        // curated entries fire (or don't) exactly where intended.
        let terms = cfg_with(vec![rule("engine x", "nginx")], false);
        assert_eq!(apply("restart engine x now", &terms), "restart nginx now");
        assert_eq!(apply("the engine ran hot", &terms), "the engine ran hot");

        let golang = cfg_with(vec![rule("golang", "Go")], false);
        assert_eq!(apply("golang rocks", &golang), "Go rocks");

        // http must NOT corrupt https (word boundary at the 's').
        let http = cfg_with(vec![rule("http", "HTTP"), rule("https", "HTTPS")], false);
        assert_eq!(apply("http and https", &http), "HTTP and HTTPS");

        // compound-first: "double colon" before "colon" yields "::" intact.
        let colon = cfg_with(vec![rule("double colon", "::"), rule("colon", ":")], false);
        assert_eq!(apply("path double colon method", &colon), "path :: method");

        // "pipe symbol" (two-word form) leaves prose "pipe" untouched.
        let pipe = cfg_with(vec![rule("pipe symbol", "|")], false);
        assert_eq!(apply("pipe the output", &pipe), "pipe the output");
    }

    // --- snippets: multi-line + {cursor} -------------------------------

    #[test]
    fn multiline_value_survives_intact() {
        let sig = "Best regards,\nThomas\nDictum Inc.";
        let cfg = cfg_with(vec![rule("sig block", sig)], false);
        assert_eq!(apply("please add sig block here", &cfg), format!("please add {sig} here"));
    }

    #[test]
    fn cursor_at_end_offset_zero() {
        let cfg = cfg_with(vec![rule("my email", "me@example.com{cursor}")], false);
        let (text, off) = apply_with_cursor("email me at my email", &cfg);
        assert_eq!(text, "email me at me@example.com");
        assert_eq!(off, Some(0));
    }

    #[test]
    fn cursor_at_start_of_value() {
        let cfg = cfg_with(vec![rule("greeting", "{cursor}Dear Sir")], false);
        let (text, off) = apply_with_cursor("insert greeting", &cfg);
        assert_eq!(text, "insert Dear Sir");
        // caret before "Dear Sir" -> 8 chars from the end.
        assert_eq!(off, Some(8));
    }

    #[test]
    fn cursor_multiple_last_wins() {
        let cfg = cfg_with(vec![rule("tag", "<{cursor}b>{cursor}</b>")], false);
        let (text, off) = apply_with_cursor("tag", &cfg);
        assert_eq!(text, "<b></b>"); // both sentinels stripped
        assert_eq!(off, Some(4)); // after the LAST sentinel: "</b>"
    }

    #[test]
    fn cursor_mid_multiline_value() {
        let cfg = cfg_with(vec![rule("letter", "Dear {cursor},\n\nRegards")], false);
        let (text, off) = apply_with_cursor("letter", &cfg);
        assert_eq!(text, "Dear ,\n\nRegards");
        // tail ",\n\nRegards" = 10 chars.
        assert_eq!(off, Some(10));
    }

    #[test]
    fn no_cursor_returns_none() {
        let cfg = cfg_with(vec![rule("my email", "me@example.com")], false);
        let (text, off) = apply_with_cursor("my email", &cfg);
        assert_eq!(text, "me@example.com");
        assert_eq!(off, None);
    }

    #[test]
    fn cursor_survives_filler_stripping() {
        // Fillers are stripped from raw BEFORE expansion, so a {cursor} in the
        // value is untouched by remove_fillers.
        let cfg = cfg_with(vec![rule("my email", "me@ex.com{cursor}")], true);
        let (text, off) = apply_with_cursor("um send my email uh", &cfg);
        assert_eq!(text, "send me@ex.com");
        assert_eq!(off, Some(0));
    }

    #[test]
    fn adjacent_snippet_expansions() {
        let cfg = cfg_with(vec![rule("greeting", "Hello{cursor}"), rule("closing", "Bye{cursor}")], false);
        let (text, off) = apply_with_cursor("greeting closing", &cfg);
        assert_eq!(text, "Hello Bye");
        assert_eq!(off, Some(0)); // last sentinel (from "closing") sits at the end
    }

    // ===================================================================
    // ADVERSARIAL SUITE — attacks vocab canonicalization, the 67 preset
    // pack entries (mirrored from src/main/packs.ts and driven through the
    // real engine), and the multi-line/{cursor} machinery. These lock the
    // invariants a "break it" pass must prove.
    // ===================================================================

    /// Exact mirror of packs.ts CODE_SYMBOLS (29), in array order.
    fn code_symbols() -> Vec<Replacement> {
        [
            ("open brace", "{"), ("close brace", "}"),
            ("open bracket", "["), ("close bracket", "]"),
            ("open paren", "("), ("close paren", ")"),
            ("open angle bracket", "<"), ("close angle bracket", ">"),
            ("fat arrow", "=>"), ("thin arrow", "->"),
            ("triple backtick", "```"),
            ("double colon", "::"),
            ("double ampersand", "&&"),
            ("double pipe", "||"),
            ("backtick", "`"),
            ("colon", ":"), ("semicolon", ";"),
            ("pipe symbol", "|"), ("ampersand", "&"),
            ("underscore", "_"), ("backslash", "\\"),
            ("forward slash", "/"), ("dollar sign", "$"),
            ("hash symbol", "#"), ("at sign", "@"),
            ("percent sign", "%"), ("asterisk", "*"),
            ("tilde", "~"), ("caret", "^"),
        ].iter().map(|(h, p)| rule(h, p)).collect()
    }

    /// Exact mirror of packs.ts CODING_TERMS (38), in array order.
    fn coding_terms() -> Vec<Replacement> {
        [
            ("get hub", "GitHub"), ("git hub", "GitHub"),
            ("kube control", "kubectl"), ("py test", "pytest"),
            ("engine x", "nginx"), ("node js", "Node.js"),
            ("next js", "Next.js"), ("nest js", "NestJS"),
            ("type script", "TypeScript"), ("java script", "JavaScript"),
            ("react js", "React"), ("mongo db", "MongoDB"),
            ("web socket", "WebSocket"), ("local host", "localhost"),
            ("post gres", "Postgres"), ("c plus plus", "C++"),
            ("c sharp", "C#"), ("dot net", ".NET"),
            ("golang", "Go"),
            ("github", "GitHub"), ("gitlab", "GitLab"),
            ("typescript", "TypeScript"), ("javascript", "JavaScript"),
            ("json", "JSON"), ("yaml", "YAML"),
            ("graphql", "GraphQL"), ("oauth", "OAuth"),
            ("sqlite", "SQLite"), ("postgres", "Postgres"),
            ("kubernetes", "Kubernetes"), ("redis", "Redis"),
            ("api", "API"), ("url", "URL"),
            ("html", "HTML"), ("css", "CSS"),
            ("http", "HTTP"), ("https", "HTTPS"),
            ("sql", "SQL"),
        ].iter().map(|(h, p)| rule(h, p)).collect()
    }

    /// Both packs wired as the user gets them when clicking CODE SYMBOLS then
    /// CODING TERMS (addPack appends in array order). 67 rules, order preserved.
    fn full_pack() -> Vec<Replacement> {
        let mut v = code_symbols();
        v.extend(coding_terms());
        assert_eq!(v.len(), 67, "packs.ts drifted from this mirror");
        v
    }

    fn apply_pack(input: &str) -> String {
        apply(input, &cfg_with(full_pack(), false))
    }

    #[test]
    fn pack_corpus_every_entry_and_collisions() {
        // (input, expected) — designed so exactly the intended rules fire under
        // the FULL 67-rule pack. Covers every entry plus the substring/prose
        // collisions that word boundaries must defuse.
        let cases: &[(&str, &str)] = &[
            // --- CODE_SYMBOLS: compounds survive their own singles -----------
            ("open brace x close brace", "{ x }"),
            ("open bracket close bracket open paren close paren", "[ ] ( )"),
            ("open angle bracket close angle bracket", "< >"),
            ("fat arrow and thin arrow", "=> and ->"),
            ("triple backtick block", "``` block"),        // not eaten by `backtick`
            ("double colon path", ":: path"),              // not eaten by `colon`
            ("left double ampersand right", "left && right"),
            ("a double pipe b pipe symbol c", "a || b | c"),
            // every remaining single symbol, incl. backslash (one char each)
            ("semicolon underscore backslash forward slash dollar sign hash symbol at sign percent sign asterisk tilde caret",
             "; _ \\ / $ # @ % * ~ ^"),
            // --- CODE_SYMBOLS: prose must NOT be corrupted -------------------
            ("the engine ran hot and the pipe leaked", "the engine ran hot and the pipe leaked"),
            // --- CODING_TERMS: spelling / multi-word (every entry once) ------
            ("get hub git hub kube control py test engine x node js next js nest js \
              type script java script react js mongo db web socket local host post gres \
              c plus plus c sharp dot net golang",
             "GitHub GitHub kubectl pytest nginx Node.js Next.js NestJS \
              TypeScript JavaScript React MongoDB WebSocket localhost Postgres \
              C++ C# .NET Go"),
            // --- CODING_TERMS: casing (every entry once); http !-> https -----
            ("github gitlab typescript javascript json yaml graphql oauth sqlite postgres \
              kubernetes redis api url html css http https sql",
             "GitHub GitLab TypeScript JavaScript JSON YAML GraphQL OAuth SQLite Postgres \
              Kubernetes Redis API URL HTML CSS HTTP HTTPS SQL"),
            // --- SUBSTRING SAFETY: the crown jewel. Every term here is a
            //     substring of a real English/tech word; NONE may fire. ------
            ("curl the rapid apiary in mysql and postgresql success",
             "curl the rapid apiary in mysql and postgresql success"),
            // golang != go: bare "go" is deliberately not a rule.
            ("go to the repo", "go to the repo"),
        ];
        for (input, expected) in cases {
            // Collapse the source-wrapped whitespace so multi-line literals above
            // compare as single-spaced prose.
            let want: String = expected.split_whitespace().collect::<Vec<_>>().join(" ");
            let got: String = apply_pack(input).split_whitespace().collect::<Vec<_>>().join(" ");
            assert_eq!(got, want, "pack corpus failed on input: {input:?}");
        }
    }

    #[test]
    fn pack_is_idempotent() {
        // Casing rules run over spelling-rule output in the same pass; a second
        // pass must be a fixpoint (no double-transform, no oscillation).
        for once in ["github", "type script and typescript", "http https", "sqlite sql"] {
            let a = apply_pack(once);
            let b = apply_pack(&a);
            assert_eq!(a, b, "pack not idempotent for {once:?}");
        }
    }

    #[test]
    fn pack_symbol_words_are_literal_in_prose() {
        // LOCKED CURATION TRADEOFF, not a bug: CODE_SYMBOLS is opt-in and turns
        // spoken symbol WORDS into glyphs by design. "underscore" (the verb) and
        // "caret" (often meaning the text cursor) therefore convert in prose.
        // Documented so a future edit doesn't "fix" it by accident.
        assert_eq!(apply_pack("please underscore the caret position"), "please _ the ^ position");
        // Likewise "engine x-axis": trailing '-' is a boundary, so nginx fires.
        assert_eq!(apply_pack("the engine x-axis"), "the nginx-axis");
    }

    // --- vocab canonicalization attacks --------------------------------

    #[test]
    fn vocab_length_preserved() {
        // Casing-only invariant: for reachable (ASCII/Latin) terms the match spans
        // the same characters, so both char AND byte length survive — this is what
        // keeps the caret offset (computed later on the final text) trustworthy.
        // ponytail: byte-length can only differ for Unicode compatibility-uppercase
        // forms (Kelvin U+212A, ẞ) that ASR never emits; char length always holds.
        let cfg = cfg_vocab(vec!["GitHub", "OAuth", "C++", "TypeScript"], vec![]);
        let input = "push github using oauth in c++ and typescript";
        let out = apply(input, &cfg);
        assert_eq!(out, "push GitHub using OAuth in C++ and TypeScript");
        assert_eq!(out.chars().count(), input.chars().count(), "vocab changed char length");
        assert_eq!(out.len(), input.len(), "vocab changed byte length (ASCII terms)");
    }

    #[test]
    fn vocab_regex_metachars_are_escaped() {
        // Terms full of regex metacharacters must match LITERALLY, never as a
        // pattern. If "A.B.C" leaked the dots as "any char", "axbxc" would also
        // be rewritten — this proves regex::escape holds and nothing panics.
        let cfg = cfg_vocab(vec!["C++", "C#", ".NET", "A.B.C", "kubectl+"], vec![]);
        assert_eq!(
            apply("i use c++ and c# and .net with a.b.c but not axbxc via kubectl+", &cfg),
            "i use C++ and C# and .NET with A.B.C but not axbxc via kubectl+",
        );
    }

    #[test]
    fn vocab_fifty_terms_scale_and_boundaries() {
        // VOCAB_MAX = 50. Prove the full-capacity vocab applies without panic and
        // keeps word boundaries: canonical members re-case, non-members and
        // substrings do not. Term "Word05" must not fire inside "Word050".
        let terms: Vec<String> = (1..=50).map(|n| format!("Word{n:02}")).collect();
        let cfg = cfg_vocab(terms.iter().map(String::as_str).collect(), vec![]);
        assert_eq!(
            apply("word05 and word50 and word050 and notaterm", &cfg),
            "Word05 and Word50 and word050 and notaterm",
        );
    }

    #[test]
    fn vocab_unicode_terms() {
        // Non-ASCII vocab: umlaut re-cases (1 char in, 1 char out), CJK is
        // caseless so it rides through untouched, and neither corrupts neighbours.
        let cfg = cfg_vocab(vec!["Ümlaut", "東京"], vec![]);
        let out = apply("the ümlaut in 東京 today", &cfg);
        assert_eq!(out, "the Ümlaut in 東京 today");
        assert_eq!(out.chars().count(), "the ümlaut in 東京 today".chars().count());
    }

    #[test]
    fn vocab_collides_with_pack_entry() {
        // Same span targeted by a vocab term, a pack casing rule, AND a user
        // multi-word rule. Vocab runs first, rules cascade in order — the result
        // is deterministic and sane (no double-application artifact).
        let cfg = cfg_vocab(
            vec!["API"],
            vec![rule("api", "API"), rule("api call", "endpoint")],
        );
        assert_eq!(apply("make the api call now", &cfg), "make the endpoint now");
    }

    #[test]
    fn interaction_vocab_rule_pack_overlap_with_cursor() {
        // vocab + snippet rule (bearing {cursor}) + pack casing rule all touching
        // one utterance. Offset is measured on the FINAL text, so vocab's earlier
        // rewrite cannot desync it.
        let cfg = cfg_vocab(
            vec!["GitHub"],
            vec![rule("push", "git push{cursor}"), rule("github", "GitHub")],
        );
        let (text, off) = apply_with_cursor("push to github", &cfg);
        assert_eq!(text, "git push to GitHub");
        assert_eq!(off, Some(" to GitHub".chars().count())); // 10
    }

    // --- multi-line snippet apply-side behavior ------------------------

    #[test]
    fn snippet_crlf_and_cursor() {
        // Textarea-authored value with Windows CRLF: inserted verbatim (NoExpand),
        // and the caret offset counts CRLF as its two real chars.
        let cfg = cfg_with(vec![rule("sig", "line1{cursor}\r\nline2")], false);
        let (text, off) = apply_with_cursor("sig", &cfg);
        assert_eq!(text, "line1\r\nline2");
        assert_eq!(off, Some(7)); // "\r\nline2": \r \n l i n e 2
    }

    #[test]
    fn snippet_only_newlines_and_trailing() {
        // A value that is nothing but newlines survives; trailing newlines after a
        // {cursor} still count toward the offset.
        assert_eq!(apply("blank", &cfg_with(vec![rule("blank", "\n\n\n")], false)), "\n\n\n");
        let cfg = cfg_with(vec![rule("x", "hello\n\n{cursor}")], false);
        let (text, off) = apply_with_cursor("x", &cfg);
        assert_eq!(text, "hello\n\n");
        assert_eq!(off, Some(0)); // cursor at the very end, trailing newlines before it
    }

    #[test]
    fn snippet_value_contains_txt_delimiter() {
        // A snippet whose VALUE contains " -> " (the TXT import/export delimiter,
        // owned by commands.rs). The apply side must treat it as literal text, not
        // structure — NoExpand guarantees it.
        let cfg = cfg_with(vec![rule("arrow", "a -> b -> c")], false);
        assert_eq!(apply("arrow", &cfg), "a -> b -> c");
    }
}
