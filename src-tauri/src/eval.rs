//! Coding-term accuracy harness: record real takes, then score the shipped
//! pipeline against them.
//!
//! Why this exists: before it, Dictum could not measure its own coding-term error
//! rate at all. Every accuracy figure in the 0.5 research traces back to one
//! hand-authored confusion list with no audio behind it, so every change to the
//! term table, the hotword list or the guardrail was unfalsifiable.
//!
//! Two entry points, both `#[ignore]`d because one needs a microphone and the
//! other needs ~600 MB of model:
//!
//!   cargo test --release -- --ignored --nocapture eval_record
//!   cargo test --release -- --ignored --nocapture eval_score
//!
//! Scoring is **keyword F-score, not WER**. This is the one methodological point
//! worth defending: contextual biasing reliably moves keyword F-score while
//! leaving WER flat or slightly worse, because swapping one short word for
//! another barely registers across a sentence. A/B this on WER and the honest
//! conclusion is "nothing works".
//!
//! The harness calls `crate::deterministic_text` — the same function
//! `RealEffects::apply_replacements` calls — rather than re-implementing the
//! chain. A harness with its own copy of the ordering measures the copy.

use crate::filetag;
use crate::terms;
use crate::types::Config;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// --- corpus ---------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Split {
    /// Out-of-repo library / tool / API vocabulary.
    Lib,
    /// Repo identifier, reachable through the spoken symbol cue.
    Repo,
    /// Identifier the precision gating deliberately refuses to glue together —
    /// the spoken form must SURVIVE, so a rewrite here is the failure.
    Gated,
    /// No target terms. Scores corruption, not accuracy.
    Prose,
}

impl Split {
    fn parse(s: &str) -> Option<Split> {
        Some(match s {
            "lib" => Split::Lib,
            "repo" => Split::Repo,
            "gated" => Split::Gated,
            "prose" => Split::Prose,
            _ => return None,
        })
    }
    fn name(self) -> &'static str {
        match self {
            Split::Lib => "lib",
            Split::Repo => "repo",
            Split::Gated => "gated",
            Split::Prose => "prose",
        }
    }
}

struct Row {
    id: String,
    split: Split,
    keywords: Vec<String>,
    text: String,
}

fn eval_dir() -> PathBuf {
    // .parent() rather than join("..") so printed paths read cleanly, and rather
    // than canonicalize(), which hands back a verbatim (extended-length) path on
    // Windows that is harder to read than the thing it fixed.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("manifest dir always has a parent")
        .join("eval")
}

fn take_path(id: &str) -> PathBuf {
    eval_dir().join("takes").join(format!("{id}.wav"))
}

fn load_corpus() -> Vec<Row> {
    let p = eval_dir().join("corpus.tsv");
    let text = std::fs::read_to_string(&p)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", p.display()));
    let mut out = Vec::new();
    for line in text.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() != 4 || f[0] == "id" {
            continue; // header, or a row someone broke — skip loudly below
        }
        let Some(split) = Split::parse(f[1]) else {
            panic!("row {}: unknown split {:?}", f[0], f[1]);
        };
        out.push(Row {
            id: f[0].to_string(),
            split,
            keywords: f[2].split(',').filter(|k| !k.trim().is_empty()).map(|k| k.trim().to_string()).collect(),
            text: f[3].to_string(),
        });
    }
    assert!(!out.is_empty(), "corpus is empty");
    out
}

// --- wav ------------------------------------------------------------------

fn write_wav(path: &Path, samples: &[f32]) {
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).expect("create wav");
    for s in samples {
        // Clamp before scaling: a resampler overshoot past 1.0 wraps otherwise.
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        w.write_sample(v).expect("write sample");
    }
    w.finalize().expect("finalize wav");
}

fn read_wav(path: &Path) -> Option<Vec<f32>> {
    let mut r = hound::WavReader::open(path).ok()?;
    if r.spec().sample_rate != 16_000 || r.spec().channels != 1 {
        eprintln!("skipping {}: expected 16 kHz mono", path.display());
        return None;
    }
    Some(r.samples::<i16>().filter_map(Result::ok).map(|s| s as f32 / 32768.0).collect())
}

// --- keyword scoring ------------------------------------------------------

/// Count word-bounded occurrences of `needle` in `hay`.
///
/// Hand-rolled rather than regex because the keywords include `Node.js`, `C++`
/// and `CI/CD`: `\b` is defined against word characters, so it behaves wrongly
/// (or refuses to match at all) when the term starts or ends with punctuation.
/// The rule here is simply "not glued to an alphanumeric on either side", which
/// is what the intent was all along.
fn count_occurrences(hay: &str, needle: &str, case_sensitive: bool) -> usize {
    if needle.is_empty() {
        return 0;
    }
    let (h, n) = if case_sensitive {
        (hay.to_string(), needle.to_string())
    } else {
        (hay.to_lowercase(), needle.to_lowercase())
    };
    let hb = h.as_bytes();
    let glued = |i: usize| -> bool {
        // Byte indexing is safe for the boundary test: a UTF-8 continuation byte
        // is never an ASCII alphanumeric, so a multi-byte char reads as "not
        // glued", which is the conservative answer.
        i < hb.len() && (hb[i].is_ascii_alphanumeric() || hb[i] == b'_')
    };
    let mut count = 0;
    let mut from = 0;
    while let Some(rel) = h[from..].find(&n) {
        let start = from + rel;
        let end = start + n.len();
        let before_ok = start == 0 || !glued(start - 1);
        if before_ok && !glued(end) {
            count += 1;
        }
        // Advance one byte past the start so overlapping hits are still found;
        // step to a char boundary so slicing stays valid.
        from = start + 1;
        while from < h.len() && !h.is_char_boundary(from) {
            from += 1;
        }
        if from >= h.len() {
            break;
        }
    }
    count
}

#[derive(Default, Clone, Copy)]
struct Tally {
    tp: usize,
    fp: usize,
    fna: usize,
}

impl Tally {
    fn add(&mut self, o: Tally) {
        self.tp += o.tp;
        self.fp += o.fp;
        self.fna += o.fna;
    }
    fn precision(&self) -> f64 {
        let d = self.tp + self.fp;
        if d == 0 { 1.0 } else { self.tp as f64 / d as f64 }
    }
    fn recall(&self) -> f64 {
        let d = self.tp + self.fna;
        if d == 0 { 1.0 } else { self.tp as f64 / d as f64 }
    }
    fn f1(&self) -> f64 {
        let (p, r) = (self.precision(), self.recall());
        if p + r == 0.0 { 0.0 } else { 2.0 * p * r / (p + r) }
    }
}

/// Keyword tally for one take: expected occurrences come from the reference,
/// produced occurrences from the pipeline's output.
fn score_row(reference: &str, hypothesis: &str, keywords: &[String], case_sensitive: bool) -> Tally {
    let mut t = Tally::default();
    for k in keywords {
        let want = count_occurrences(reference, k, case_sensitive).max(1);
        let got = count_occurrences(hypothesis, k, case_sensitive);
        t.tp += want.min(got);
        t.fna += want.saturating_sub(got);
        t.fp += got.saturating_sub(want);
    }
    t
}

/// Tokens in `after` that differ from `before`, and the token count of `before`.
///
/// Used only on prose rows, and against the RAW condition rather than the
/// reference: that isolates what the post-ASR pipeline changed from what the
/// recognizer misheard. The pipeline should be inert on prose, so every
/// difference here is a wrong rewrite.
fn changed_tokens(before: &str, after: &str) -> (usize, usize) {
    // Token-level Levenshtein, not a positional compare. A rule that glues two
    // words into one ("engine x" -> "nginx") shifts every token after it, and a
    // positional diff would score the whole tail as corrupted. That inflates the
    // exact number someone would use to wave away a real regression.
    let a: Vec<&str> = before.split_whitespace().collect();
    let b: Vec<&str> = after.split_whitespace().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    (prev[b.len()], a.len().max(1))
}

// --- conditions -----------------------------------------------------------

/// One column of the ablation. The point is not "is Dictum accurate" but "which
/// change bought what" — a single default-on number cannot answer that.
struct Condition {
    id: &'static str,
    note: &'static str,
    /// Which ASR pass feeds it (see `Pass`).
    pass: Pass,
    coding_terms: bool,
    repo: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Pass {
    /// Greedy, no hotwords — what 0.4 shipped.
    Greedy,
    /// Beam search biased on the built-in term list.
    BiasTerms,
    /// Beam search biased on built-in terms plus the repo symbol harvest.
    BiasRepo,
}

const CONDITIONS: &[Condition] = &[
    Condition { id: "raw", note: "0.4 default: greedy, no post-processing", pass: Pass::Greedy, coding_terms: false, repo: false },
    Condition { id: "terms", note: "built-in term rules only", pass: Pass::Greedy, coding_terms: true, repo: false },
    Condition { id: "bias", note: "ASR biasing only", pass: Pass::BiasTerms, coding_terms: false, repo: false },
    Condition { id: "both", note: "0.5 default: biasing + term rules", pass: Pass::BiasTerms, coding_terms: true, repo: false },
    Condition { id: "full", note: "+ project roots and symbol cue", pass: Pass::BiasRepo, coding_terms: true, repo: true },
];

fn config_for(c: &Condition, repo_root: &str) -> Config {
    let mut cfg = Config::default();
    cfg.coding_terms = c.coding_terms;
    cfg.asr_biasing = !matches!(c.pass, Pass::Greedy);
    if c.repo {
        cfg.project_roots = vec![repo_root.to_string()];
        cfg.symbol_cue = "symbol".into();
    }
    cfg
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- unit tests: the scorer itself, no audio and no model needed -------

    #[test]
    fn counts_only_word_bounded_occurrences() {
        assert_eq!(count_occurrences("use the api now", "api", true), 1);
        // the crown jewel: every term here is a substring of a real word
        assert_eq!(count_occurrences("the apiary was rapid", "api", true), 0);
        assert_eq!(count_occurrences("mysql and postgresql", "sql", true), 0);
        assert_eq!(count_occurrences("call kubectl twice, kubectl again", "kubectl", true), 2);
    }

    #[test]
    fn counts_terms_that_start_or_end_in_punctuation() {
        // `\b` would refuse these, which is why the boundary test is hand-rolled.
        assert_eq!(count_occurrences("we build with C++ here", "C++", true), 1);
        assert_eq!(count_occurrences("install Node.js first", "Node.js", true), 1);
        assert_eq!(count_occurrences("the .NET runtime", ".NET", true), 1);
        assert_eq!(count_occurrences("our CI/CD pipeline", "CI/CD", true), 1);
        // still bounded: a longer token must not match
        assert_eq!(count_occurrences("Node.json is not a thing", "Node.js", true), 0);
    }

    #[test]
    fn case_sensitivity_separates_misheard_from_miscased() {
        // This is the whole reason both scores are reported: "heard it but cased
        // it wrong" is a replacement-table bug, "did not hear it" is an ASR bug.
        assert_eq!(count_occurrences("the json payload", "JSON", true), 0);
        assert_eq!(count_occurrences("the json payload", "JSON", false), 1);
    }

    #[test]
    fn a_missing_keyword_is_a_false_negative_not_a_false_positive() {
        let t = score_row("deploy nginx now", "deploy engine x now", &["nginx".into()], true);
        assert_eq!((t.tp, t.fp, t.fna), (0, 0, 1));
        assert_eq!(t.recall(), 0.0);
        // Precision is untouched: the pipeline did not invent anything.
        assert_eq!(t.precision(), 1.0);
    }

    #[test]
    fn over_firing_is_a_false_positive() {
        let t = score_row("deploy nginx now", "deploy nginx nginx now", &["nginx".into()], true);
        assert_eq!((t.tp, t.fp, t.fna), (1, 1, 0));
        assert_eq!(t.recall(), 1.0);
        assert_eq!(t.precision(), 0.5);
    }

    #[test]
    fn a_perfect_take_scores_one() {
        let t = score_row("run kubectl on the Kubernetes box", "Run kubectl on the Kubernetes box.", &["kubectl".into(), "Kubernetes".into()], true);
        assert_eq!((t.tp, t.fp, t.fna), (2, 0, 0));
        assert_eq!(t.f1(), 1.0);
    }

    /// A reference that does not itself contain the keyword still expects it
    /// once — that is the point of the `repo` and `lib` splits, where the SPOKEN
    /// form differs from the term that must be printed.
    #[test]
    fn expected_count_floors_at_one() {
        let t = score_row("deploy engine x now", "deploy nginx now", &["nginx".into()], true);
        assert_eq!((t.tp, t.fp, t.fna), (1, 0, 0));
    }

    #[test]
    fn prose_corruption_counts_tokens_the_pipeline_moved() {
        assert_eq!(changed_tokens("i flew to tokyo", "i flew to tokyo"), (0, 4));
        assert_eq!(changed_tokens("i flew to tokyo", "i flew to tokio"), (1, 4));
        // A merge is ONE substitution plus ONE deletion, not a shifted tail.
        assert_eq!(changed_tokens("the engine x axis", "the nginx axis"), (2, 4));
        // An insertion early on must not score the rest of the line as changed.
        assert_eq!(changed_tokens("keep the scope small", "keep the whole scope small"), (1, 4));
    }

    #[test]
    fn corpus_is_well_formed() {
        let rows = load_corpus();
        assert!(rows.len() > 100, "corpus shrank unexpectedly: {}", rows.len());
        let mut seen = std::collections::HashSet::new();
        for r in &rows {
            assert!(seen.insert(r.id.clone()), "duplicate id {}", r.id);
            assert!(!r.text.trim().is_empty(), "{}: empty text", r.id);
            match r.split {
                Split::Prose => assert!(r.keywords.is_empty(), "{}: prose rows score corruption, not keywords", r.id),
                _ => assert!(!r.keywords.is_empty(), "{}: needs at least one keyword", r.id),
            }
            // A repo row is invoked by the cue; without it the pass is inert and
            // the row would silently measure nothing.
            if r.split == Split::Repo {
                assert!(r.text.contains("symbol "), "{}: repo rows must speak the cue word", r.id);
            }
        }
        // Every prose control that exists because a rule was demoted for
        // colliding with it. If someone re-adds the rule, the corpus still has
        // the sentence that catches it.
        let prose: String = rows.iter().filter(|r| r.split == Split::Prose).map(|r| r.text.clone()).collect::<Vec<_>>().join(" ");
        for hazard in ["tokyo", "quarks", "this error", "my pie", "jot", "onyx", "bit bucket", "standard in", "see make", "engine x-axis"] {
            assert!(prose.contains(hazard), "prose controls lost the {hazard:?} case");
        }
    }

    /// Fixture coverage, printed rather than asserted — the corpus is recorded
    /// incrementally and a partial run is still useful.
    #[test]
    fn eval_coverage() {
        let rows = load_corpus();
        let mut have = HashMap::new();
        let mut total = HashMap::new();
        for r in &rows {
            *total.entry(r.split).or_insert(0usize) += 1;
            if take_path(&r.id).exists() {
                *have.entry(r.split).or_insert(0usize) += 1;
            }
        }
        let n: usize = have.values().sum();
        eprintln!("=== EVAL COVERAGE: {n}/{} takes recorded ===", rows.len());
        for s in [Split::Lib, Split::Repo, Split::Gated, Split::Prose] {
            eprintln!(
                "  {:<6} {:>3}/{:<3}",
                s.name(),
                have.get(&s).copied().unwrap_or(0),
                total.get(&s).copied().unwrap_or(0)
            );
        }
        if n == 0 {
            eprintln!("\n  Nothing recorded yet. Run:");
            eprintln!("    cargo test --release -- --ignored --nocapture eval_record");
        }
    }

    // --- recorder ----------------------------------------------------------

    /// Prompt through the corpus and record each take from the default input
    /// device, at the same 16 kHz mono the pipeline decodes.
    ///
    /// Resumable: rows that already have audio are skipped, so this can be done
    /// in sittings. Set EVAL_REDO=1 to re-record everything, or EVAL_ONLY=lib to
    /// work one split at a time.
    #[test]
    #[ignore]
    fn eval_record() {
        use std::io::Write;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let rows = load_corpus();
        let redo = std::env::var("EVAL_REDO").is_ok();
        let only = std::env::var("EVAL_ONLY").ok();
        let todo: Vec<&Row> = rows
            .iter()
            .filter(|r| only.as_deref().is_none_or(|p| r.id.starts_with(p)))
            .filter(|r| redo || !take_path(&r.id).exists())
            .collect();

        if todo.is_empty() {
            eprintln!("Nothing to record — every selected take already has audio.");
            return;
        }
        eprintln!("=== RECORDING {} takes ===", todo.len());
        eprintln!("Read each line naturally. Do not read the punctuation aloud.");
        eprintln!("ENTER starts, ENTER stops. 's' skips, 'q' quits, 'r' re-records.\n");

        let mut done = 0usize;
        for (i, row) in todo.iter().enumerate() {
            loop {
                eprintln!("[{}/{}] {} ({})", i + 1, todo.len(), row.id, row.split.name());
                eprintln!("    {}", row.text);
                eprint!("    > ");
                let _ = std::io::stderr().flush();
                let mut cmd = String::new();
                if std::io::stdin().read_line(&mut cmd).is_err() {
                    return;
                }
                match cmd.trim() {
                    "q" => {
                        eprintln!("\nStopped. {done} recorded this session.");
                        return;
                    }
                    "s" => break,
                    _ => {}
                }

                // Open the real capture path so the fixtures carry this machine's
                // actual device, rate and downmix — not a synthesized signal.
                let h = match crate::audio::capture::open(None) {
                    Ok(h) => h,
                    Err(e) => {
                        eprintln!("    capture failed: {e}");
                        return;
                    }
                };
                let mut consumer = h.consumer;
                let channels = h.channels;
                let mut resampler = crate::audio::resample::Downsampler::new(h.native_rate);

                let stop = Arc::new(AtomicBool::new(false));
                let s2 = stop.clone();
                std::thread::spawn(move || {
                    let mut l = String::new();
                    let _ = std::io::stdin().read_line(&mut l);
                    s2.store(true, Ordering::Relaxed);
                });

                let mut out16: Vec<f32> = Vec::new();
                let (mut scratch, mut mono, mut chunk_out) = (Vec::new(), Vec::new(), Vec::new());
                eprintln!("    recording… ENTER to stop");
                while !stop.load(Ordering::Relaxed) {
                    let avail = consumer.slots();
                    let n = avail - avail % channels.max(1); // whole frames only
                    if n > 0 {
                        scratch.clear();
                        if let Ok(c) = consumer.read_chunk(n) {
                            let (a, b) = c.as_slices();
                            scratch.extend_from_slice(a);
                            scratch.extend_from_slice(b);
                            c.commit_all();
                        }
                        mono.clear();
                        crate::audio::resample::downmix_to_mono(&scratch, channels, &mut mono);
                        chunk_out.clear();
                        resampler.push(&mono, &mut chunk_out);
                        out16.extend_from_slice(&chunk_out);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                drop(h.stream); // stops capture
                // Flush the resampler's internal tail or the last fraction of a
                // second is silently clipped off every take.
                chunk_out.clear();
                resampler.finish(&mut chunk_out);
                out16.extend_from_slice(&chunk_out);

                let secs = out16.len() as f64 / 16_000.0;
                if out16.is_empty() {
                    eprintln!("    nothing captured — check the input device. Retrying.\n");
                    continue;
                }
                write_wav(&take_path(&row.id), &out16);
                eprint!("    {secs:.1}s saved. ENTER to keep, 'r' to re-record > ");
                let _ = std::io::stderr().flush();
                let mut again = String::new();
                let _ = std::io::stdin().read_line(&mut again);
                if again.trim() == "r" {
                    eprintln!();
                    continue;
                }
                if secs < 0.8 {
                    eprintln!("    NOTE: under a second — verify that one before trusting the score.");
                }
                done += 1;
                eprintln!();
                break;
            }
        }
        eprintln!("=== {done} takes recorded ===");
    }

    // --- scorer ------------------------------------------------------------

    /// Decode every recorded take under each condition and print the ablation.
    #[test]
    #[ignore]
    fn eval_score() {
        let rows = load_corpus();
        let loaded: Vec<(&Row, Vec<f32>)> = rows
            .iter()
            .filter_map(|r| read_wav(&take_path(&r.id)).map(|s| (r, s)))
            .collect();

        if loaded.is_empty() {
            panic!(
                "no recorded takes in {} — run eval_record first",
                eval_dir().join("takes").display()
            );
        }
        let repo_root = std::fs::canonicalize(format!("{}/..", env!("CARGO_MANIFEST_DIR")))
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let folder = Path::new(&repo_root).file_name().unwrap().to_string_lossy().into_owned();
        let title = format!("coordinator.rs — {folder} — Visual Studio Code");

        eprintln!("=== EVAL: {} of {} takes recorded ===", loaded.len(), rows.len());
        if loaded.len() * 4 < rows.len() {
            eprintln!("WARNING: under a quarter of the corpus is recorded. Treat these numbers");
            eprintln!("as directional only — a handful of takes cannot separate the conditions.");
        }

        let index = filetag::Index::build(&[repo_root.clone()]);
        let hw_terms = terms::join_hotwords(terms::hotwords());
        let hw_repo = {
            let mut e = terms::hotwords();
            e.extend(index.hotwords());
            terms::join_hotwords(e)
        };
        eprintln!("hotwords: {} built-in, {} with repo symbols", hw_terms.split('/').count(), hw_repo.split('/').count());

        // --- ASR passes. Two recognizers, three passes: the model is ~600 MB, so
        // load each once and decode every take through it rather than the reverse.
        let mut asr: HashMap<(Pass, &str), String> = HashMap::new();
        {
            let rec = crate::asr::EvalRecognizer::load(crate::model::DEFAULT_MODEL_ID, false)
                .expect("install the default ASR SKU first");
            for (r, s) in &loaded {
                asr.insert((Pass::Greedy, r.id.as_str()), rec.decode(s, ""));
            }
        }
        {
            let rec = crate::asr::EvalRecognizer::load(crate::model::DEFAULT_MODEL_ID, true)
                .expect("recognizer failed to load with biasing");
            assert!(rec.biased(), "biasing silently downgraded to greedy — bpe.vocab derivation failed");
            for (r, s) in &loaded {
                asr.insert((Pass::BiasTerms, r.id.as_str()), rec.decode(s, &hw_terms));
                asr.insert((Pass::BiasRepo, r.id.as_str()), rec.decode(s, &hw_repo));
            }
        }

        // --- post-process and score
        let mut report = String::from("condition\tsplit\ttakes\ttp\tfp\tfn\tprecision\trecall\tf1\tf1_nocase\n");
        eprintln!(
            "\n{:<8} {:<6} {:>5} {:>4} {:>4} {:>4}  {:>5} {:>5} {:>6} {:>8}",
            "cond", "split", "takes", "tp", "fp", "fn", "P", "R", "F1", "F1(case-)"
        );
        eprintln!("{}", "-".repeat(66));

        // Raw condition output per take, for the prose-corruption baseline.
        let mut raw_out: HashMap<&str, String> = HashMap::new();
        let raw_cfg = config_for(&CONDITIONS[0], &repo_root);
        for (r, _) in &loaded {
            let hyp = asr.get(&(Pass::Greedy, r.id.as_str())).cloned().unwrap_or_default();
            raw_out.insert(r.id.as_str(), crate::deterministic_text(&hyp, &raw_cfg, &index, Some(&title)));
        }

        for c in CONDITIONS {
            let cfg = config_for(c, &repo_root);
            let mut by_split: HashMap<Split, (Tally, Tally, usize)> = HashMap::new();
            let mut corrupt = (0usize, 0usize); // (changed tokens, total tokens) on prose

            for (r, _) in &loaded {
                let hyp = asr.get(&(c.pass, r.id.as_str())).cloned().unwrap_or_default();
                let out = crate::deterministic_text(&hyp, &cfg, &index, Some(&title));
                let e = by_split.entry(r.split).or_insert((Tally::default(), Tally::default(), 0));
                e.2 += 1;
                if r.split == Split::Prose {
                    let base = raw_out.get(r.id.as_str()).cloned().unwrap_or_default();
                    let (ch, tot) = changed_tokens(&base, &out);
                    corrupt.0 += ch;
                    corrupt.1 += tot;
                } else {
                    e.0.add(score_row(&r.text, &out, &r.keywords, true));
                    e.1.add(score_row(&r.text, &out, &r.keywords, false));
                }
            }

            for s in [Split::Lib, Split::Repo, Split::Gated] {
                let Some((exact, lenient, n)) = by_split.get(&s) else { continue };
                eprintln!(
                    "{:<8} {:<6} {:>5} {:>4} {:>4} {:>4}  {:>5.3} {:>5.3} {:>6.3} {:>8.3}",
                    c.id, s.name(), n, exact.tp, exact.fp, exact.fna,
                    exact.precision(), exact.recall(), exact.f1(), lenient.f1()
                );
                report.push_str(&format!(
                    "{}\t{}\t{}\t{}\t{}\t{}\t{:.4}\t{:.4}\t{:.4}\t{:.4}\n",
                    c.id, s.name(), n, exact.tp, exact.fp, exact.fna,
                    exact.precision(), exact.recall(), exact.f1(), lenient.f1()
                ));
            }
            if corrupt.1 > 0 {
                let per100 = corrupt.0 as f64 * 100.0 / corrupt.1 as f64;
                eprintln!(
                    "{:<8} {:<6} {:>5} {:>4} {:>4} {:>4}  wrong rewrites per 100 words: {:.2}",
                    c.id, "prose", by_split.get(&Split::Prose).map(|e| e.2).unwrap_or(0), "", "", "", per100
                );
                report.push_str(&format!("{}\tprose\t{}\t\t\t\t\t\t\t{:.4}\n", c.id, corrupt.1, per100));
            }
            eprintln!("{:<8} {}", "", c.note);
        }

        let out_path = eval_dir().join("results.tsv");
        let _ = std::fs::write(&out_path, &report);
        eprintln!("\nwritten: {}", out_path.display());
        eprintln!("\nRead the `bias` row against `raw` for what biasing alone bought, and");
        eprintln!("`terms` against `raw` for what the table bought. The prose line is the");
        eprintln!("cost side — a gain there is a regression.");
    }
}
