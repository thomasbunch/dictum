//! Output guardrail for the LLM reformatter (PLAN-0.3 §2.4). `check(input, out)`
//! validates the model's rewrite against the deterministic-pipeline text; on any
//! trip the caller falls back to `det` (the light-cleanup floor), never the raw
//! LLM output. Ordered cheap->expensive, short-circuit on the first trip.
//!
//! `input` = deterministic pipeline result (post voice/replacements/filetag, pre-LLM).
//! `out`   = raw LLM output (untrusted).
//!
//! Residual unguarded class (accepted, not covered): a *subset-shaped* answer to an
//! unpunctuated yes/no question — e.g. det "should I use a mutex or a channel here" ->
//! out "Use a channel here." Gate 4 only trips when the answer *elaborates* (out/in >
//! `QUESTION_RATIO`); a terse answer that stays shorter than the question reads exactly
//! like a legit imperative tightening (cf. fixture 9) and cannot be told apart by
//! length/overlap alone. It passes.

use regex::Regex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trip {
    Empty,
    Preamble,
    LengthRatio,
    QuestionLost,
    IdentifierLost,
    PolarityFlip,
}

// ponytail: UNVALIDATED design constants (PLAN-0.3 §2.4) — pinned to §2.5 fixtures; re-tune if the model changes.
const MIN_OUT_CHARS: usize = 3; // gate 1: shorter output than this is degenerate
const DEGENERATE_IN: usize = 60; // gate 1: a long input...
const DEGENERATE_OUT: usize = 15; // ...that collapses below this many out chars is a drop
const RATIO_MIN_LEN: usize = 40; // gate 3: below this, use the content-word floor not the ratio
const RATIO_LOW: f64 = 0.35; // gate 3: out/in below this = content dropped
const RATIO_HIGH: f64 = 1.8; // gate 3: out/in above this = content invented
const SHORT_FLOOR: usize = 2; // gate 3: min distinct content words a short input must retain
const QUESTION_RATIO: f64 = 1.3; // gate 4: a lost '?' only trips when output also grew past this
const JACCARD_MIN: f64 = 0.5; // gate 5: content-word overlap floor (F7 pins this at ~0.545)

/// Validate an LLM reformat against the deterministic text. `Ok(())` = safe to inject `out`.
pub fn check(input: &str, out: &str) -> Result<(), Trip> {
    let in_chars = input.chars().count();
    let out_chars = out.chars().count();

    // 1. Empty / degenerate.
    if out.trim().chars().count() < MIN_OUT_CHARS {
        return Err(Trip::Empty);
    }
    if in_chars > DEGENERATE_IN && out_chars < DEGENERATE_OUT {
        return Err(Trip::Empty);
    }

    // 2. Preamble / refusal / fence — bare "okay/sure" openers must NOT trip (§2.4).
    if is_preamble(input, out) {
        return Err(Trip::Preamble);
    }

    // 3. Length-ratio band; short inputs use an absolute content-word floor instead.
    if in_chars >= RATIO_MIN_LEN {
        let r = out_chars as f64 / in_chars as f64;
        if r < RATIO_LOW || r > RATIO_HIGH {
            return Err(Trip::LengthRatio);
        }
    } else {
        let in_cw = content_words(input).len();
        let out_cw = content_words(out).len();
        if out_cw < SHORT_FLOOR.min(in_cw) {
            return Err(Trip::LengthRatio);
        }
    }

    // 4. Question stays a question — trip only if the marker is gone AND output grew.
    if is_interrogative(input) && !out.contains('?') {
        let r = out_chars as f64 / in_chars.max(1) as f64;
        if r > QUESTION_RATIO {
            return Err(Trip::QuestionLost);
        }
    }

    // 5. Identifier + bare-number preservation + polarity + content overlap.
    // Each code-shaped token OR standalone number in det must survive into out, UNLESS a
    // self-correction cue ("no wait", "i mean", …) follows it within CORRECTION_WINDOW
    // tokens — the abandoned branch of a spoken correction is meant to be dropped.
    let out_lower = out.to_lowercase();
    let out_tokens: std::collections::HashSet<String> = norm_tokens(out).into_iter().collect();
    let toks: Vec<String> = reconstruct(input).split_whitespace().map(|t| t.to_lowercase()).collect();
    for (i, raw) in toks.iter().enumerate() {
        let tok = raw.trim_matches(TRIM);
        if tok.is_empty() {
            continue;
        }
        let is_num = tok.chars().all(|c| c.is_ascii_digit());
        if !is_num && !is_identifier(tok) {
            continue;
        }
        // Numbers must match as a whole token ("30" must not "match" inside "300");
        // identifiers keep substring matching (v0.3.0, src/main.rs read fine as substrings).
        let present = if is_num { out_tokens.contains(tok) } else { out_lower.contains(tok) };
        if present || abandoned(&toks, i) {
            continue;
        }
        return Err(Trip::IdentifierLost);
    }
    // Polarity: any dropped negation flips meaning. Presence-to-zero only catches
    // single-negation sentences; a strict decrease catches partial flips too.
    if negation_count(out) < negation_count(input) {
        return Err(Trip::PolarityFlip);
    }
    // Content-word Jaccard. No dedicated ContentLost variant in the fixed enum, so a
    // low-overlap answer-substitution reports as IdentifierLost (the preservation gate).
    let a = content_words(&reconstruct(input));
    let b = content_words(out);
    if !a.is_empty() || !b.is_empty() {
        let inter = a.intersection(&b).count();
        let union = a.union(&b).count();
        if union > 0 && (inter as f64 / union as f64) < JACCARD_MIN {
            return Err(Trip::IdentifierLost);
        }
    }

    Ok(())
}

// --- gate 2 helpers ----------------------------------------------------

const OPENERS: &[&str] =
    &["okay", "sure", "certainly", "absolutely", "of course", "got it", "understood", "alright", "no problem"];
const STRONG_META: &[&str] = &[
    "cleaned version",
    "cleaned up",
    "cleaned-up",
    "reformatted",
    "rewritten",
    "revised version",
    "as requested",
    "hope this helps",
    "here is the cleaned",
    "here's the cleaned",
];
const WEAK_META: &[&str] = &["here is", "here's", "here are"];
const REFUSAL: &[&str] = &[
    "i can't",
    "i cannot",
    "i'm sorry",
    "i am sorry",
    "as an ai",
    "i'm unable",
    "i am unable",
    "i won't be able",
    "i will not",
];

fn is_preamble(input: &str, out: &str) -> bool {
    let low = out.trim_start().to_lowercase();
    let lead: String = low.chars().take(50).collect();

    if REFUSAL.iter().any(|r| low.contains(r)) {
        return true;
    }
    if STRONG_META.iter().any(|m| lead.contains(m)) {
        return true;
    }
    // A chat opener is only a preamble when followed by a colon or a "here is" meta-phrase;
    // "Okay, we should…" / "Sure thing…" are ordinary spoken openers and must pass.
    if let Some(op) = OPENERS.iter().find(|o| lead.starts_with(**o)) {
        let after = lead[op.len()..].trim_start();
        if after.starts_with(':') || WEAK_META.iter().any(|m| lead.contains(m)) {
            return true;
        }
    }
    // A code fence or blockquote the input never had is invented structure.
    if out.contains("```") && !input.contains("```") {
        return true;
    }
    if leading_blockquote(out) && !leading_blockquote(input) {
        return true;
    }
    false
}

fn leading_blockquote(text: &str) -> bool {
    text.lines().any(|l| l.trim_start().starts_with('>'))
}

// --- tokenisation / content words --------------------------------------

const TRIM: &[char] = &['.', ',', ';', ':', '!', '?', '(', ')', '[', ']', '{', '}', '"', '\'', '—', '–'];

// Function words + discourse fillers + spelled-out numbers; dropped before overlap/floor.
const STOPWORDS: &[&str] = &[
    "a", "an", "the", "and", "or", "but", "if", "so", "to", "of", "in", "on", "at", "for", "with", "from", "by", "as",
    "is", "are", "was", "were", "be", "been", "being", "am", "do", "does", "did", "has", "have", "had", "will", "would",
    "can", "could", "should", "may", "might", "must", "i", "you", "he", "she", "it", "we", "they", "me", "him", "her",
    "us", "them", "my", "your", "his", "its", "our", "their", "this", "that", "these", "those", "then", "than", "when",
    "while", "up", "out", "down", "off", "over", "just", "like", "well", "okay", "ok", "oh", "hey", "um", "uh", "uhm",
    "erm", "really", "very", "some", "any", "no", "not", "because", "about", "into", "onto", "per", "via", "i've",
    "you've", "we've", "i'm", "you're", "it's", "that's", "there's", "let's", "lets", "actually", "basically", "one",
    "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "zero", "point",
];

fn norm_tokens(text: &str) -> Vec<String> {
    text.split_whitespace().map(|t| t.trim_matches(TRIM).to_lowercase()).collect()
}

fn content_words(text: &str) -> std::collections::HashSet<String> {
    norm_tokens(text)
        .into_iter()
        .filter(|t| !t.is_empty() && !STOPWORDS.contains(&t.as_str()) && !t.chars().all(|c| c.is_ascii_digit()))
        .collect()
}

// --- gate 4: interrogative ---------------------------------------------

const INTERROG: &[&str] = &["why", "how", "what", "whats", "when", "where", "who", "whom", "whose", "which"];
const FILLERS: &[&str] =
    &["so", "like", "well", "okay", "ok", "um", "uh", "uhm", "erm", "oh", "hey", "yeah", "basically", "actually", "just", "and", "but"];
// Aux-fronted yes/no start: aux immediately followed by a subject word ("should I…", "can we…",
// "is the…"). Weak signal only — gate 4 still requires the r>QUESTION_RATIO elaboration condition,
// so fixture 9's "can you make…" -> imperative (which shrinks) stays passing.
const AUX: &[&str] =
    &["should", "shall", "can", "could", "do", "does", "did", "is", "are", "was", "were", "will", "would", "may", "might"];
const AUX_SUBJ: &[&str] = &["we", "i", "you", "it", "the", "they", "he", "she", "there", "this", "that"];

fn is_interrogative(text: &str) -> bool {
    if text.trim_end().ends_with('?') {
        return true;
    }
    // First non-filler tokens decide (skips leading "so like…" scaffolding).
    let toks: Vec<String> = norm_tokens(text).into_iter().filter(|t| !t.is_empty() && !FILLERS.contains(&t.as_str())).collect();
    let first = match toks.first() {
        Some(t) => t.as_str(),
        None => return false,
    };
    if INTERROG.contains(&first) {
        return true;
    }
    AUX.contains(&first) && toks.get(1).is_some_and(|s| AUX_SUBJ.contains(&s.as_str()))
}

// --- gate 5: negation --------------------------------------------------

// Bare "no" is excluded on purpose: it collides with the discourse "actually no…"
// self-correction (fixture 5) that the reformatter correctly drops.
const NEG_WORDS: &[&str] = &[
    "not", "never", "cannot", "without", "none", "nor", "neither", "dont", "doesnt", "didnt", "wont", "cant", "isnt",
    "arent", "wasnt", "werent", "havent", "hasnt", "hadnt", "wouldnt", "couldnt", "shouldnt", "aint",
];

fn negation_count(text: &str) -> usize {
    let low = text.to_lowercase();
    let mut n = low.matches("no longer").count();
    for t in norm_tokens(&low) {
        if NEG_WORDS.contains(&t.as_str()) || t.ends_with("n't") {
            n += 1;
        }
    }
    n
}

// --- gate 5: identifiers -----------------------------------------------

/// Cheap spoken-form joins so post-det identifiers read canonically: "X dot Y"->"X.Y",
/// "X underscore Y"->"X_Y". Number-word forms ("two point five") are intentionally NOT
/// reconstructed (not cheap, and dropping them only under-extracts — safe).
// ponytail: naive fixpoint join; "the dot product"->"the.product" is a known false-join
// whose only cost is a good reformat falling back to the deterministic floor.
fn reconstruct(text: &str) -> String {
    let dot = Regex::new(r"(?i)\b(\w+) dot (\w+)\b").unwrap();
    let und = Regex::new(r"(?i)\b(\w+) underscore (\w+)\b").unwrap();
    let mut s = text.to_string();
    for _ in 0..8 {
        let a = dot.replace_all(&s, "$1.$2").into_owned();
        let b = und.replace_all(&a, "${1}_${2}").into_owned();
        if b == s {
            break;
        }
        s = b;
    }
    s
}

// A preservable token (identifier or bare number) is exempt from the preservation gate
// when one of these self-correction cues follows within CORRECTION_WINDOW tokens: the
// speaker named the wrong thing, then corrected it ("...coordinator.rs no wait scheduler.rs").
const CORRECTION_WINDOW: usize = 6;
const CUE_PHRASES: &[&str] = &["no wait", "no,", "actually", "sorry", "i mean", "make that", "scratch that"];

fn abandoned(toks: &[String], i: usize) -> bool {
    let end = (i + 1 + CORRECTION_WINDOW).min(toks.len());
    if i + 1 >= end {
        return false;
    }
    let win = &toks[i + 1..end];
    let joined = win.join(" ");
    if CUE_PHRASES.iter().any(|c| joined.contains(c)) {
        return true;
    }
    // "not X," — self-correction that names then drops the wrong token.
    win.windows(2).any(|w| w[0].trim_matches(TRIM) == "not" && w[1].ends_with(','))
}

/// A code-shaped token: dotted, underscored, camelCase, path, flag, sigil, or alnum-mix.
fn is_identifier(t: &str) -> bool {
    if t.chars().count() < 2 {
        return false;
    }
    let has_alpha = t.chars().any(|c| c.is_ascii_alphabetic());
    let has_digit = t.chars().any(|c| c.is_ascii_digit());
    if t.contains('_') && (has_alpha || has_digit) {
        return true;
    }
    if t.contains('/') || t.contains('\\') {
        return true;
    }
    if t.starts_with('-') && t.trim_start_matches('-').chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
        return true; // flag: -v, --no-default-features
    }
    if t.starts_with('$') {
        return true;
    }
    // internal dot/colon flanked by alnum (foo.rs, v0.3.0, Qwen2.5, docker tag nginx:alpine)
    let ch: Vec<char> = t.chars().collect();
    for i in 1..ch.len().saturating_sub(1) {
        if (ch[i] == '.' || ch[i] == ':') && ch[i - 1].is_alphanumeric() && ch[i + 1].is_alphanumeric() {
            return true;
        }
    }
    // camelCase: a lowercase immediately followed by an uppercase
    if ch.windows(2).any(|w| w[0].is_lowercase() && w[1].is_uppercase()) {
        return true;
    }
    // alphanumeric mix: version tags, numbers-with-units (v2, 512mb, b10078)
    has_alpha && has_digit
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- gate 1: empty / degenerate ----

    #[test]
    fn empty_output_trips() {
        assert_eq!(check("some real long dictation text about the settings page", ""), Err(Trip::Empty));
    }
    #[test]
    fn whitespace_output_trips() {
        assert_eq!(check("some real long dictation text about the settings page", "   \n\t "), Err(Trip::Empty));
    }
    #[test]
    fn tiny_output_trips() {
        assert_eq!(check("some real long dictation text about the settings page", "ok"), Err(Trip::Empty));
    }
    #[test]
    fn long_input_collapsed_output_trips() {
        let input = "this is a reasonably long dictation about the settings page and the hotkeys";
        assert_eq!(check(input, "Settings."), Err(Trip::Empty));
    }
    #[test]
    fn moderate_compression_passes_gate1() {
        let input = "please clean up the settings page and also the hotkey handling right now";
        assert!(check(input, "Clean up the settings page and hotkey handling.").is_ok());
    }

    // ---- gate 2: preamble / refusal / fence ----

    #[test]
    fn meta_preamble_trips() {
        let out = "Sure, here's the cleaned version: Update the settings page.";
        assert_eq!(check("update the settings page and hotkeys please", out), Err(Trip::Preamble));
    }
    #[test]
    fn refusal_trips() {
        let out = "I'm sorry, I can't help rewrite that message.";
        assert_eq!(check("rewrite this rude message for me right now", out), Err(Trip::Preamble));
    }
    #[test]
    fn code_fence_the_input_lacked_trips() {
        assert_eq!(check("update the config file to enable dark mode", "```\nUpdate the config file.\n```"), Err(Trip::Preamble));
    }
    #[test]
    fn blockquote_the_input_lacked_trips() {
        assert_eq!(check("reply to sarah about the demo going well", "> Reply to Sarah about the demo."), Err(Trip::Preamble));
    }
    #[test]
    fn bare_okay_opener_passes() {
        // The §2.4 anti-false-positive case: "okay" is an ordinary spoken opener.
        let input = "okay so we should just cache the sessions in postgres";
        assert!(check(input, "Okay, we should cache the sessions in Postgres.").is_ok());
    }
    #[test]
    fn bare_sure_opener_passes() {
        let input = "sure thing that works fine for me and the team";
        assert!(check(input, "Sure thing, that works fine for me and the team.").is_ok());
    }

    // ---- gate 3: length ratio / short floor ----

    #[test]
    fn ratio_too_low_trips() {
        let input = "the hotkey is completely broken and does not stop the recording when i press it a second time quickly now";
        assert_eq!(check(input, "The hotkey is broken now."), Err(Trip::LengthRatio));
    }
    #[test]
    fn ratio_too_high_trips() {
        let input = "please gray out the export button when there is no history at all";
        let out = "Please gray out the export button whenever there is no history at all, and also make sure to disable \
                   it and show a tooltip explaining exactly why it is disabled right now today.";
        assert_eq!(check(input, out), Err(Trip::LengthRatio));
    }
    #[test]
    fn short_input_content_floor_trips() {
        assert_eq!(check("gray out the export button now", "Done."), Err(Trip::LengthRatio));
    }
    #[test]
    fn moderate_compression_passes_ratio() {
        let input = "so um the settings page really needs a dark mode toggle added to it";
        assert!(check(input, "The settings page needs a dark mode toggle.").is_ok());
    }
    #[test]
    fn short_input_kept_words_passes() {
        assert!(check("gray out the export button now", "Gray out the button.").is_ok());
    }

    // ---- gate 4: question stays a question ----

    #[test]
    fn answered_question_trips() {
        // Spike-observed failure class: the model answers instead of reformatting.
        let input = "why does the app take so long to start up when i have a big history file";
        let out = "The app takes so long to start up because it re-indexes the whole history file on every single launch.";
        assert_eq!(check(input, out), Err(Trip::QuestionLost));
    }
    #[test]
    fn question_that_keeps_marker_passes() {
        let input = "why does the app take so long to start up with a big history file";
        assert!(check(input, "Why does the app take so long to start up with a big history file?").is_ok());
    }
    #[test]
    fn lost_marker_but_not_longer_passes() {
        // Marker gone but output shrank (r <= 1.3) — a legit tightening, not an answer.
        let input = "why is the export button always grayed out even when there is real history";
        assert!(check(input, "The export button stays grayed out even with history.").is_ok());
    }

    // ---- gate 5: identifiers / polarity / overlap ----

    #[test]
    fn identifier_hallucination_trips() {
        // Spike-observed failure: remove_fillers -> remove_underscorers.
        let input = "in remove_fillers we strip the comma but sometimes leave a double space";
        let out = "In remove_underscorers we strip the comma but sometimes leave a double space.";
        assert_eq!(check(input, out), Err(Trip::IdentifierLost));
    }
    #[test]
    fn flag_identifier_dropped_trips() {
        let input = "run the build with --no-default-features to skip the native step";
        assert_eq!(check(input, "Run the build to skip the native step."), Err(Trip::IdentifierLost));
    }
    #[test]
    fn version_identifier_changed_trips() {
        let input = "please download v0.3.0 from the releases page right now";
        assert_eq!(check(input, "Download v0.4.0 from the releases page."), Err(Trip::IdentifierLost));
    }
    #[test]
    fn path_identifier_dropped_trips() {
        let input = "edit src/main.rs and then save the whole file";
        assert_eq!(check(input, "Edit the main file and then save it."), Err(Trip::IdentifierLost));
    }
    #[test]
    fn answer_substitution_low_overlap_trips() {
        // Length-matched, no identifiers, no negation, not a question — caught by Jaccard.
        let input = "the deploy keeps failing on the build step every single time we push";
        assert_eq!(check(input, "You should check the CI logs and clear the cache before retrying it now."), Err(Trip::IdentifierLost));
    }
    #[test]
    fn polarity_flip_trips() {
        let input = "don't delete the old config file before you check it";
        assert_eq!(check(input, "Delete the old config file before you check it."), Err(Trip::PolarityFlip));
    }
    #[test]
    fn identifier_preserved_passes() {
        let input = "in remove_fillers we strip the comma but sometimes leave a space";
        assert!(check(input, "remove_fillers strips the comma but sometimes leaves a space.").is_ok());
    }
    #[test]
    fn flag_identifier_preserved_passes() {
        let input = "run the build with --no-default-features to skip the native step";
        assert!(check(input, "Run the build with --no-default-features to skip the native step.").is_ok());
    }
    #[test]
    fn version_identifier_preserved_passes() {
        let input = "please download v0.3.0 from the releases page right now";
        assert!(check(input, "Download v0.3.0 from the releases page.").is_ok());
    }
    #[test]
    fn path_identifier_preserved_passes() {
        let input = "edit src/main.rs and then save the whole file";
        assert!(check(input, "Edit src/main.rs and then save the file.").is_ok());
    }
    #[test]
    fn polarity_preserved_passes() {
        let input = "don't delete the old config file before you check";
        assert!(check(input, "Don't delete the old config file before you check.").is_ok());
    }
    #[test]
    fn partial_negation_flip_trips() {
        // Finding :79 (critical) — one of two negations dropped; out still has a negation,
        // so the old presence-to-zero test missed it. A strict decrease catches it.
        let input = "don't delete the old config and never touch the new one";
        assert_eq!(check(input, "Delete the old config and never touch the new one."), Err(Trip::PolarityFlip));
    }

    // ---- finding :74 — identifier self-correction is allowed to drop the abandoned branch ----

    #[test]
    fn identifier_self_correction_passes() {
        // det names coordinator.rs, corrects to scheduler.rs; dropping the abandoned one is correct.
        let input = "move the retry logic into coordinator.rs no wait into scheduler.rs";
        assert!(check(input, "Move the retry logic into scheduler.rs.").is_ok());
    }
    #[test]
    fn identifier_self_correction_spoken_form_passes() {
        // Same case via the reformat.rs spoken fixture ("dot rs", not ".rs").
        let input = "move the retry logic into coordinator dot rs no wait into scheduler dot rs";
        assert!(check(input, "Move the retry logic into scheduler.rs.").is_ok());
    }
    #[test]
    fn corrupted_mandatory_identifier_still_trips() {
        // No correction cue -> the identifier stays mandatory even after the self-correction fix.
        let input = "move the retry logic into scheduler.rs and log it";
        assert_eq!(check(input, "Move the retry logic into worker.rs and log it."), Err(Trip::IdentifierLost));
    }

    // ---- finding :181 — bare numbers are preservable (ports / timeouts / counts) ----

    #[test]
    fn bare_number_changed_trips() {
        let input = "set the connection timeout to 30 seconds and retry 3 times";
        assert_eq!(check(input, "Set the connection timeout to 300 seconds and retry 3 times."), Err(Trip::IdentifierLost));
    }
    #[test]
    fn corrected_number_passes() {
        // "port 80, I mean 8080" — the abandoned 80 may be dropped; 8080 must survive.
        // (Fuller sentence than the bare 4-word form so the 'mean' token doesn't sink Jaccard.)
        let input = "set the port to 80, I mean 8080, in the config";
        assert!(check(input, "Set the port to 8080 in the config.").is_ok());
    }

    // ---- finding :187 — aux-fronted yes/no questions as a weak signal ----

    #[test]
    fn answered_yesno_question_elaboration_trips() {
        // Aux-fronted ("should I…"), no '?', and the answer elaborates past QUESTION_RATIO.
        let input = "should I use a mutex here";
        let out = "You should use a mutex here because it prevents concurrent access to the shared state and avoids the race condition entirely.";
        assert_eq!(check(input, out), Err(Trip::QuestionLost));
    }
    #[test]
    fn subset_answer_to_yesno_passes_documented_gap() {
        // Residual unguarded class (see module doc): terse subset answer stays shorter than the
        // question, reads like a legit imperative tightening, and passes.
        let input = "should I use a mutex or a channel here";
        assert!(check(input, "Use a channel here.").is_ok());
    }

    // ---- EVAL.md gap — colon-suffixed docker image tags are identifiers ----

    #[test]
    fn colon_tag_is_identifier() {
        assert!(is_identifier("postgres:15-alpine")); // (also caught by alnum-mix)
        assert!(is_identifier("nginx:alpine")); // no digit — only the colon rule catches this
    }
    #[test]
    fn colon_tag_corruption_trips() {
        let input = "pull the postgres:15-alpine image before deploying the stack";
        assert_eq!(check(input, "Pull the postgres15-alpine image before deploying the stack."), Err(Trip::IdentifierLost));
    }

    // ---- unit checks on the tricky helpers ----

    #[test]
    fn reconstruct_joins_spoken_forms() {
        assert_eq!(reconstruct("edit replacements dot rs now"), "edit replacements.rs now");
        assert_eq!(reconstruct("the remove underscore fillers fn"), "the remove_fillers fn");
        assert_eq!(reconstruct("open src dot main dot rs"), "open src.main.rs");
    }

    // ---- §2.5 fixtures: every reformat must pass ----

    #[test]
    fn plan_2_5_fixtures_all_pass() {
        // (deterministic-pipeline approximation of SPOKEN, OUTPUT column). All must be Ok.
        let fixtures: &[(&str, &str)] = &[
            (
                "for the settings page we need to add a dark mode toggle and also change the hotkey and then export history to json",
                "For the settings page:\n- Add a dark mode toggle\n- Let users change the hotkey\n- Export history to JSON",
            ),
            (
                "to reproduce you first open the app then start recording and while recording you unplug the mic and that's when it blows up",
                "To reproduce the crash:\n1. Open the app\n2. Start recording\n3. Unplug the mic while recording",
            ),
            (
                "okay so basically I think we should use the existing cache instead of building a new one",
                "I think we should use the existing cache instead of building a new one.",
            ),
            (
                "so like why does the app take so long to start up when I've got a big history file is that the indexing thing",
                "Why does the app take so long to start up when I have a big history file? Is that the indexing?",
            ),
            (
                "let's cache the sessions in redis, actually no, use postgres, just a regular table is fine",
                "Cache the sessions in Postgres — a regular table is fine.",
            ),
            (
                "hey sarah just wanted to say the demo went really well today they loved the new tape view so thanks for pushing on that",
                "Hey Sarah — just wanted to say the demo went really well today. They loved the new TAPE view, so thanks for pushing on that.",
            ),
            (
                "in replacements dot rs the remove underscore fillers function is stripping the comma but leaving a double space sometimes",
                "In replacements.rs, remove_fillers strips the comma but sometimes leaves a double space.",
            ),
            (
                "so the hotkey thing is broken like when i press it once it starts but if i press it again real quick it doesn't stop it just keeps going and i have to click the tray",
                "The hotkey is broken: pressing it once starts recording, but pressing it again quickly doesn't stop it — it keeps going and I have to click the tray to stop.",
            ),
            (
                "can you make the can you make the export button gray out when there's no history",
                "Gray out the export button when there's no history.",
            ),
            (
                "let's switch the model to qwen two point five because phi keeps adding a preamble we don't want",
                "Switch the model to Qwen2.5 — Phi keeps adding a preamble we don't want.",
            ),
            (
                "oh what if the little waveform in the hud pulsed when it's actually hearing you talk like reacting to volume",
                "What if the waveform in the HUD pulsed when it's actually hearing you — reacting to your volume?",
            ),
            ("just commit this and push it to main", "Commit this and push it to main."),
            (
                "refactor the parser to handle empty input and write a test for it",
                "Refactor the parser to handle empty input and write a test for it.",
            ),
        ];
        for (i, (input, out)) in fixtures.iter().enumerate() {
            assert_eq!(check(input, out), Ok(()), "fixture #{} should pass, got a trip", i + 1);
        }
    }
}
