//! cpal capture: re-query the device, open a native-rate input stream, push frames into
//! a lock-free ring buffer. The callback does NOTHING but push — no alloc, no locks.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, Host, SampleFormat, Stream, StreamConfig};
use rtrb::{Consumer, Producer, RingBuffer};

/// Everything the worker needs from an opened session. `!Send` (holds a cpal Stream) —
/// constructed on and never leaves the worker thread. Dropping `stream` stops capture.
pub struct StreamHandle {
    pub stream: Stream,
    pub consumer: Consumer<f32>,
    /// Set by the stream error callback (device unplug etc.) — polled by the worker.
    pub dead: Arc<Mutex<Option<String>>>,
    pub channels: usize,
    pub native_rate: u32,
}

/// Re-query the device EVERY call (sessions are seconds; per-session binding suffices),
/// open the stream, and start it immediately so no opening words are lost.
pub fn open(device: Option<String>) -> Result<StreamHandle, String> {
    let host = cpal::default_host();
    let dev = pick_device(&host, device)?;
    let supported = dev.default_input_config().map_err(|e| e.to_string())?;

    let native_rate = supported.sample_rate();
    let channels = (supported.channels() as usize).max(1);
    let fmt = supported.sample_format();
    let config = supported.config();

    // ~2 s of native-rate interleaved headroom; worker drains far faster.
    let (producer, consumer) = RingBuffer::<f32>::new(native_rate as usize * channels * 2);
    let dead = Arc::new(Mutex::new(None));
    let stream = build_stream(&dev, config, fmt, producer, dead.clone())?;
    stream.play().map_err(|e| e.to_string())?;

    Ok(StreamHandle { stream, consumer, dead, channels, native_rate })
}

/// Named device if still present, else the system default (device may have vanished
/// between config save and hotkey press).
fn pick_device(host: &Host, name: Option<String>) -> Result<Device, String> {
    if let Some(name) = name {
        if let Ok(mut devices) = host.input_devices() {
            if let Some(d) = devices.find(|d| d.description().map(|desc| desc.name() == name).unwrap_or(false)) {
                return Ok(d);
            }
        }
    }
    host.default_input_device()
        .ok_or_else(|| "no input device".to_string())
}

fn build_stream(
    dev: &Device,
    config: StreamConfig,
    fmt: SampleFormat,
    mut producer: Producer<f32>,
    dead: Arc<Mutex<Option<String>>>,
) -> Result<Stream, String> {
    let timeout = Some(Duration::from_secs(2));
    let err = move |e: cpal::Error| {
        if let Ok(mut g) = dead.lock() {
            *g = Some(e.to_string());
        }
    };
    // Match the reported format defensively; F32 is the WASAPI shared-mode norm.
    let built = match fmt {
        SampleFormat::F32 => dev.build_input_stream(
            config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                let _ = producer.push_partial_slice(data);
            },
            err,
            timeout,
        ),
        SampleFormat::I16 => dev.build_input_stream(
            config,
            move |data: &[i16], _: &cpal::InputCallbackInfo| {
                for &s in data {
                    let _ = producer.push(s as f32 / 32768.0);
                }
            },
            err,
            timeout,
        ),
        SampleFormat::U16 => dev.build_input_stream(
            config,
            move |data: &[u16], _: &cpal::InputCallbackInfo| {
                for &s in data {
                    let _ = producer.push((s as i32 - 32768) as f32 / 32768.0);
                }
            },
            err,
            timeout,
        ),
        other => return Err(format!("unsupported input sample format: {other:?}")),
    };
    built.map_err(|e| e.to_string())
}

/// Input device names for the settings dropdown (off-UI-thread callers only).
pub fn list_input_devices() -> Vec<String> {
    let host = cpal::default_host();
    match host.input_devices() {
        Ok(devices) => devices.filter_map(|d| d.description().ok().map(|desc| desc.name().to_string())).collect(),
        Err(_) => Vec::new(),
    }
}
