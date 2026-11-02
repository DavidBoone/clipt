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
use std::time::{Duration, Instant};

/// How long `Encoder::finish` waits for ffmpeg to exit after its input
/// closes: encoders flush their buffer in well under a second.
#[cfg(not(test))]
const FINISH_LIMIT: Duration = Duration::from_secs(30);
#[cfg(test)]
const FINISH_LIMIT: Duration = Duration::from_millis(500);

/// How long `to_gif` may take: it reads the whole recording, so its limit
/// scales with the recording (60 s plus twice its length).
#[cfg(not(test))]
fn gif_limit(duration: Duration) -> Duration {
    Duration::from_secs(60) + duration.saturating_mul(2)
}
#[cfg(test)]
fn gif_limit(_duration: Duration) -> Duration {
    Duration::from_millis(500)
}

/// Wait for `child` to exit, at most `limit`; `None` means it's still running.
fn wait_up_to(child: &mut Child, limit: Duration) -> Result<Option<std::process::ExitStatus>> {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(status) = child.try_wait().context("waiting for ffmpeg")? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// SIGKILL and reap: a hung ffmpeg has already ignored SIGTERM.
fn kill(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

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

/// What `record_audio_balance` / `--audio-balance` asks for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Balance {
    #[default]
    Off,
    /// Clean up the microphone's sound and duck the other sources under it.
    Duck,
}

impl Balance {
    /// A `record_audio_balance` value or `--audio-balance` argument.
    pub fn parse(s: &str) -> Result<Self> {
        match s.trim() {
            "off" => Ok(Self::Off),
            "duck" => Ok(Self::Duck),
            other => bail!("unknown audio balance '{other}' (off, duck)"),
        }
    }
}

/// Voice clean-up for `duck`, from the phase 4 spike: high-pass at 80 Hz,
/// 3:1 compression from -30 dB (threshold 0.0316), de-essing and a 3 kHz
/// presence boost. The spike's 4 dB make-up left the voice 5.5 dB quieter;
/// 9.5 dB (`makeup=3`) brings the cleaned voice back to the raw one's level.
const CLEAN_MIC: &str = "highpass=f=80,acompressor=threshold=0.0316:ratio=3:makeup=3.0,deesser,equalizer=f=3000:width_type=q:w=1:g=3";
/// The ducking from the spike: about 11 dB of dip while the voice speaks.
const DUCK: &str = "sidechaincompress=threshold=0.02:ratio=8:attack=20:release=300";

/// The sound tracks of a recording: one per source, mixed at these volumes.
pub struct Sound {
    pub volumes: Vec<f32>,
    pub labels: Vec<String>,
    /// Also keep each source as its own track, at the volume it was recorded.
    pub separate: bool,
    /// The microphone's source index, when it is recorded: the voice `duck`
    /// works on. A named microphone device isn't the voice.
    pub voice: Option<usize>,
    pub balance: Balance,
}

impl Sound {
    /// ffmpeg arguments: the filter graph, the outputs to map after the video,
    /// and their names. MP4 and MOV keep a track's name as its handler name
    /// (what players show); Matroska as its title.
    fn args(&self, mov: bool) -> Vec<String> {
        let name = if mov { "handler_name" } else { "title" };
        let n = self.volumes.len();
        let separate = self.separate && n > 1;
        // `duck` needs the microphone and something to duck under it.
        let duck = (self.balance == Balance::Duck && n > 1).then_some(self.voice).flatten().filter(|&v| v < n);
        if self.balance == Balance::Duck && duck.is_none() {
            eprintln!("clipt: audio balance: no microphone to duck under");
        }
        let mut graph = Vec::new();
        if let Some(voice) = duck {
            self.duck_graph(voice, separate, &mut graph);
        } else {
            for (i, &v) in self.volumes.iter().enumerate() {
                graph.push(if separate {
                    format!("[0:a:{i}]{RESAMPLE},asplit=2[m{i}][t{i}];[m{i}]volume={v}[v{i}]")
                } else {
                    format!("[0:a:{i}]{RESAMPLE}{}[v{i}]", volume_arg(v))
                });
            }
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

    /// The `duck` graph: the microphone through the clean-up chain, feeding
    /// both the mix and the ducking keys; every other source, after its
    /// volume, is sidechained to the voice.
    fn duck_graph(&self, voice: usize, separate: bool, graph: &mut Vec<String>) {
        let others: Vec<usize> = (0..self.volumes.len()).filter(|&i| i != voice).collect();
        // The clean voice is split into the mix and the keys. One key is
        // named directly; several need a second split.
        let key = if others.len() == 1 { format!("[k{}]", others[0]) } else { "[key]".to_string() };
        if separate {
            graph.push(format!("[0:a:{voice}]{RESAMPLE},asplit=2[wet{voice}][t{voice}]"));
            graph.push(format!("[wet{voice}]{CLEAN_MIC},asplit=2[m{voice}]{key}"));
        } else {
            graph.push(format!("[0:a:{voice}]{RESAMPLE},{CLEAN_MIC},asplit=2[m{voice}]{key}"));
        }
        graph.push(format!("[m{voice}]volume={}[v{voice}]", self.volumes[voice]));
        if others.len() > 1 {
            let keys: String = others.iter().map(|i| format!("[k{i}]")).collect();
            graph.push(format!("[key]asplit={}{keys}", others.len()));
        }
        for &i in &others {
            let v = self.volumes[i];
            graph.push(if separate {
                format!("[0:a:{i}]{RESAMPLE},asplit=2[m{i}][t{i}];[m{i}]volume={v}[w{i}]")
            } else {
                format!("[0:a:{i}]{RESAMPLE}{}[w{i}]", volume_arg(v))
            });
            graph.push(format!("[w{i}][k{i}]{DUCK}[v{i}]"));
        }
    }
}

/// A source's filter argument for its volume, omitted when it is 100 %.
fn volume_arg(v: f32) -> String {
    if (v - 1.0).abs() < 1e-6 { String::new() } else { format!(",volume={v}") }
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

    /// Close the input and wait for ffmpeg to write out the file. Killed and
    /// reported if it hasn't exited within `FINISH_LIMIT`: the caller says
    /// where the partial file is.
    pub fn finish(mut self) -> Result<()> {
        drop(self.stdin.take());
        let Some(status) = wait_up_to(&mut self.child, FINISH_LIMIT)? else {
            kill(&mut self.child);
            bail!("ffmpeg didn't finish within {FINISH_LIMIT:?}, so it was killed: {}", self.stderr_text());
        };
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
/// then the clip mapped onto it, at most `fps` frames a second. `duration` is
/// the recording's length; it scales how long ffmpeg may take. Killed and
/// reported if it takes longer, leaving the MP4 for the caller.
pub fn to_gif(mp4: &Path, gif: &Path, fps: u32, duration: Duration) -> Result<()> {
    let filter =
        format!("fps={fps},split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=4:diff_mode=rectangle");
    let mut child = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(mp4)
        .args(["-vf", &filter, "-loop", "0"])
        .arg(gif)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("running ffmpeg")?;
    // Read its errors on a thread, so a chatty ffmpeg can't fill the pipe and
    // block while we wait for it.
    let mut err = child.stderr.take().unwrap();
    let stderr = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = err.read_to_string(&mut s);
        s
    });
    let limit = gif_limit(duration);
    let status = wait_up_to(&mut child, limit)?;
    if status.is_none() {
        kill(&mut child);
    }
    let stderr = stderr.join().unwrap_or_default();
    let Some(status) = status else {
        bail!("making the GIF failed: ffmpeg didn't finish within {limit:?}, so it was killed");
    };
    if !status.success() {
        bail!("making the GIF failed: {}", stderr.trim());
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
    use std::os::unix::fs::PermissionsExt;

    /// The ffmpeg tests must not overlap: the stand-in ffmpeg goes first on
    /// the process-wide PATH.
    static FFMPEG: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A stand-in `ffmpeg` first on PATH that ignores its input and hangs,
    /// like the ffmpeg that stopped at the end of its input in the audio
    /// spike. It writes its pid to a file in its temp dir, which is removed
    /// and PATH restored when dropped.
    struct HungFfmpeg {
        dir: std::path::PathBuf,
        pid: std::path::PathBuf,
        path: Option<std::ffi::OsString>,
    }

    impl HungFfmpeg {
        /// The caller must hold `FFMPEG`.
        fn install() -> Self {
            let dir = std::env::temp_dir().join(format!("clipt-hung-ffmpeg-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let pid = dir.join("ffmpeg.pid");
            let script = dir.join("ffmpeg");
            std::fs::write(&script, format!("#!/bin/sh\necho $$ > '{}'\nexec sleep 600\n", pid.display())).unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
            let path = std::env::var_os("PATH");
            let mut dirs = vec![dir.clone()];
            dirs.extend(path.iter().flat_map(|p| std::env::split_paths(p)));
            // Safe: every test that runs ffmpeg takes `FFMPEG` first.
            unsafe { std::env::set_var("PATH", std::env::join_paths(dirs).unwrap()) };
            Self { dir, pid, path }
        }

        /// Wait until the stand-in has started, and return its pid.
        fn wait_started(&self) -> u32 {
            let start = Instant::now();
            while !self.pid.exists() {
                assert!(start.elapsed() < Duration::from_secs(10), "the stand-in ffmpeg never started");
                std::thread::sleep(Duration::from_millis(10));
            }
            std::fs::read_to_string(&self.pid).unwrap().trim().parse().unwrap()
        }
    }

    impl Drop for HungFfmpeg {
        fn drop(&mut self) {
            unsafe {
                match &self.path {
                    Some(p) => std::env::set_var("PATH", p),
                    None => std::env::remove_var("PATH"),
                }
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// The stand-in was killed, not left running.
    fn assert_gone(pid: u32) {
        // Signal 0 checks for the process without sending it anything.
        let alive = unsafe { libc::kill(pid as i32, 0) } == 0;
        assert!(!alive, "ffmpeg (pid {pid}) is still running");
    }

    #[test]
    fn finish_kills_a_hung_ffmpeg() {
        let _ffmpeg = FFMPEG.lock().unwrap_or_else(|e| e.into_inner());
        let fake = HungFfmpeg::install();
        let enc = Encoder::spawn(&Codec::X264, 16, 16, PixelOrder::Bgrx, 30, None, Path::new("/nonexistent/out.mp4")).unwrap();
        let pid = fake.wait_started();
        let start = Instant::now();
        let e = enc.finish().unwrap_err();
        assert!(start.elapsed() < Duration::from_secs(10), "finish waited too long");
        assert!(format!("{e:#}").contains("didn't finish"), "{e:#}");
        assert_gone(pid);
    }

    #[test]
    fn to_gif_kills_a_hung_ffmpeg_and_keeps_the_mp4() {
        let _ffmpeg = FFMPEG.lock().unwrap_or_else(|e| e.into_inner());
        let fake = HungFfmpeg::install();
        let mp4 = fake.dir.join("in.mp4");
        let gif = fake.dir.join("out.gif");
        std::fs::write(&mp4, b"a recording").unwrap();
        let start = Instant::now();
        let e = to_gif(&mp4, &gif, 10, Duration::from_secs(5)).unwrap_err();
        assert!(start.elapsed() < Duration::from_secs(10), "to_gif waited too long");
        assert!(format!("{e:#}").contains("didn't finish"), "{e:#}");
        assert_gone(fake.wait_started());
        assert!(mp4.exists(), "the MP4 must be kept");
    }

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
        let _ffmpeg = FFMPEG.lock().unwrap_or_else(|e| e.into_inner());
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
            voice: None,
            balance: Balance::Off,
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
    fn duck_graphs() {
        let s = |v: &[f32], separate, voice, balance| Sound {
            volumes: v.to_vec(),
            labels: v.iter().enumerate().map(|(i, _)| format!("S{i}")).collect(),
            separate,
            voice,
            balance,
        };
        let graph = |s: &Sound| s.args(false)[1].clone();
        let maps = |s: &Sound| s.args(false).windows(2).filter(|w| w[0] == "-map").map(|w| w[1].clone()).collect::<Vec<_>>();
        // The microphone is cleaned up into the mix and split into the
        // ducking keys; the other source is sidechained after its volume.
        let duck = s(&[0.7, 1.0], false, Some(1), Balance::Duck);
        assert_eq!(
            graph(&duck),
            format!(
                "[0:a:1]{RESAMPLE},{CLEAN_MIC},asplit=2[m1][k0];[m1]volume=1[v1];\
                 [0:a:0]{RESAMPLE},volume=0.7[w0];[w0][k0]{DUCK}[v0];[v0][v1]{MIX}:inputs=2[mix]"
            )
        );
        assert_eq!(maps(&duck), ["0:v", "[mix]"]);
        // Separate tracks stay as recorded: the microphone is split before
        // the clean-up, and the other source before its volume.
        let sep = s(&[0.7, 1.0], true, Some(1), Balance::Duck);
        assert_eq!(
            graph(&sep),
            format!(
                "[0:a:1]{RESAMPLE},asplit=2[wet1][t1];[wet1]{CLEAN_MIC},asplit=2[m1][k0];[m1]volume=1[v1];\
                 [0:a:0]{RESAMPLE},asplit=2[m0][t0];[m0]volume=0.7[w0];[w0][k0]{DUCK}[v0];[v0][v1]{MIX}:inputs=2[mix]"
            )
        );
        assert_eq!(maps(&sep), ["0:v", "[mix]", "[t0]", "[t1]"]);
        // `off` is the graph without any of that.
        let off = s(&[0.7, 1.0], false, Some(1), Balance::Off);
        assert!(!graph(&off).contains(CLEAN_MIC) && !graph(&off).contains(DUCK));
        // Without a microphone, `duck` changes nothing.
        let no_mic = s(&[0.7, 1.0], false, None, Balance::Duck);
        assert_eq!(no_mic.args(false), s(&[0.7, 1.0], false, None, Balance::Off).args(false));
        // Nor with the microphone alone.
        let mic_only = s(&[1.0], false, Some(0), Balance::Duck);
        assert_eq!(graph(&mic_only), format!("[0:a:0]{RESAMPLE}[v0]"));
        assert_eq!(maps(&mic_only), ["0:v", "[v0]"]);
        // Several other sources each get their own key.
        let three = s(&[1.0, 1.0, 1.0], false, Some(1), Balance::Duck);
        assert!(graph(&three).contains("[key]asplit=2[k0][k2]"), "{}", graph(&three));
        assert_eq!(Balance::parse("off").unwrap(), Balance::Off);
        assert_eq!(Balance::parse("duck").unwrap(), Balance::Duck);
        assert!(Balance::parse("smart").is_err(), "smart is a later phase");
    }

    /// Mean level (dBFS) of a lavfi `source` through `filter`, as
    /// volumedetect reports it.
    fn mean_volume(source: &str, filter: &str) -> f64 {
        let af = if filter.is_empty() { "volumedetect".to_string() } else { format!("{filter},volumedetect") };
        let out = Command::new("ffmpeg")
            .args(["-hide_banner", "-nostats", "-f", "lavfi", "-i", source, "-af", &af, "-f", "null", "-"])
            .stdin(Stdio::null())
            .output()
            .expect("running ffmpeg");
        assert!(out.status.success(), "ffmpeg failed: {}", String::from_utf8_lossy(&out.stderr));
        let err = String::from_utf8_lossy(&out.stderr);
        err.lines()
            .find_map(|l| l.split_once("mean_volume:").map(|(_, v)| v.split_whitespace().next().unwrap_or("").to_string()))
            .and_then(|v| v.parse().ok())
            .expect("volumedetect reported no mean level")
    }

    /// The clean-up chain leaves the voice at its recorded level: within 1 dB
    /// of the raw voice on speech-like sound at about -23 dBFS mean, the
    /// spike's voice level. With the spike's 4 dB make-up the same signal
    /// comes out 5.6 dB quieter; it measured 5.5 dB there.
    #[test]
    fn duck_cleanup_keeps_the_voice_level() {
        let _ffmpeg = FFMPEG.lock().unwrap_or_else(|e| e.into_inner());
        if Command::new("ffmpeg").arg("-version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_err() {
            return;
        }
        // A voiced vowel, modulated at a syllable rate with pauses: the
        // speech-shaped stand-in for the spike's reading.
        let voice = "aevalsrc='(0.152*sin(2*PI*130*t)+0.091*sin(2*PI*260*t)+0.053*sin(2*PI*520*t)+0.030*sin(2*PI*1040*t)+0.015*sin(2*PI*2080*t))*if(lt(mod(t,0.82),0.2),1,if(lt(mod(t,0.82),0.5),0.3,0.05))':s=48000:d=10";
        let raw = mean_volume(voice, "");
        let cleaned = mean_volume(voice, CLEAN_MIC);
        assert!((raw - cleaned).abs() < 1.0, "cleaned voice is {:.2} dB from the raw one ({raw:.1} -> {cleaned:.1} dBFS)", cleaned - raw);
    }

    #[test]
    fn codec_preferences() {
        assert_eq!(candidates("x264").unwrap(), vec![Codec::X264]);
        assert_eq!(candidates("nvenc").unwrap(), vec![Codec::Nvenc]);
        assert!(candidates("auto").unwrap().ends_with(&[Codec::Nvenc, Codec::X264]));
        assert!(candidates("av1").is_err());
    }
}
