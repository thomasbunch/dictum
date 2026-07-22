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
    for rule in &cfg.replacements {
        text = apply_rule(&text, rule);
    }
    let offset = cursor_back_offset(&text);
    (text.replace(CURSOR, ""), offset)
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

// --- import/export -----------------------------------------------------

/// One rule per line: `heard<TAB>printed` or `heard -> printed`. Blank lines skipped.
pub fn parse_txt(input: &str) -> Vec<Replacement> {
    input
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let (heard, printed) = line.split_once('\t').or_else(|| line.split_once(" -> "))?;
            Some(Replacement { heard: heard.trim().to_string(), printed: printed.trim().to_string() })
        })
        .collect()
}

pub fn to_txt(rules: &[Replacement]) -> String {
    rules.iter().map(|r| format!("{}\t{}", r.heard, r.printed)).collect::<Vec<_>>().join("\n")
}

pub fn parse_json(input: &str) -> Result<Vec<Replacement>, String> {
    serde_json::from_str(input).map_err(|e| e.to_string())
}

pub fn to_json(rules: &[Replacement]) -> String {
    serde_json::to_string_pretty(rules).unwrap_or_default()
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

    #[test]
    fn txt_round_trip() {
        let rules = vec![rule("teh", "the"), rule("point break", "Point Break")];
        assert_eq!(parse_txt(&to_txt(&rules)), rules);
    }

    #[test]
    fn txt_parses_arrow_format() {
        let parsed = parse_txt("teh -> the\ngonna -> going to");
        assert_eq!(parsed, vec![rule("teh", "the"), rule("gonna", "going to")]);
    }

    #[test]
    fn json_round_trip() {
        let rules = vec![rule("api", "API"), rule("json", "JSON")];
        assert_eq!(parse_json(&to_json(&rules)).unwrap(), rules);
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
}
