//! Model presence/verify, first-run download (resumable), and sideload.
//! This is the ONLY network code in the app.

use crate::types::*;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const MODEL_ID: &str = "parakeet-tdt-0.6b-v2-int8";
const MODEL_DISPLAY: &str = "PARAKEET-TDT 0.6B V2 INT8";
const DIR_NAME: &str = "sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8";
/// Extracted on-disk total (~630 MiB); download tarball is ~628 MiB.
const SIZE_MB: u64 = 630;
const URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8.tar.bz2";

/// (filename, expected sha256). `None` = presence + non-empty only (tokens.txt).
const FILES: &[(&str, Option<&str>)] = &[
    ("encoder.int8.onnx", Some("a32b12d17bbbc309d0686fbbcc2987b5e9b8333a7da83fa6b089f0a2acd651ab")),
    ("decoder.int8.onnx", Some("b6bb64963457237b900e496ee9994b59294526439fbcc1fecf705b31a15c6b4e")),
    ("joiner.int8.onnx", Some("7946164367946e7f9f29a122407c3252b680dbae9a51343eb2488d057c3c43d2")),
    ("tokens.txt", None),
];

pub fn model_dir() -> PathBuf {
    models_dir().join(DIR_NAME)
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

pub fn model_files() -> ModelFiles {
    let d = model_dir();
    ModelFiles {
        encoder: d.join("encoder.int8.onnx"),
        decoder: d.join("decoder.int8.onnx"),
        joiner: d.join("joiner.int8.onnx"),
        tokens: d.join("tokens.txt"),
    }
}

pub fn check() -> ModelInfo {
    ModelInfo {
        id: MODEL_ID.into(),
        display: MODEL_DISPLAY.into(),
        present: present(),
        size_mb: SIZE_MB,
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
pub fn download(progress: impl Fn(DownloadProgress)) {
    if let Err(e) = download_inner(&progress) {
        progress(DownloadProgress::Failed { error: e });
    }
}

fn download_inner(progress: &impl Fn(DownloadProgress)) -> Result<(), String> {
    fs::create_dir_all(models_dir()).map_err(|e| e.to_string())?;
    let partial = models_dir().join("download.partial");

    let resume_from = fs::metadata(&partial).map(|m| m.len()).unwrap_or(0);
    let mut req = ureq::get(URL);
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
        Ok(()) => {
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

/// Extract a `.tar.bz2`, verify SHA256s, atomically install into model_dir().
/// Shared by the download and sideload paths.
pub fn install_from_archive(archive: &Path) -> Result<(), String> {
    let staging = models_dir().join(".staging");
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).map_err(|e| e.to_string())?;

    let f = fs::File::open(archive).map_err(|e| e.to_string())?;
    let bz = bzip2::read::BzDecoder::new(f);
    tar::Archive::new(bz)
        .unpack(&staging)
        .map_err(|e| format!("extract failed: {e}"))?;

    let staged = resolve_staged(&staging).ok_or("model files not found in archive")?;
    verify_dir(&staged)?;

    let dest = model_dir();
    if dest.exists() {
        fs::remove_dir_all(&dest).map_err(|e| e.to_string())?;
    }
    fs::rename(&staged, &dest).map_err(|e| e.to_string())?;
    let _ = fs::write(dest.join(".verified"), b"ok");
    let _ = fs::remove_dir_all(&staging);
    Ok(())
}

/// The archive's top entry may be the DIR_NAME folder, a differently-named
/// folder, or the files flat at the root — find where encoder.int8.onnx lives.
fn resolve_staged(staging: &Path) -> Option<PathBuf> {
    if staging.join("encoder.int8.onnx").exists() {
        return Some(staging.to_path_buf());
    }
    let named = staging.join(DIR_NAME);
    if named.join("encoder.int8.onnx").exists() {
        return Some(named);
    }
    fs::read_dir(staging).ok()?.flatten().find_map(|e| {
        let p = e.path();
        (p.is_dir() && p.join("encoder.int8.onnx").exists()).then_some(p)
    })
}

/// Presence + non-empty for all files; SHA256 for the .onnx files, cached via a
/// `.verified` marker so startup does not re-hash 660 MB every launch.
fn present() -> bool {
    let dir = model_dir();
    for (name, _) in FILES {
        match fs::metadata(dir.join(name)) {
            Ok(m) if m.len() > 0 => {}
            _ => return false,
        }
    }
    if dir.join(".verified").exists() {
        return true;
    }
    match verify_dir(&dir) {
        Ok(()) => {
            let _ = fs::write(dir.join(".verified"), b"ok");
            true
        }
        Err(_) => false,
    }
}

fn verify_dir(dir: &Path) -> Result<(), String> {
    for (name, hash) in FILES {
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
        let names: Vec<&str> = FILES.iter().map(|(n, _)| *n).collect();
        assert_eq!(
            names,
            ["encoder.int8.onnx", "decoder.int8.onnx", "joiner.int8.onnx", "tokens.txt"]
        );
        assert_eq!(FILES.iter().filter(|(_, h)| h.is_some()).count(), 3);
        assert!(FILES.last().unwrap().1.is_none()); // tokens.txt: presence-only
    }
}
