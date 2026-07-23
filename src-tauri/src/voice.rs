//! VOICE — within-take spoken editing commands in the dictated transcript.
//! Deterministic, no LLM. Runs first at the post-ASR hook, before
//! replacements.rs and filetag.rs. Same safety spine as filetag.rs.
//!
//! Firing rule (PLAN-0.3 §8 — the user may literally be dictating "new line"):
//! a command fires only when its phrase stands ALONE as an utterance segment —
//! the whole content between two segment boundaries (sentence punctuation
//! `. ! ?`, a comma/semicolon, or a string edge) equals the phrase, case- and
//! whitespace-insensitive. Embedded in a clause it is left exactly as dictated
//! ("the design needs a new line of thinking", "hit new line twice" never
//! trigger). When ambiguous, do nothing.
//!
//! Commands (all operate on the CURRENT take only):
//! - "new line" / "new paragraph"      -> \n / \n\n in place of the phrase
//! - "scratch that" / "delete last sentence" -> drop the preceding sentence
//! - "all caps that"                   -> uppercase the preceding sentence
//! - "make that a list"                -> bullet the preceding sentence's
//!   comma-enumeration (3+ items); no clean enumeration => whole command is a
//!   no-op and the phrase stays as dictated.
//!
//! Commands compose left-to-right: "first point. new line. second point.
//! scratch that." resolves each in order.

/// Segment terminators. A run of these (plus string edges) brackets the
/// standalone segments a command must exactly fill.
const SEP: &[char] = &['.', '!', '?', ',', ';'];

#[derive(Clone, Copy)]
enum Cmd {
    NewLine,
    NewPara,
    Scratch,
    AllCaps,
    MakeList,
}

/// A command fires only on an EXACT segment match (whitespace-collapsed,
/// lowercased) — this is what makes it word-bounded and standalone.
fn classify(text: &str) -> Option<Cmd> {
    let norm = text.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    match norm.as_str() {
        "new line" => Some(Cmd::NewLine),
        "new paragraph" => Some(Cmd::NewPara),
        "scratch that" | "delete last sentence" => Some(Cmd::Scratch),
        "all caps that" => Some(Cmd::AllCaps),
        "make that a list" => Some(Cmd::MakeList),
        _ => None,
    }
}

/// One utterance segment: trimmed content plus the punctuation run that ended
/// it ("." / "," / "?!" / "" for the final segment). The terminator rides with
/// its segment, so deleting a segment never orphans a lone "." into a dangling
/// ". ." — the source of the punctuation-cleanup guarantee.
struct Seg {
    text: String,
    term: String,
}

enum Node {
    Seg(Seg),
    Break(u8),   // 1 = new line, 2 = new paragraph
    Raw(String), // pre-rendered block (a bullet list)
}

/// Split into segments on `SEP` runs. Empty-content segments (leading/detached
/// punctuation) are dropped along with their terminator — that is the leading-
/// orphan-punctuation cleanup.
fn parse(text: &str) -> Vec<Seg> {
    let mut segs = Vec::new();
    let mut content = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if SEP.contains(&c) {
            let mut term = String::from(c);
            while let Some(&n) = chars.peek() {
                if SEP.contains(&n) {
                    term.push(n);
                    chars.next();
                } else {
                    break;
                }
            }
            let t = content.trim();
            if !t.is_empty() {
                segs.push(Seg { text: t.to_string(), term });
            }
            content.clear();
        } else {
            content.push(c);
        }
    }
    let t = content.trim();
    if !t.is_empty() {
        segs.push(Seg { text: t.to_string(), term: String::new() });
    }
    segs
}

/// Start index of the preceding sentence in `out`: the trailing run of prose
/// segments back to (never across) a `Break`, a `Raw`, or a segment that ended
/// an earlier sentence (its term carries `. ! ?`). `None` if the last node
/// isn't prose (e.g. two commands in a row) — the command then no-ops.
fn prev_sentence_start(out: &[Node]) -> Option<usize> {
    if !matches!(out.last(), Some(Node::Seg(_))) {
        return None;
    }
    let mut start = out.len() - 1;
    while start > 0 {
        match &out[start - 1] {
            Node::Seg(p) if !p.term.contains(['.', '!', '?']) => start -= 1,
            _ => break,
        }
    }
    Some(start)
}

/// Comma/semicolon parts of the preceding sentence, each stripped of a leading
/// "and"/"&" connective. ponytail: the list signal is the COMMA — "a, b, c" or
/// "a, b, and c" bullet; "a, b and c" (no serial comma) and pure-"and" runs are
/// left as prose, because splitting on bare "and" over-splits compounds
/// ("research and development") and PLAN §8 says under-trigger when ambiguous.
fn list_items(parts: &[&str]) -> Vec<String> {
    let mut items = Vec::new();
    for p in parts {
        let mut words: Vec<&str> = p.split_whitespace().collect();
        if matches!(words.first(), Some(&w) if w.eq_ignore_ascii_case("and") || w == "&") {
            words.remove(0);
        }
        if !words.is_empty() {
            items.push(words.join(" "));
        }
    }
    items
}

/// Replace the preceding sentence with a bullet block. Returns false (a no-op)
/// unless the sentence is a clean 3+ comma-enumeration.
fn make_list(out: &mut Vec<Node>) -> bool {
    let Some(start) = prev_sentence_start(out) else { return false };
    let parts: Vec<&str> = out[start..]
        .iter()
        .filter_map(|n| match n {
            Node::Seg(s) => Some(s.text.as_str()),
            _ => None,
        })
        .collect();
    let items = list_items(&parts);
    if items.len() < 3 {
        return false;
    }
    let block = items.iter().map(|i| format!("- {i}")).collect::<Vec<_>>().join("\n");
    out.truncate(start);
    out.push(Node::Raw(block));
    true
}

fn render(nodes: &[Node]) -> String {
    let mut out = String::new();
    for node in nodes {
        match node {
            Node::Break(n) => {
                while out.ends_with(' ') {
                    out.pop();
                }
                for _ in 0..*n {
                    out.push('\n');
                }
            }
            Node::Raw(block) => {
                while out.ends_with(' ') {
                    out.pop();
                }
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(block);
                out.push('\n'); // block is line-level; keep following prose off the last bullet
            }
            Node::Seg(s) => {
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push(' ');
                }
                out.push_str(&s.text);
                out.push_str(&s.term);
            }
        }
    }
    out.trim().to_string()
}

/// Apply within-take voice commands to a raw ASR transcript.
pub fn apply(text: &str) -> String {
    let mut out: Vec<Node> = Vec::new();
    for seg in parse(text) {
        match classify(&seg.text) {
            None => out.push(Node::Seg(seg)),
            Some(Cmd::NewLine) => out.push(Node::Break(1)),
            Some(Cmd::NewPara) => out.push(Node::Break(2)),
            Some(Cmd::Scratch) => {
                if let Some(start) = prev_sentence_start(&out) {
                    out.truncate(start);
                }
            }
            Some(Cmd::AllCaps) => {
                if let Some(start) = prev_sentence_start(&out) {
                    for node in &mut out[start..] {
                        if let Node::Seg(s) = node {
                            s.text = s.text.to_uppercase();
                        }
                    }
                }
            }
            Some(Cmd::MakeList) => {
                // No clean enumeration: leave the phrase in place (spec no-op).
                if !make_list(&mut out) {
                    out.push(Node::Seg(seg));
                }
            }
        }
    }
    render(&out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands() {
        // (input, expected). Boundary guards, happy paths, composition, edges.
        let cases: &[(&str, &str)] = &[
            // --- happy paths -------------------------------------------------
            ("line one. new line. line two.", "line one.\nline two."),
            ("intro. new paragraph. body.", "intro.\n\nbody."),
            ("use redis. scratch that. use postgres.", "use postgres."),
            ("keep this. delete this. delete last sentence.", "keep this."),
            ("ship it today. all caps that.", "SHIP IT TODAY."),
            (
                "we need dark mode, a hotkey, and json export. make that a list.",
                "- we need dark mode\n- a hotkey\n- json export",
            ),
            ("red, green, blue. make that a list.", "- red\n- green\n- blue"),
            // --- make-that-a-list no-ops (leave the phrase) ------------------
            (
                "commit this and push it. make that a list.",
                "commit this and push it. make that a list.",
            ),
            ("ship it. make that a list.", "ship it. make that a list."),
            ("make that a list", "make that a list"),
            // Conservative under-trigger: no serial comma before "and" => prose.
            (
                "apples, oranges and bananas. make that a list.",
                "apples, oranges and bananas. make that a list.",
            ),
            // --- false positives: phrase embedded in a clause, never fires ---
            ("the design needs a new line of thinking", "the design needs a new line of thinking"),
            ("and then hit new line twice", "and then hit new line twice"),
            ("how do I delete last sentence in vim", "how do I delete last sentence in vim"),
            ("we use all caps that day for headers", "we use all caps that day for headers"),
            ("please don't scratch that surface", "please don't scratch that surface"),
            // --- MUST trigger: set off by sentence punctuation ---------------
            ("end of thought. new line. next point.", "end of thought.\nnext point."),
            // --- composition -------------------------------------------------
            ("first point. new line. second point. scratch that.", "first point."),
            ("alpha. new paragraph. beta. all caps that.", "alpha.\n\nBETA."),
            // --- case / punctuation variants ---------------------------------
            ("Line One. NEW LINE. Line Two.", "Line One.\nLine Two."),
            ("item one, new line, item two", "item one,\nitem two"),
            ("really? new line. yes.", "really?\nyes."),
            // --- edges -------------------------------------------------------
            ("", ""),
            ("   ", ""),
            ("just some normal dictation here", "just some normal dictation here"),
            ("scratch that", ""),
            ("new line", ""),
            ("a. b. c. scratch that.", "a. b."),
            (
                "the api, the ui, and the db. all caps that.",
                "THE API, THE UI, AND THE DB.",
            ),
            ("first. scratch that. second.", "second."),
            (". hello world", "hello world"),
            (
                "red, green, blue. make that a list. more text.",
                "- red\n- green\n- blue\nmore text.",
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(&apply(input), expected, "input: {input:?}");
        }
    }
}
