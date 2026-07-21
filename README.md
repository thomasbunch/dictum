# Dictum

Local, CPU-only, push-to-talk dictation for Windows.

Hold your hotkey anywhere, speak, release — transcribed text appears at your cursor in under a second. Fully local processing, zero network calls during use, no telemetry.

## Status

Pre-release. Internal codename: Dictum (public name pending rename).

Currently in development. Milestone M0 (proof of concept on real hardware) and M1 (walking skeleton) underway.

## Quick Start

### Installer (Recommended)

1. Download the latest installer from [GitHub Releases](https://github.com/thomasbunch/dictum/releases)
2. Run `Dictum-Setup.exe` (per-user, no admin required)
3. On first launch, the app downloads the English ASR model (~630 MB) and stores it in `%APPDATA%\Dictum\models\`
4. Press and hold **Ctrl+Win** to start dictating; release to transcribe and paste

### Offline Sideload (Air-Gapped / Locked-Down Networks)

If your machine has no internet access or network restrictions:

1. On a machine with internet, download the model archive:
   - English (Parakeet-TDT 0.6B v2, int8): [sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8.tar.bz2](https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8.tar.bz2)
2. Transfer the archive to your target machine via USB or internal file transfer
3. Extract into `%APPDATA%\Dictum\models\` (create the folder if it doesn't exist)
4. Launch Dictum; it detects the model and skips the download step

### Hotkey

**Default:** Ctrl+Win (hold to dictate, release to paste)

- **Hold mode:** Press and hold to record; release immediately after speaking
- **Toggle mode:** Tap to start, tap again to stop (configurable in Settings)
- **Mixed mode:** Both hold and toggle work with the same key (configurable)

Customizable in Settings → Hotkey. The app validates chord availability on startup.

## Zero-Egress Verification

**Claim:** The dictation pipeline contains no networking code, no updater, no telemetry. Audio and text never leave your machine. The app's only network operation is the model download you explicitly trigger once (or skip entirely via sideload). Enforceable via per-exe firewall rule; verifiable via Sysmon Event ID 3 and packet capture.

### How to Verify

**Firewall Rule (Windows Defender Firewall with Advanced Security):**

```powershell
New-NetFirewallRule -DisplayName "Dictum - Egress Block" `
  -Direction Outbound `
  -Action Block `
  -Program "C:\Users\<YourUser>\AppData\Local\Dictum\Dictum.exe" `
  -Protocol TCP,UDP `
  -RemoteAddress Any
```

If you apply this rule *after* first-run model download, the app continues to work normally during dictation. No errors, no outbound attempts.

**Sysmon Event ID 3 (Network Connection):**

Enable Sysmon:

```powershell
# Download Sysmon from https://docs.microsoft.com/en-us/sysinternals/downloads/sysmon
sysmon.exe -i -accepteula
```

Use Event Viewer → Windows Logs → System to monitor Sysmon events. Filter for:
- Event ID: 3
- Image: `Dictum.exe`

During a full dictation session (hotkey down, speak, paste), zero connection attempts should appear.

**Note:** WebView2 runtime updates and Edge updates are OS-level processes outside the Dictum exe and may generate traffic independently. The claim applies only to the Dictum application process itself.

## SmartScreen Warning

Dictum ships as an unsigned binary. Windows may display a SmartScreen warning on first run:

> "Windows protected your PC" / "Unknown publisher"

This is normal for new applications. Click "More info" → "Run anyway" to proceed.

Signed binaries (via Azure Trusted Signing) are planned for wider release and enterprise deployment.

## Features

- **Offline ASR:** Parakeet-TDT 0.6B v2 (6.05% WER, ~4× faster than Whisper Small)
- **Latency:** Key-release to text-in-app **< 1000 ms** for utterances ≤ 15 seconds on target hardware (i5/i7 corporate class)
- **No configuration:** Works out of the box; sensible defaults for all settings
- **Multi-app support:** Tested with Word, Excel, Chrome, Firefox, Teams, Slack, Windows Terminal, VS Code, and more
- **Deterministic replacements:** Post-transcription text rules (e.g. "em-dash" → "—") with import/export
- **Local history:** Optional transcript retention (0–30 days configurable; no audio saved by default)
- **Accessibility:** Describes failures in clear, actionable user-facing text; no silent failures
- **Elevated window detection:** If the target app is running as admin, Dictum offers clipboard-only injection and clear next steps

## Requirements

- **OS:** Windows 10 / Windows 11 (22H2 or later recommended)
- **Hardware:** 
  - CPU: x86-64 (i5/i7 class or better)
  - RAM: 2 GB minimum (4+ GB recommended; model footprint ~1.2–2 GB)
  - Audio:** Working microphone (headset, USB, or built-in)
- **Network:** None required for dictation; internet needed only for first-run model download

## License

Dictum is licensed under the **Apache License 2.0**. See [LICENSE](LICENSE) for the full text.

Copyright 2026 Dictum contributors.

### Third-Party Licenses

This project incorporates several open-source libraries:

- **sherpa-onnx** (speech recognition runtime): Apache-2.0
- **Silero VAD** (voice activity detection): MIT
- **Tauri** (app framework): MIT / Apache-2.0
- **Audio processing** (cpal, rubato, rtrb, rodio, hound): MIT / Apache-2.0
- **Windows integration** (windows-rs, clipboard-win): MIT / Apache-2.0
- **Other utilities** (serde, dirs, ureq, sha2, regex, etc.): MIT / Apache-2.0

**Model weights:**
- **NVIDIA Parakeet-TDT 0.6B v2** (ASR model): CC-BY-4.0
  - Attribution: © NVIDIA. Model licensed under CC-BY-4.0. ONNX export and int8 quantization by sherpa-onnx.
  - See [THIRD-PARTY-LICENSES.md](THIRD-PARTY-LICENSES.md) for full attribution and model details.

See [THIRD-PARTY-LICENSES.md](THIRD-PARTY-LICENSES.md) for the complete license list.

## Feedback

Found a bug? Noticed inaccurate transcription for your accent or domain? Open an issue on [GitHub](https://github.com/username/dictum/issues).

Feature requests are welcome. Note that Dictum is a personal tool first; enterprise features live in optional documentation, not the critical path.
