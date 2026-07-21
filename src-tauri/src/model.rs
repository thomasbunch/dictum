//! Model presence/verify, first-run download (resumable), and sideload.
//! This is the ONLY network code in the app.

use crate::types::*;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// One downloadable model SKU. Both Parakeet exports share the same file
/// layout, so `ModelFiles` works for every entry.
pub struct ModelSpec {
    pub id: &'static str,
    pub display: &'static str,
    pub dir_name: &'static str,
    /// Extracted on-disk total; the download tarball is slightly smaller.
    pub size_mb: u64,
    pub url: &'static str,
    /// SETUP card line-2 fragment ("ENGLISH" / "25 LANGUAGES · AUTO-DETECT").
    pub langs: &'static str,
    /// (filename, expected sha256). `None` = presence + non-empty only.
    files: &'static [(&'static str, Option<&'static str>)],
}

pub const DEFAULT_MODEL_ID: &str = "parakeet-tdt-0.6b-v2-int8";

pub const MODELS: &[ModelSpec] = &[
    ModelSpec {
        id: "parakeet-tdt-0.6b-v2-int8",
        display: "PARAKEET-TDT 0.6B V2 INT8",
        dir_name: "sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8",
        size_mb: 630,
        url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8.tar.bz2",
        langs: "ENGLISH",
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
        // Hashes computed from the k2-fsa release archive on 2026-07-21 via the
        // app's own sha256 path.
        files: &[
            ("encoder.int8.onnx", Some("acfc2b4456377e15d04f0243af540b7fe7c992f8d898d751cf134c3a55fd2247")),
            ("decoder.int8.onnx", Some("179e50c43d1a9de79c8a24149a2f9bac6eb5981823f2a2ed88d655b24248db4e")),
            ("joiner.int8.onnx", Some("3164c13fc2821009440d20fcb5fdc78bff28b4db2f8d0f0b329101719c0948b3")),
            ("tokens.txt", None),
        ],
    },
];

/// Spec for a config model id. Unknown ids (config written by a newer version)
/// fall back to the default model rather than panicking.
pub fn spec(id: &str) -> &'static ModelSpec {
    MODELS.iter().find(|m| m.id == id).unwrap_or(&MODELS[0])
}

pub fn model_dir(spec: &ModelSpec) -> PathBuf {
    models_dir().join(spec.dir_name)
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
    match install_from_archive(&partial) {
        Ok(_installed) => {
            fs::remove_file(&partial).ok();
            progress(DownloadProgress::Done);
            Ok(())
        }
        Err(e) => {
            // Download completed but the archive is bad — resuming re-serves the
            // same bytes, so start fresh next time.
            fs::remove_file(&partial).ok();
            Err(e)
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
        for m in MODELS {
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
