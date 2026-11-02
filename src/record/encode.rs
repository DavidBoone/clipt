//! Video encoding through an `ffmpeg` subprocess fed raw frames on stdin.
//!
//! ffmpeg is optional: only recording needs it. The codec is picked once per
//! recording by test-encoding a frame with each candidate, so the GPU encoder
//! is used wherever one works and libx264 everywhere else:
//!   1. h264_vaapi (AMD, Intel), BGRA → NV12 on the GPU
//!   2. h264_nvenc (NVIDIA's driver has no VA-API encoding)
//!   3. libx264 on the CPU
//!
//! Frames reach ffmpeg in a minimal Matroska stream (see `mkv.rs`) that
//! carries each frame's capture time, and the output keeps those times
//! (variable frame rate): frames only need sending when the screen changed,
//! and a slow encoder drops frames instead of slowing the video down. Sound,
//! when recorded, travels in the same stream with timestamps from the same
//! clock, and is encoded as AAC.

use crate::capture::PixelOrder;
use anyhow::{Context, Result, bail};
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Codec {
    /// VA-API through this render node.
    Vaapi(String),
    Nvenc,
    X264,
}

impl Codec {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Vaapi(_) => "h264_vaapi",
            Self::Nvenc => "h264_nvenc",
            Self::X264 => "libx264",
        }
    }

    /// ffmpeg arguments placed before the input.
    fn global_args(&self) -> Vec<String> {
        match self {
            Self::Vaapi(dev) => vec!["-vaapi_device".into(), dev.clone()],
            _ => vec![],
        }
    }

    /// Filters and encoder settings for a `w` x `h` input. H.264 needs even
    /// dimensions, so odd sizes get a one-pixel pad.
    fn output_args(&self, w: u32, h: u32) -> Vec<String> {
        let pad = "pad=ceil(iw/2)*2:ceil(ih/2)*2";
        let v = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        match self {
            // No B-frames on any encoder: with variable frame rate and sparse
            // frames (a still screen) their reordered timestamps gave MP4
            // files a wrong duration (h264_vaapi on KDE, libx264 on X11).
            Self::Vaapi(_) => {
                v(&["-vf", &format!("{pad},hwupload,scale_vaapi=format=nv12"), "-c:v", "h264_vaapi", "-qp", "24", "-bf", "0"])
            }
            Self::Nvenc => v(&["-vf", pad, "-c:v", "h264_nvenc", "-preset", "p4", "-rc", "vbr", "-cq", "24", "-b:v", "0", "-bf", "0"]),
            Self::X264 => {
                // Above 1440p, veryfast needs several cores to keep up.
                let preset = if w as u64 * h as u64 > 2560 * 1440 { "ultrafast" } else { "veryfast" };
                v(&["-vf", &format!("{pad},format=yuv420p"), "-c:v", "libx264", "-preset", preset, "-crf", "23", "-bf", "0"])
            }
        }
    }
}

/// Candidates for a `record_codec` setting, in the order they're tried.
fn candidates(pref: &str) -> Result<Vec<Codec>> {
    let vaapi = || render_nodes().into_iter().map(Codec::Vaapi).collect::<Vec<_>>();
    Ok(match pref {
        "auto" => {
            let mut v = vaapi();
            v.extend([Codec::Nvenc, Codec::X264]);
            v
        }
        "vaapi" => vaapi(),
        "nvenc" => vec![Codec::Nvenc],
        "x264" => vec![Codec::X264],
        other => bail!("unknown record_codec '{other}' (auto, vaapi, nvenc, x264)"),
    })
}

fn render_nodes() -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir("/dev/dri")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with("renderD"))
        .map(|n| format!("/dev/dri/{n}"))
        .collect();
    v.sort();
    v
}

/// The first codec that encodes a test frame, or an error naming what's missing.
pub fn choose(pref: &str) -> Result<Codec> {
    match Command::new("ffmpeg").arg("-version").stdout(Stdio::null()).stderr(Stdio::null()).status() {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            bail!("recording needs ffmpeg, which isn't installed (install the 'ffmpeg' package)")
        }
        Err(e) => return Err(e).context("running ffmpeg"),
    }
    let mut tried = Vec::new();
    for c in candidates(pref)? {
        match probe(&c) {
            Ok(()) => {
                debug(&format!("encoder: {}", c.name()));
                return Ok(c);
            }
            Err(e) => {
                debug(&format!("encoder {} unusable: {e}", c.name()));
                tried.push(format!("{}: {e}", c.name()));
            }
        }
    }
    bail!("no working video encoder:\n  {}", tried.join("\n  "))
}

/// Encode one small frame the way a recording would, to see if this codec works here.
fn probe(c: &Codec) -> Result<()> {
    let (w, h) = (320, 240);
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-loglevel", "error"]).args(c.global_args());
    cmd.args(["-f", "lavfi", "-i", &format!("color=c=gray:s={w}x{h}:d=0.1,format=bgr0"), "-frames:v", "1"]);
    cmd.args(c.output_args(w, h)).args(["-f", "null", "-"]);
    let out = cmd.stdin(Stdio::null()).output().context("running ffmpeg")?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    bail!("{}", err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("failed").trim())
}

/// Fails, saying so, if this ffmpeg can't encode the AAC sound recordings
/// use or mix sources (a minimal build may leave them out).
pub fn check_aac() -> Result<()> {
    // Silence as 16-bit samples, as recordings send them: a minimal ffmpeg
    // (the Flatpak's) has that decoder, not the 8-bit one anullsrc gives.
    let silence = "anullsrc=r=48000:cl=stereo,aformat=sample_fmts=s16";
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", silence, "-f", "lavfi", "-i", silence])
        .args([
            "-t",
            "0.1",
            "-filter_complex",
            &format!("[0:a]{RESAMPLE},volume=0.5[a];[1:a]{RESAMPLE}[b];[a][b]{MIX}:inputs=2[m]"),
            "-map",
            "[m]",
        ])
        .args(AAC)
        .args(["-f", "null", "-"])
        .stdin(Stdio::null())
        .output()
        .context("running ffmpeg")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!("this ffmpeg can't encode sound (AAC): {}", err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("failed").trim());
    }
    Ok(())
}

/// Each source first goes through the resampler: its async mode closes the
/// tiny gaps and overlaps between chunks stamped on the system clock, and
/// takes up the sound card's drift against it; `first_pts=0` fills any wait
/// before the first chunk with silence (PulseAudio takes over a second to
/// deliver it), so the sound starts with the video.
const RESAMPLE: &str = "aresample=async=1:first_pts=0";
/// Mixing at full volume each (`normalize=0`): the volumes given are the
/// volumes heard, rather than each source being divided by their number.
const MIX: &str = "amix=duration=longest:normalize=0";
const AAC: [&str; 4] = ["-c:a", "aac", "-b:a", "160k"];

/// The sound tracks of a recording: one per source, mixed at these volumes.
pub struct Sound {
    pub volumes: Vec<f32>,
    pub labels: Vec<String>,
    /// Also keep each source as its own track, at the volume it was recorded.
    pub separate: bool,
}

impl Sound {
    /// ffmpeg arguments: the filter graph, the outputs to map after the video,
    /// and their names. MP4 and MOV keep a track's name as its handler name
    /// (what players show); Matroska as its title.
    fn args(&self, mov: bool) -> Vec<String> {
        let name = if mov { "handler_name" } else { "title" };
        let n = self.volumes.len();
        let separate = self.separate && n > 1;
        let vol = |v: f32| if (v - 1.0).abs() < 1e-6 { String::new() } else { format!(",volume={v}") };
        let mut graph = Vec::new();
        for (i, &v) in self.volumes.iter().enumerate() {
            graph.push(if separate {
                format!("[0:a:{i}]{RESAMPLE},asplit=2[m{i}][t{i}];[m{i}]volume={v}[v{i}]")
            } else {
                format!("[0:a:{i}]{RESAMPLE}{}[v{i}]", vol(v))
            });
        }
        let mix = if n == 1 {
            "v0".to_string()
        } else {
            let ins: String = (0..n).map(|i| format!("[v{i}]")).collect();
            graph.push(format!("{ins}{MIX}:inputs={n}[mix]"));
            "mix".to_string()
        };
        let mut a = vec!["-filter_complex".to_string(), graph.join(";"), "-map".into(), "0:v".into(), "-map".into(), format!("[{mix}]")];
        let first = if n > 1 { "Mix" } else { &self.labels[0] };
        a.extend(["-metadata:s:a:0".into(), format!("{name}={first}")]);
        if separate {
            for (i, label) in self.labels.iter().enumerate() {
                a.extend(["-map".into(), format!("[t{i}]"), format!("-metadata:s:a:{}", i + 1), format!("{name}={label}")]);
            }
        }
        a.extend(AAC.iter().map(|s| s.to_string()));
        a
    }
}

pub struct Encoder {
    child: Child,
    stdin: Option<ChildStdin>,
    stderr: Option<std::thread::JoinHandle<String>>,
}

impl Encoder {
    /// Start ffmpeg on a `w` x `h` video, with `sound` fed through
    /// `write_audio`.
    pub fn spawn(codec: &Codec, w: u32, h: u32, order: PixelOrder, fps: u32, sound: Option<&Sound>, path: &Path) -> Result<Self> {
        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-hide_banner", "-loglevel", "error", "-y"]).args(codec.global_args());
        cmd.args(["-f", "matroska", "-i", "pipe:0"]);
        cmd.args(codec.output_args(w, h));
        let mov = matches!(path.extension().and_then(|e| e.to_str()), Some("mp4" | "mov"));
        if let Some(s) = sound {
            cmd.args(s.args(mov));
        }
        // Variable frame rate: each frame keeps its capture time, to the
        // millisecond (left to itself, ffmpeg picks a time base from its
        // frame-rate guess, which can snap timestamps to a coarse grid).
        cmd.args(["-enc_time_base:v", "1/1000"]);
        cmd.args(["-fps_mode", "passthrough"]);
        if matches!(path.extension().and_then(|e| e.to_str()), Some("mp4" | "mov")) {
            cmd.args(["-movflags", "+faststart"]);
        }
        cmd.arg(path);
        // Its own process group: Ctrl-C in the terminal reaches only clipt,
        // which then closes ffmpeg's input so it can finish the file.
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
        let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().context("starting ffmpeg")?;
        let stdin = child.stdin.take();
        let mut err = child.stderr.take().unwrap();
        let stderr = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = err.read_to_string(&mut s);
            s
        });
        let mut enc = Self { child, stdin, stderr: Some(stderr) };
        let tracks = sound.map_or(0, |s| s.volumes.len());
        enc.send(&super::mkv::header(w, h, order, 1_000_000_000 / fps.max(1) as u64, tracks, super::audio::RATE))?;
        Ok(enc)
    }

    /// Send a frame to be shown `ts_ms` milliseconds into the video.
    /// Timestamps must increase.
    pub fn write(&mut self, frame: &[u8], ts_ms: u64) -> Result<()> {
        self.send(&super::mkv::block_prefix(super::mkv::VIDEO, ts_ms, frame.len()))?;
        self.send(frame)
    }

    /// Send a chunk of source `i`'s sound, heard `ts_ms` milliseconds into
    /// the video.
    pub fn write_audio(&mut self, i: usize, pcm: &[u8], ts_ms: u64) -> Result<()> {
        self.send(&super::mkv::block_prefix(super::mkv::audio_track(i), ts_ms, pcm.len()))?;
        self.send(pcm)
    }

    fn send(&mut self, bytes: &[u8]) -> Result<()> {
        let stdin = self.stdin.as_mut().context("encoder already finished")?;
        if stdin.write_all(bytes).is_err() {
            // ffmpeg died; its stderr says why.
            self.stdin = None;
            let _ = self.child.wait();
            bail!("ffmpeg stopped: {}", self.stderr_text());
        }
        Ok(())
    }

    /// Close the input and wait for ffmpeg to write out the file.
    pub fn finish(mut self) -> Result<()> {
        drop(self.stdin.take());
        let status = self.child.wait().context("waiting for ffmpeg")?;
        if !status.success() {
            bail!("ffmpeg failed ({status}): {}", self.stderr_text());
        }
        Ok(())
    }

    fn stderr_text(&mut self) -> String {
        let s = self.stderr.take().and_then(|h| h.join().ok()).unwrap_or_default();
        let s = s.trim();
        if s.is_empty() { "no error output".into() } else { s.lines().last().unwrap_or(s).to_string() }
    }
}

/// Turn a finished recording into a GIF: a palette made from the whole clip,
/// then the clip mapped onto it, at most `fps` frames a second.
pub fn to_gif(mp4: &Path, gif: &Path, fps: u32) -> Result<()> {
    let filter =
        format!("fps={fps},split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=4:diff_mode=rectangle");
    let r = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(mp4)
        .args(["-vf", &filter, "-loop", "0"])
        .arg(gif)
        .stdin(Stdio::null())
        .output()
        .context("running ffmpeg")?;
    if !r.status.success() {
        bail!("making the GIF failed: {}", String::from_utf8_lossy(&r.stderr).trim());
    }
    Ok(())
}

fn debug(msg: &str) {
    if std::env::var_os("CLIPT_DEBUG").is_some() {
        eprintln!("clipt: {msg}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x264_preset_drops_above_1440p() {
        let a = Codec::X264.output_args(1920, 1080);
        assert!(a.contains(&"veryfast".to_string()));
        let a = Codec::X264.output_args(3840, 2160);
        assert!(a.contains(&"ultrafast".to_string()));
    }

    #[test]
    fn every_codec_pads_to_even_size() {
        for c in [Codec::Vaapi("/dev/dri/renderD128".into()), Codec::Nvenc, Codec::X264] {
            let vf = c.output_args(101, 99);
            let i = vf.iter().position(|a| a == "-vf").unwrap();
            assert!(vf[i + 1].starts_with("pad=ceil(iw/2)*2:ceil(ih/2)*2"), "{c:?}: {}", vf[i + 1]);
        }
    }

    /// The AAC check's own command line is valid (it once wasn't, which made
    /// every sound recording fail the check). Needs ffmpeg; passes without.
    #[test]
    fn aac_check_runs() {
        if Command::new("ffmpeg").arg("-version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_err() {
            return;
        }
        check_aac().unwrap();
    }

    #[test]
    fn sound_graphs() {
        let s = |v: &[f32], separate| Sound {
            volumes: v.to_vec(),
            labels: v.iter().enumerate().map(|(i, _)| format!("S{i}")).collect(),
            separate,
        };
        let graph = |s: &Sound| s.args(false)[1].clone();
        let maps = |s: &Sound| s.args(false).windows(2).filter(|w| w[0] == "-map").map(|w| w[1].clone()).collect::<Vec<_>>();
        // One source: no mixer, and a volume only when it isn't 100 %.
        let one = s(&[1.0], true);
        assert_eq!(graph(&one), format!("[0:a:0]{RESAMPLE}[v0]"));
        assert_eq!(maps(&one), ["0:v", "[v0]"], "a lone source isn't kept twice");
        assert!(graph(&s(&[0.5], false)).ends_with(",volume=0.5[v0]"));
        // Two: each at its volume into the mixer.
        let two = s(&[0.7, 1.2], false);
        assert_eq!(graph(&two), format!("[0:a:0]{RESAMPLE},volume=0.7[v0];[0:a:1]{RESAMPLE},volume=1.2[v1];[v0][v1]{MIX}:inputs=2[mix]"));
        assert_eq!(maps(&two), ["0:v", "[mix]"]);
        // Separate tracks: split before the volume, so they're as recorded.
        let sep = s(&[0.7, 1.0], true);
        assert!(graph(&sep).starts_with(&format!("[0:a:0]{RESAMPLE},asplit=2[m0][t0];[m0]volume=0.7[v0];")));
        assert_eq!(maps(&sep), ["0:v", "[mix]", "[t0]", "[t1]"]);
        assert!(sep.args(false).windows(2).any(|w| w == ["-metadata:s:a:2", "title=S1"]));
        assert!(sep.args(true).windows(2).any(|w| w == ["-metadata:s:a:0", "handler_name=Mix"]), "MP4 names tracks by handler");
    }

    #[test]
    fn codec_preferences() {
        assert_eq!(candidates("x264").unwrap(), vec![Codec::X264]);
        assert_eq!(candidates("nvenc").unwrap(), vec![Codec::Nvenc]);
        assert!(candidates("auto").unwrap().ends_with(&[Codec::Nvenc, Codec::X264]));
        assert!(candidates("av1").is_err());
    }
}
