//! Sound for recordings: which sources, and the chunks they're read in.
//! Reading them is the platform's: libpulse-simple on Linux and the BSDs
//! (`audio/pulse.rs`), ScreenCaptureKit and an Audio Queue on macOS
//! (`macos/sound.rs`). Each offers `check`, `open` and `Capture::run`.

#[cfg(wayland_x11)]
mod pulse;
#[cfg(target_os = "macos")]
pub use crate::macos::sound::{check, open};
#[cfg(wayland_x11)]
pub use pulse::{check, devices, open};

use anyhow::{Result, bail};
use std::time::Instant;

pub const RATE: u32 = 48_000;
pub const CHANNELS: u32 = 2;
/// 16-bit samples.
pub const BYTES_PER_FRAME: u32 = 2 * CHANNELS;
/// 20 ms.
pub const CHUNK_FRAMES: u32 = RATE / 50;

/// Most sources one recording takes (each is a Matroska track, 2 to 9).
pub const MAX_SOURCES: usize = 8;

/// Where one source's sound comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Device {
    /// What the computer is playing: the default output's monitor.
    System,
    /// The default input.
    Mic,
    /// A source by its server name, as `clipt list sound` shows it.
    Named(String),
}

/// One source and its volume in the mix.
#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    pub device: Device,
    /// 1.0 = as recorded.
    pub volume: f32,
}

impl Source {
    /// A `record_audio` value or `--audio=` argument: "none", "both", or
    /// sources separated by commas, each `system`, `mic` or a device name,
    /// optionally followed by `:PERCENT`.
    pub fn parse(s: &str) -> Result<Vec<Self>> {
        let s = s.trim();
        if s.is_empty() || s == "none" {
            return Ok(Vec::new());
        }
        let s = if s == "both" { "system,mic" } else { s };
        let mut v: Vec<Self> = Vec::new();
        for part in s.split(',').map(str::trim) {
            // Device names don't contain ':', but check the volume is a number
            // before splitting on it all the same.
            let (name, volume) = match part.rsplit_once(':') {
                Some((n, pct)) if pct.parse::<f32>().is_ok() => {
                    let pct: f32 = pct.parse().unwrap();
                    if !(0.0..=400.0).contains(&pct) {
                        bail!("volume {pct} for '{n}' is outside 0 to 400 %");
                    }
                    (n, pct / 100.0)
                }
                _ => (part, 1.0),
            };
            let device = match name {
                "" => bail!("empty audio source in '{s}'"),
                "none" | "both" => bail!("'{name}' can't be combined with other sources"),
                "system" => Device::System,
                "mic" => Device::Mic,
                n => Device::Named(n.into()),
            };
            if v.iter().any(|x| x.device == device) {
                bail!("'{name}' is listed twice");
            }
            v.push(Self { device, volume });
        }
        if v.len() > MAX_SOURCES {
            bail!("at most {MAX_SOURCES} audio sources");
        }
        Ok(v)
    }

    /// Whether `sources` include the computer's sound and the microphone, as
    /// the overlay's toggles show them.
    pub fn toggles(sources: &[Self]) -> (bool, bool) {
        (sources.iter().any(|s| s.device == Device::System), sources.iter().any(|s| s.device == Device::Mic))
    }

    /// `sources` with `system` and `mic` turned on or off as the overlay left
    /// them: a source turned off goes, one turned on is added at full volume,
    /// and everything else (named devices, volumes) stays as it was.
    pub fn with_toggles(mut sources: Vec<Self>, system: bool, mic: bool) -> Vec<Self> {
        for (device, on) in [(Device::System, system), (Device::Mic, mic)] {
            let have = sources.iter().any(|s| s.device == device);
            if have && !on {
                sources.retain(|s| s.device != device);
            } else if on && !have {
                sources.push(Self { device, volume: 1.0 });
            }
        }
        sources
    }

    /// The track's title in the file.
    pub fn label(&self) -> String {
        match &self.device {
            Device::System => "System sound".into(),
            Device::Mic => "Microphone".into(),
            Device::Named(n) => n.clone(),
        }
    }
}

/// 20 ms of interleaved 16-bit stereo, and when its first sample was heard.
pub struct Chunk {
    pub at: Instant,
    pub data: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources() {
        let s = |d: Device, volume: f32| Source { device: d, volume };
        assert!(Source::parse("none").unwrap().is_empty());
        assert!(Source::parse("").unwrap().is_empty());
        assert_eq!(Source::parse("system").unwrap(), [s(Device::System, 1.0)]);
        assert_eq!(Source::parse("both").unwrap(), [s(Device::System, 1.0), s(Device::Mic, 1.0)]);
        assert_eq!(Source::parse("system:70, mic:120").unwrap(), [s(Device::System, 0.7), s(Device::Mic, 1.2)]);
        assert_eq!(
            Source::parse("alsa_input.usb-Foo-00.analog-stereo:50").unwrap(),
            [s(Device::Named("alsa_input.usb-Foo-00.analog-stereo".into()), 0.5)]
        );
        assert_eq!(Source::parse("mic:0").unwrap(), [s(Device::Mic, 0.0)], "muted in the mix, still a track");
        assert!(Source::parse("system:500").is_err(), "too loud");
        assert!(Source::parse("system,system:50").is_err(), "twice");
        assert!(Source::parse("none,mic").is_err());
        assert!(Source::parse("system,").is_err());
        assert!(Source::parse(&vec!["mic"; 1].into_iter().chain((0..8).map(|_| "x")).collect::<Vec<_>>().join(",")).is_err());
    }

    #[test]
    fn overlay_toggles() {
        let v = Source::parse("system:70,alsa_input.x:50").unwrap();
        assert_eq!(Source::toggles(&v), (true, false));
        // Turning the mic on adds it at 100 %; system keeps its 70 %.
        assert_eq!(Source::with_toggles(v.clone(), true, true), Source::parse("system:70,alsa_input.x:50,mic").unwrap());
        // Turning system off leaves the named device alone.
        assert_eq!(Source::with_toggles(v.clone(), false, false), Source::parse("alsa_input.x:50").unwrap());
        assert_eq!(Source::with_toggles(Vec::new(), false, false), Vec::new());
    }
}
