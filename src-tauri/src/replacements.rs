//! Deterministic post-ASR text transforms: filler-word removal + replacement
//! rules. Runs between ASR and inject (CONTRACTS.md). No LLM.

use crate::types::{Config, Replacement};
use regex::{NoExpand, Regex};

pub fn apply(raw: &str, cfg: &Config) -> String {
    let mut text = raw.to_string();
    if cfg.remove_fillers {
        text = remove_fillers(&text);
    }
    for rule in &cfg.replacements {
        text = apply_rule(&text, rule);
    }
    text
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
    // heard is regex::escape'd so the pattern is always valid; \b needs word
    // boundaries at both ends of the (possibly multi-word) phrase.
    let pattern = format!(r"(?i)\b{}\b", regex::escape(&rule.heard));
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
}
