# Third-Party Licenses

Dictum incorporates the following open-source projects:

## Dictum Core Dependencies

### Tauri Framework
- **License:** MIT / Apache-2.0 dual
- **Repository:** https://github.com/tauri-apps/tauri
- **Version:** 2.11.5

### sherpa-onnx (Official Rust Crate)
- **License:** Apache-2.0
- **Repository:** https://github.com/k2-fsa/sherpa-onnx
- **Version:** 1.13.4
- **Notice:** Provides the ASR runtime for offline speech recognition.

### Silero VAD (Voice Activity Detector)
- **License:** MIT
- **Repository:** https://github.com/snakers4/silero-vad
- **Integrated via:** sherpa-onnx
- **Notice:** Used for speech segment detection and silence trimming.

## Audio Processing

### cpal
- **License:** Apache-2.0 / MIT dual
- **Repository:** https://github.com/RustAudio/cpal
- **Version:** 0.18.1
- **Notice:** WASAPI-based audio capture from the default input device.

### rubato
- **License:** MIT / Apache-2.0 dual
- **Repository:** https://github.com/HEnquist/rubato
- **Version:** 0.16.2
- **Notice:** Resampling from device native rate (typically 48 kHz) to 16 kHz for ASR.

### rtrb
- **License:** MIT / Apache-2.0 dual
- **Repository:** https://github.com/RustAudio/rtrb
- **Version:** 0.3.4
- **Notice:** Lock-free ring buffer for audio callback to worker thread handoff.

### rodio
- **License:** MIT / Apache-2.0 dual
- **Repository:** https://github.com/RustAudio/rodio
- **Version:** 0.22.2
- **Notice:** Earcon (cue) audio playback on an isolated output thread.

### hound
- **License:** Apache-2.0
- **Repository:** https://github.com/ruuda/hound
- **Version:** 3.5.1
- **Notice:** WAV audio encoding and decoding.

## Windows Integration

### windows-rs (windows crate)
- **License:** MIT / Apache-2.0 dual
- **Repository:** https://github.com/microsoft/windows-rs
- **Version:** 0.62.2
- **Notice:** Win32 API bindings for hotkey registration, clipboard operations, process elevation detection, and system event handling.

### windows-registry
- **License:** MIT / Apache-2.0 dual
- **Repository:** https://github.com/microsoft/windows-rs
- **Version:** 0.6.0
- **Notice:** Windows Registry access for microphone privacy toggle detection.

### clipboard-win
- **License:** MIT
- **Repository:** https://github.com/DoumanAsh/clipboard-win
- **Version:** 5.4.1
- **Notice:** Clipboard text snapshot, write, and restore with exclusion formats for Cloud Clipboard and Clipboard History.

### rusqlite
- **License:** Public Domain (Rusqlite wrapper) / Public Domain (Bundled SQLite 3.53.2)
- **Repository:** https://github.com/rusqlite/rusqlite
- **Version:** 0.40.1 (with bundled feature)
- **Notice:** Local history database (WAL mode) for transcript retention and search.

## Utilities

### serde / serde_json
- **License:** MIT / Apache-2.0 dual
- **Repository:** https://github.com/serde-rs/serde
- **Versions:** 1.x / 1.x
- **Notice:** Serialization framework for config and IPC types.

### dirs
- **License:** MIT / Apache-2.0 dual
- **Repository:** https://github.com/serde-rs/dirs
- **Version:** 6.0.0
- **Notice:** Platform-aware %APPDATA% / home directory resolution.

### ureq
- **License:** Apache-2.0 / MIT dual
- **Repository:** https://github.com/algesten/ureq
- **Version:** 2.x
- **Notice:** HTTP client for model download (the app's only network operation, explicitly triggered by the user).

### sha2
- **License:** Apache-2.0 / MIT dual
- **Repository:** https://github.com/RustCrypto/hashes
- **Version:** 0.10.x
- **Notice:** SHA256 verification of downloaded model archives.

### tar
- **License:** MIT / Apache-2.0 dual
- **Repository:** https://github.com/alexcrichton/tar-rs
- **Version:** 0.4.x
- **Notice:** Tar archive extraction for model packages.

### bzip2
- **License:** MIT / Apache-2.0 dual
- **Repository:** https://github.com/alexcrichton/bzip2-rs
- **Version:** 0.4.x
- **Notice:** Bzip2 decompression for model packages.

### regex
- **License:** MIT / Apache-2.0 dual
- **Repository:** https://github.com/rust-lang/regex
- **Version:** 1.x
- **Notice:** Replacement rules engine (deterministic post-ASR text transformations).

### anyhow
- **License:** MIT / Apache-2.0 dual
- **Repository:** https://github.com/dtolnay/anyhow
- **Version:** 1.x
- **Notice:** Error handling utility.

## Model Weights

### NVIDIA Parakeet-TDT 0.6B v2
- **License:** CC-BY-4.0 (Creative Commons Attribution 4.0 International)
- **Author:** NVIDIA
- **Model:** Parakeet-TDT 0.6B v2 (English, ASR transducer)
- **URL:** https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8
- **License URL:** https://creativecommons.org/licenses/by/4.0/
- **Attribution Required:** Yes. See below for required attribution text.

**Modifications:** ONNX export and int8 quantization by the sherpa-onnx project.

**Attribution Statement:** The model weights used by Dictum are derived from NVIDIA Parakeet-TDT 0.6B v2, published by NVIDIA under the CC-BY-4.0 license. The weights have been exported to ONNX format and quantized to int8 precision by the sherpa-onnx project. For full model details, see https://github.com/NVIDIA/NeMo/tree/main/nemo/collections/asr/models/transducers.

This CC-BY-4.0 attribution requirement applies to the model weights only and does not affect the licensing of the Dictum application code (Apache-2.0).

---

All licenses are available in their respective repositories. Where dual licensing is offered, either license may be used.
