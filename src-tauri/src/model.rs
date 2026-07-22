//! Model presence/verify, first-run download (resumable), and sideload.
//! This is the ONLY network code in the app.

use crate::types::*;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// One downloadable model SKU. ASR SKUs are `.tar.bz2` archives (4-file sherpa
/// layout, `ModelFiles`); LLM SKUs are a single `.gguf` file. `kind` selects the
/// download/install/sideload branch — everything else (`present`/`verify_dir`)
/// is generic over `files`.
pub struct ModelSpec {
    pub id: &'static str,
    pub display: &'static str,
    pub dir_name: &'static str,
    /// Extracted on-disk total; the download tarball is slightly smaller.
    pub size_mb: u64,
    pub url: &'static str,
    /// SETUP card line-2 fragment ("ENGLISH" / "25 LANGUAGES · AUTO-DETECT").
    pub langs: &'static str,
    /// ASR (tar.bz2, sherpa) vs LLM (single .gguf, llama.cpp).
    pub kind: ModelKind,
    /// (filename, expected sha256). `None` = presence + non-empty only.
    files: &'static [(&'static str, Option<&'static str>)],
}

pub const DEFAULT_MODEL_ID: &str = "parakeet-tdt-0.6b-v2-int8";

/// Reformat LLM SKU ids — auto-picked by the GPU gate (gpu.rs), never a stored
/// user selection. 3B on a capable dGPU, 1.5B CPU fallback.
pub const REFORMAT_3B_ID: &str = "dictum-reformat-3b";
pub const REFORMAT_1_5B_ID: &str = "dictum-reformat-1.5b";

pub const MODELS: &[ModelSpec] = &[
    ModelSpec {
        id: "parakeet-tdt-0.6b-v2-int8",
        display: "PARAKEET-TDT 0.6B V2 INT8",
        dir_name: "sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8",
        size_mb: 630,
        url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8.tar.bz2",
        langs: "ENGLISH",
        kind: ModelKind::Asr,
        files: &[
            ("encoder.int8.onnx", Some("a32b12d17bbbc309d0686fbbcc2987b5e9b8333a7da83fa6b089f0a2acd651ab")),
            ("decoder.int8.onnx", Some("b6bb64963457237b900e496ee9994b59294526439fbcc1fecf705b31a15c6b4e")),
            ("joiner.int8.onnx", Some("7946164367946e7f9f29a122407c3252b680dbae9a51343eb2488d057c3c43d2")),
            ("tokens.txt", None),
        ],
    },
    ModelSpec {
        id: "parakeet-tdt-0.6b-v3-int8",
        display: "PARAKEET-TDT 0.6B V3 INT8",
        dir_name: "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8",
        size_mb: 641,
        url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2",
        langs: "25 LANGUAGES · AUTO-DETECT",
        kind: ModelKind::Asr,
        // Hashes computed from the k2-fsa release archive on 2026-07-21 via the
        // app's own sha256 path.
        files: &[
            ("encoder.int8.onnx", Some("acfc2b4456377e15d04f0243af540b7fe7c992f8d898d751cf134c3a55fd2247")),
            ("decoder.int8.onnx", Some("179e50c43d1a9de79c8a24149a2f9bac6eb5981823f2a2ed88d655b24248db4e")),
            ("joiner.int8.onnx", Some("3164c13fc2821009440d20fcb5fdc78bff28b4db2f8d0f0b329101719c0948b3")),
            ("tokens.txt", None),
        ],
    },
    // --- Reformat LLM SKUs: single .gguf files, not archives. -----------------
    ModelSpec {
        id: REFORMAT_3B_ID,
        display: "DICTUM REFORMAT 3B",
        dir_name: "dictum-reformat-3b",
        size_mb: 1930,
        url: "https://github.com/thomasbunch/dictum/releases/download/v0.3.0/dictum-reformat-3b-Q4_K_M.gguf",
        langs: "3B · Q4_K_M · GPU (4GB+ VRAM)",
        kind: ModelKind::Llm,
        files: &[("dictum-reformat-3b-Q4_K_M.gguf", Some("ddd7a3ecfbe7f4497f3235305570f64d78d72f581eae9d2f829786983021bc87"))],
    },
    ModelSpec {
        id: REFORMAT_1_5B_ID,
        display: "DICTUM REFORMAT 1.5B",
        dir_name: "dictum-reformat-1.5b",
        size_mb: 986,
        url: "https://github.com/thomasbunch/dictum/releases/download/v0.3.0/dictum-reformat-1.5b-Q4_K_M.gguf",
        langs: "1.5B · Q4_K_M · CPU",
        kind: ModelKind::Llm,
        files: &[("dictum-reformat-1.5b-Q4_K_M.gguf", Some("ee87905270eb92b2ec00ed6536241dd1553caff4e2f7f8c6ea192faccaba2d72"))],
    },
];

/// Spec for a config model id. Unknown ids (config written by a newer version)
/// fall back to the default model rather than panicking.
pub fn spec(id: &str) -> &'static ModelSpec {
    MODELS.iter().find(|m| m.id == id).unwrap_or(&MODELS[0])
}

/// Reformat (LLM) spec by id. Unknown/missing ids fall back to the 1.5B CPU SKU
/// — NOT to an ASR spec (spec()'s fallback would resolve a bad id to a recognizer).
pub fn reformat_spec(id: &str) -> &'static ModelSpec {
    MODELS
        .iter()
        .find(|m| m.id == id && m.kind == ModelKind::Llm)
        .unwrap_or_else(|| MODELS.iter().find(|m| m.id == REFORMAT_1_5B_ID).unwrap())
}

/// The reformat SKU the GPU gate picks: 3B on a capable dGPU, else 1.5B CPU.
pub fn reformat_id_for_gpu(offer_gpu_3b: bool) -> &'static str {
    if offer_gpu_3b {
        REFORMAT_3B_ID
    } else {
        REFORMAT_1_5B_ID
    }
}

pub fn model_dir(spec: &ModelSpec) -> PathBuf {
    models_dir().join(spec.dir_name)
}

/// The single file of a one-file (LLM/GGUF) SKU, inside its `dir_name` folder.
pub fn single_file_path(spec: &ModelSpec) -> PathBuf {
    model_dir(spec).join(spec.files[0].0)
}

/// Cheap presence check: every listed file exists and is non-empty. No SHA256
/// (unlike `present()`), so it's safe to call on the coordinator thread each
/// take to gate whether to attempt a reformat.
pub fn files_present(spec: &ModelSpec) -> bool {
    let dir = model_dir(spec);
    spec.files
        .iter()
        .all(|(name, _)| fs::metadata(dir.join(name)).map(|m| m.len() > 0).unwrap_or(false))
}

/// Absolute paths to the four files the recognizer needs.
pub struct ModelFiles {
    pub encoder: PathBuf,
    pub decoder: PathBuf,
    pub joiner: PathBuf,
    pub tokens: PathBuf,
}

impl ModelFiles {
    pub fn all_present(&self) -> bool {
        [&self.encoder, &self.decoder, &self.joiner, &self.tokens]
            .iter()
            .all(|p| p.exists())
    }
}

pub fn model_files(spec: &ModelSpec) -> ModelFiles {
    let d = model_dir(spec);
    ModelFiles {
        encoder: d.join("encoder.int8.onnx"),
        decoder: d.join("decoder.int8.onnx"),
        joiner: d.join("joiner.int8.onnx"),
        tokens: d.join("tokens.txt"),
    }
}

pub fn check(spec: &'static ModelSpec) -> ModelInfo {
    ModelInfo {
        id: spec.id.into(),
        display: spec.display.into(),
        present: present(spec),
        size_mb: spec.size_mb,
        langs: spec.langs.into(),
        kind: spec.kind,
    }
}

/// A hand-dropped `.tar.bz2` in models_dir() (sideload), if any. Excludes the
/// in-progress download partial.
pub fn find_dropped_archive() -> Option<PathBuf> {
    let dir = models_dir();
    fs::read_dir(&dir).ok()?.flatten().find_map(|e| {
        let p = e.path();
        let name = p.file_name()?.to_string_lossy().to_ascii_lowercase();
        (name.ends_with(".tar.bz2") && name != "download.partial").then_some(p)
    })
}

/// Download (resumable) then extract+verify+install. Progress on the callback.
/// A partial download is kept for resume; a completed-but-corrupt archive is
/// discarded (resume would loop forever on the same bad bytes).
pub fn download(spec: &'static ModelSpec, progress: impl Fn(DownloadProgress)) {
    if let Err(e) = download_inner(spec, &progress) {
        progress(DownloadProgress::Failed { error: e });
    }
}

fn download_inner(spec: &'static ModelSpec, progress: &impl Fn(DownloadProgress)) -> Result<(), String> {
    fs::create_dir_all(models_dir()).map_err(|e| e.to_string())?;
    let partial = models_dir().join(format!("{}.partial", spec.id));

    let resume_from = fs::metadata(&partial).map(|m| m.len()).unwrap_or(0);
    let mut req = ureq::get(spec.url);
    if resume_from > 0 {
        req = req.set("Range", &format!("bytes={}-", resume_from));
    }
    let resp = req.call().map_err(|e| e.to_string())?;

    let status = resp.status();
    let start = write_start(status, resume_from);
    let remaining: u64 = resp
        .header("Content-Length")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let total = if status == 206 {
        resp.header("Content-Range")
            .and_then(parse_total)
            .unwrap_or(start + remaining)
    } else {
        remaining
    };

    let mut file = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(&partial)
        .map_err(|e| e.to_string())?;
    if start > 0 {
        file.seek(SeekFrom::Start(start)).map_err(|e| e.to_string())?;
    } else {
        file.set_len(0).map_err(|e| e.to_string())?;
    }

    let mut reader = resp.into_reader();
    let mut buf = vec![0u8; 1 << 16];
    let mut done = start;
    let mut last_pct = u8::MAX;
    loop {
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        let p = pct(done, total);
        if p != last_pct {
            last_pct = p;
            progress(DownloadProgress::Progress {
                pct: p,
                mb_done: done >> 20,
                mb_total: total >> 20,
            });
        }
    }
    file.flush().map_err(|e| e.to_string())?;
    drop(file);

    progress(DownloadProgress::Verifying);
    // ASR = tar.bz2 archive (checksum decides which SKU); LLM = one .gguf we
    // already know the SKU of (download() took the spec).
    let installed = match spec.kind {
        ModelKind::Asr => install_from_archive(&partial).map(|_| ()),
        ModelKind::Llm => install_single_file(spec, &partial),
    };
    match installed {
        Ok(()) => {
            fs::remove_file(&partial).ok(); // no-op for LLM (partial was renamed)
            progress(DownloadProgress::Done);
            Ok(())
        }
        Err(e) => {
            // Download completed but verification failed — resuming re-serves the
            // same bytes, so start fresh next time.
            fs::remove_file(&partial).ok();
            Err(e)
        }
    }
}

/// Install a downloaded single-file (GGUF) SKU: verify its sha256, then move the
/// partial into `model_dir(spec)/<filename>` and write the `.verified` marker.
/// The SKU is known (download took the spec), so no checksum-identity search.
fn install_single_file(spec: &'static ModelSpec, partial: &Path) -> Result<(), String> {
    let (name, hash) = spec.files[0];
    if let Some(expected) = hash {
        let got = sha256_file(partial).map_err(|e| e.to_string())?;
        if !got.eq_ignore_ascii_case(expected) {
            return Err(format!("checksum mismatch for {name}"));
        }
    }
    let dest_dir = model_dir(spec);
    fs::create_dir_all(&dest_dir).map_err(|e| e.to_string())?;
    let dest = dest_dir.join(name);
    fs::remove_file(&dest).ok();
    fs::rename(partial, &dest).map_err(|e| e.to_string())?;
    let _ = fs::write(dest_dir.join(".verified"), b"ok");
    Ok(())
}

/// Offline sideload for LLM SKUs (PLAN §6 "same offline sideload path"): install
/// any hand-dropped `dictum-reformat-*.gguf` sitting loose in models_dir() into
/// its SKU folder. Filename identifies the SKU; sha256 must match. Cheap no-op
/// when nothing is dropped. Called once at boot on a background thread — it only
/// installs the file, never loads the model (reformat stays lazy).
// ponytail: filename-keyed match (not checksum-search like archives) — a mislabeled
// gguf is caught by the sha256 verify below, which is the real gate.
pub fn sideload_reformat_gguf() {
    let Ok(entries) = fs::read_dir(models_dir()) else { return };
    for e in entries.flatten() {
        let p = e.path();
        let is_gguf = p.extension().and_then(|x| x.to_str()).is_some_and(|x| x.eq_ignore_ascii_case("gguf"));
        if !is_gguf {
            continue;
        }
        let fname = match p.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let Some(spec) = MODELS.iter().find(|m| m.kind == ModelKind::Llm && m.files[0].0 == fname) else { continue };
        if files_present(spec) {
            continue; // already installed
        }
        if let Some(expected) = spec.files[0].1 {
            match sha256_file(&p) {
                Ok(got) if got.eq_ignore_ascii_case(expected) => {}
                _ => continue, // corrupt / wrong file — leave it alone
            }
        }
        let dest_dir = model_dir(spec);
        if fs::create_dir_all(&dest_dir).is_err() {
            continue;
        }
        if fs::rename(&p, dest_dir.join(spec.files[0].0)).is_ok() {
            let _ = fs::write(dest_dir.join(".verified"), b"ok");
        }
    }
}

/// Extract a `.tar.bz2`, verify SHA256s, atomically install. The archive
/// decides which SKU it is: whichever spec's hashes match gets the install —
/// so a sideloaded v3 lands in the v3 slot no matter the filename. Returns the
/// installed spec. Shared by the download and sideload paths.
pub fn install_from_archive(archive: &Path) -> Result<&'static ModelSpec, String> {
    let staging = models_dir().join(".staging");
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).map_err(|e| e.to_string())?;

    let f = fs::File::open(archive).map_err(|e| e.to_string())?;
    let bz = bzip2::read::BzDecoder::new(f);
    tar::Archive::new(bz)
        .unpack(&staging)
        .map_err(|e| format!("extract failed: {e}"))?;

    let staged = resolve_staged(&staging).ok_or("model files not found in archive")?;
    let spec = MODELS
        .iter()
        .find(|m| verify_dir(m, &staged).is_ok())
        .ok_or("archive does not match any known model (checksum mismatch)")?;

    let dest = model_dir(spec);
    if dest.exists() {
        fs::remove_dir_all(&dest).map_err(|e| e.to_string())?;
    }
    fs::rename(&staged, &dest).map_err(|e| e.to_string())?;
    let _ = fs::write(dest.join(".verified"), b"ok");
    let _ = fs::remove_dir_all(&staging);
    Ok(spec)
}

/// The archive's top entry may be a model folder of any name, or the files
/// flat at the root — find where encoder.int8.onnx lives.
fn resolve_staged(staging: &Path) -> Option<PathBuf> {
    if staging.join("encoder.int8.onnx").exists() {
        return Some(staging.to_path_buf());
    }
    fs::read_dir(staging).ok()?.flatten().find_map(|e| {
        let p = e.path();
        (p.is_dir() && p.join("encoder.int8.onnx").exists()).then_some(p)
    })
}

/// Presence + non-empty for all files; SHA256 for the .onnx files, cached via a
/// `.verified` marker so startup does not re-hash 660 MB every launch.
fn present(spec: &ModelSpec) -> bool {
    let dir = model_dir(spec);
    for (name, _) in spec.files {
        match fs::metadata(dir.join(name)) {
            Ok(m) if m.len() > 0 => {}
            _ => return false,
        }
    }
    if dir.join(".verified").exists() {
        return true;
    }
    match verify_dir(spec, &dir) {
        Ok(()) => {
            let _ = fs::write(dir.join(".verified"), b"ok");
            true
        }
        Err(_) => false,
    }
}

fn verify_dir(spec: &ModelSpec, dir: &Path) -> Result<(), String> {
    for (name, hash) in spec.files {
        let p = dir.join(name);
        let meta = fs::metadata(&p).map_err(|_| format!("missing {name}"))?;
        if meta.len() == 0 {
            return Err(format!("empty {name}"));
        }
        if let Some(expected) = hash {
            let got = sha256_file(&p).map_err(|e| e.to_string())?;
            if !got.eq_ignore_ascii_case(expected) {
                return Err(format!("checksum mismatch for {name}"));
            }
        }
    }
    Ok(())
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(to_hex(&hasher.finalize()))
}

fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    to_hex(&h.finalize())
}

fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Where to start writing the partial: append only if the server honored our
/// Range request (206); a plain 200 means restart from zero.
fn write_start(status: u16, resume_from: u64) -> u64 {
    if status == 206 {
        resume_from
    } else {
        0
    }
}

fn pct(done: u64, total: u64) -> u8 {
    if total == 0 {
        0
    } else {
        ((done * 100 / total).min(100)) as u8
    }
}

/// Parse the total length out of a `Content-Range: bytes X-Y/Z` header.
fn parse_total(cr: &str) -> Option<u64> {
    cr.rsplit('/').next()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn resume_write_start() {
        assert_eq!(write_start(206, 1000), 1000); // range honored -> append
        assert_eq!(write_start(200, 1000), 0); // range ignored -> restart
        assert_eq!(write_start(206, 0), 0);
    }

    #[test]
    fn pct_clamps() {
        assert_eq!(pct(0, 0), 0);
        assert_eq!(pct(0, 100), 0);
        assert_eq!(pct(50, 100), 50);
        assert_eq!(pct(200, 100), 100); // never exceeds 100
    }

    #[test]
    fn content_range_total() {
        assert_eq!(parse_total("bytes 100-999/1000"), Some(1000));
        assert_eq!(parse_total("bytes 0-0/*"), None);
        assert_eq!(parse_total("garbage"), None);
    }

    #[test]
    fn expected_files() {
        for m in MODELS.iter().filter(|m| m.kind == ModelKind::Asr) {
            let names: Vec<&str> = m.files.iter().map(|(n, _)| *n).collect();
            assert_eq!(
                names,
                ["encoder.int8.onnx", "decoder.int8.onnx", "joiner.int8.onnx", "tokens.txt"],
                "{}", m.id
            );
            assert_eq!(m.files.iter().filter(|(_, h)| h.is_some()).count(), 3, "{}", m.id);
            assert!(m.files.last().unwrap().1.is_none()); // tokens.txt: presence-only
        }
    }

    #[test]
    fn llm_skus_are_single_hashed_gguf() {
        for m in MODELS.iter().filter(|m| m.kind == ModelKind::Llm) {
            assert_eq!(m.files.len(), 1, "{}: LLM SKU is one file", m.id);
            let (name, hash) = m.files[0];
            assert!(name.ends_with(".gguf"), "{}: {name} is not a .gguf", m.id);
            assert!(hash.is_some(), "{}: GGUF must carry a sha256", m.id);
            // single_file_path lands inside the SKU subfolder (present()/verify_dir
            // assume models_dir()/dir_name/<file>).
            assert!(single_file_path(m).ends_with(name));
        }
        // The GPU gate resolves to real, distinct LLM specs.
        assert_eq!(reformat_id_for_gpu(true), REFORMAT_3B_ID);
        assert_eq!(reformat_id_for_gpu(false), REFORMAT_1_5B_ID);
        assert_eq!(reformat_spec(REFORMAT_3B_ID).kind, ModelKind::Llm);
        // A bad/unknown reformat id falls back to the CPU SKU, never to ASR.
        assert_eq!(reformat_spec("bogus").id, REFORMAT_1_5B_ID);
    }

    #[test]
    fn registry_lookup_and_fallback() {
        assert_eq!(spec(DEFAULT_MODEL_ID).id, DEFAULT_MODEL_ID);
        assert_eq!(spec("parakeet-tdt-0.6b-v3-int8").langs, "25 LANGUAGES · AUTO-DETECT");
        // A config written by a newer app version must not panic — fall back.
        assert_eq!(spec("some-future-model").id, DEFAULT_MODEL_ID);
        // ids and dirs are unique.
        let mut ids: Vec<_> = MODELS.iter().map(|m| m.id).collect();
        ids.dedup();
        assert_eq!(ids.len(), MODELS.len());
    }
}
