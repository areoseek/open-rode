//! Audio side of the receiver: where the system exposes it as a sound card,
//! and a level meter on that capture stream.
//!
//! The Wireless GO II RX is a class-compliant USB audio device (2 ch, 24-bit,
//! 48 kHz) that `snd-usb-audio` picks up by itself. Left = TX1, right = TX2
//! (Split) or the safety channel. Note that the receiver's "output gain" only
//! affects the 3.5 mm jack; the USB stream is always at unity.
//!
//! The meter opens the stream through PipeWire's ALSA plugin (PCM `pipewire`
//! with `PIPEWIRE_NODE` pointing at the receiver) so it shares the device with
//! whatever else is recording. If that PCM is missing it falls back to ALSA
//! directly, which is exclusive.

use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::process::Command;
use std::sync::{Arc, Mutex};

/// How the operating system names the receiver's audio input.
#[derive(Debug, Clone, Default)]
pub struct AudioSource {
    /// ALSA card id, e.g. `RX` (so `hw:RX`).
    pub alsa_card: Option<String>,
    /// ALSA long name, e.g. `RØDE Wireless GO II RX at usb-...`.
    pub alsa_name: Option<String>,
    /// PipeWire/PulseAudio node name, e.g. `alsa_input.usb-R__DE_Wireless_GO_II_RX_...-01.pro-input-0`.
    pub pw_node: Option<String>,
    /// What desktop apps show, e.g. `Wireless GO II RX Pro`.
    pub pw_description: Option<String>,
}

impl AudioSource {
    pub fn discover() -> Self {
        let mut s = AudioSource::default();
        if let Ok(cards) = std::fs::read_to_string("/proc/asound/cards") {
            // " 5 [RX             ]: USB-Audio - Wireless GO II RX\n   RØDE Wireless GO II RX at usb-..."
            let lines: Vec<&str> = cards.lines().collect();
            for (i, l) in lines.iter().enumerate() {
                if l.contains("Wireless GO II") {
                    if let (Some(a), Some(b)) = (l.find('['), l.find(']')) {
                        s.alsa_card = Some(l[a + 1..b].trim().to_string());
                    }
                    s.alsa_name = lines.get(i + 1).map(|n| n.trim().to_string());
                    break;
                }
            }
        }
        if let Ok(out) = Command::new("pw-dump").output() {
            if let Ok(text) = String::from_utf8(out.stdout) {
                // Avoid a JSON dependency: scan node objects textually.
                for obj in text.split("\"type\": \"PipeWire:Interface:Node\"").skip(1) {
                    let obj = &obj[..obj.find("\"type\": \"PipeWire:Interface:").unwrap_or(obj.len())];
                    if !obj.contains("Audio/Source") || !obj.contains("Wireless_GO_II") {
                        continue;
                    }
                    s.pw_node = prop(obj, "node.name");
                    s.pw_description = prop(obj, "node.description");
                    break;
                }
            }
        }
        s
    }

    pub fn present(&self) -> bool {
        self.alsa_card.is_some()
    }
}

fn prop(obj: &str, key: &str) -> Option<String> {
    let k = format!("\"{key}\": \"");
    let i = obj.find(&k)? + k.len();
    let j = obj[i..].find('"')?;
    Some(obj[i..i + j].to_string())
}

/// Per-channel level, updated by the audio callback.
#[derive(Debug, Clone, Copy, Default)]
pub struct Level {
    /// Peak of the most recent buffer, 0..=1.
    pub peak: f32,
    /// RMS of the most recent buffer, 0..=1.
    pub rms: f32,
}

pub struct Meter {
    _stream: cpal::Stream,
    pub levels: Arc<Mutex<[Level; 2]>>,
    pub via: String,
}

impl Meter {
    /// Open the receiver's capture stream. Prefers PipeWire so the device
    /// stays shareable; falls back to direct ALSA.
    pub fn open(src: &AudioSource) -> Result<Self> {
        let card = src.alsa_card.clone().ok_or_else(|| anyhow!("receiver is not a sound card"))?;
        let host = cpal::default_host();
        let devices: Vec<cpal::Device> = host.input_devices()?.collect();
        let name_of = |d: &cpal::Device| d.name().unwrap_or_default();

        let mut candidates: Vec<(String, &cpal::Device)> = Vec::new();
        if let Some(node) = &src.pw_node {
            if let Some(d) = devices.iter().find(|d| name_of(d) == "pipewire") {
                // Read by the PipeWire ALSA plugin when it opens the PCM.
                std::env::set_var("PIPEWIRE_NODE", node);
                candidates.push((format!("PipeWire ({})", src.pw_description.clone().unwrap_or_default()), d));
            }
        }
        for pcm in [format!("sysdefault:CARD={card}"), format!("plughw:CARD={card},DEV=0"), format!("hw:CARD={card},DEV=0")] {
            if let Some(d) = devices.iter().find(|d| name_of(d) == pcm) {
                candidates.push((format!("ALSA {pcm}"), d));
            }
        }
        if candidates.is_empty() {
            return Err(anyhow!("no capture device found for card {card}"));
        }

        let levels = Arc::new(Mutex::new([Level::default(); 2]));
        let mut last_err = anyhow!("no candidates");
        for (via, dev) in candidates {
            match build(dev, levels.clone()) {
                Ok(stream) => {
                    stream.play()?;
                    return Ok(Meter { _stream: stream, levels, via });
                }
                Err(e) => last_err = e,
            }
        }
        Err(last_err)
    }

    pub fn read(&self) -> [Level; 2] {
        *self.levels.lock().unwrap()
    }
}

fn build(dev: &cpal::Device, levels: Arc<Mutex<[Level; 2]>>) -> Result<cpal::Stream> {
    let cfg = cpal::StreamConfig { channels: 2, sample_rate: cpal::SampleRate(48_000), buffer_size: cpal::BufferSize::Default };
    let err = |e| log::warn!("audio stream error: {e}");
    // Try f32 first (what PipeWire hands out), then the integer formats the
    // hardware exposes directly.
    let supported: Vec<cpal::SampleFormat> = dev
        .supported_input_configs()
        .map(|c| c.map(|r| r.sample_format()).collect())
        .unwrap_or_default();
    let fmt = [cpal::SampleFormat::F32, cpal::SampleFormat::I32, cpal::SampleFormat::I16]
        .into_iter()
        .find(|f| supported.contains(f))
        .unwrap_or(cpal::SampleFormat::F32);
    let stream = match fmt {
        cpal::SampleFormat::F32 => dev.build_input_stream(&cfg, move |d: &[f32], _| measure(d, &levels, |v| v), err, None)?,
        cpal::SampleFormat::I32 => dev.build_input_stream(&cfg, move |d: &[i32], _| measure(d, &levels, |v| v as f32 / i32::MAX as f32), err, None)?,
        cpal::SampleFormat::I16 => dev.build_input_stream(&cfg, move |d: &[i16], _| measure(d, &levels, |v| v as f32 / i16::MAX as f32), err, None)?,
        f => return Err(anyhow!("unsupported sample format {f:?}")),
    };
    Ok(stream)
}

fn measure<T: Copy>(data: &[T], levels: &Mutex<[Level; 2]>, to_f32: impl Fn(T) -> f32) {
    let mut peak = [0f32; 2];
    let mut sq = [0f32; 2];
    let frames = (data.len() / 2).max(1);
    for (i, s) in data.iter().enumerate() {
        let v = to_f32(*s);
        let c = i & 1;
        peak[c] = peak[c].max(v.abs());
        sq[c] += v * v;
    }
    let mut l = levels.lock().unwrap();
    for c in 0..2 {
        l[c] = Level { peak: peak[c], rms: (sq[c] / frames as f32).sqrt() };
    }
}

pub fn dbfs(v: f32) -> f32 {
    if v <= 1e-7 { -140.0 } else { 20.0 * v.log10() }
}
