//! Sound for recordings, read from the sound server through libpulse-simple.
//!
//! libpulse-simple is loaded at runtime (dlopen) like libpipewire, so clipt
//! still starts without it. It works the same on Wayland, X11 and in the
//! Flatpak: PipeWire desktops serve it through pipewire-pulse, PulseAudio
//! natively.
//!
//! Chunks are stamped by a sample clock rather than by when each read
//! returns: the server hands audio over in its own blocks (PipeWire's are
//! 21.3 ms against our 20 ms reads), so read times jitter by ±15 ms while the
//! sample count doesn't. See docs/audio-plan.md for the measurements.

use anyhow::{Result, bail};
use std::collections::VecDeque;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

pub const RATE: u32 = 48_000;
pub const CHANNELS: u32 = 2;
/// 16-bit samples.
pub const BYTES_PER_FRAME: u32 = 2 * CHANNELS;
/// 20 ms.
const CHUNK_FRAMES: u32 = RATE / 50;

/// What to record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// What the computer is playing: the default output's monitor.
    System,
}

impl Source {
    /// A `record_audio` value or `--audio=` argument: `None` for "none".
    pub fn parse(s: &str) -> Result<Option<Self>> {
        Ok(match s {
            "none" => None,
            "system" => Some(Self::System),
            "mic" | "both" => bail!("recording '{s}' sound isn't supported yet (none or system)"),
            other => bail!("unknown audio source '{other}' (none or system)"),
        })
    }

    fn device(self) -> &'static CStr {
        match self {
            Self::System => c"@DEFAULT_MONITOR@",
        }
    }
}

/// 20 ms of interleaved 16-bit stereo, and when its first sample was heard.
pub struct Chunk {
    pub at: Instant,
    pub data: Vec<u8>,
}

// ---- libpulse-simple, loaded at runtime ------------------------------------

#[repr(C)]
struct SampleSpec {
    format: c_int,
    rate: u32,
    channels: u8,
}

#[repr(C)]
struct BufferAttr {
    maxlength: u32,
    tlength: u32,
    prebuf: u32,
    minreq: u32,
    fragsize: u32,
}

const PA_SAMPLE_S16LE: c_int = 3;
const PA_STREAM_RECORD: c_int = 2;
/// "Server default" in a pa_buffer_attr field.
const DEFAULT: u32 = u32::MAX;

type P = *mut c_void;

struct Lib {
    new: unsafe extern "C" fn(
        *const c_char,
        *const c_char,
        c_int,
        *const c_char,
        *const c_char,
        *const SampleSpec,
        P,
        *const BufferAttr,
        *mut c_int,
    ) -> P,
    read: unsafe extern "C" fn(P, *mut c_void, usize, *mut c_int) -> c_int,
    get_latency: unsafe extern "C" fn(P, *mut c_int) -> u64,
    free: unsafe extern "C" fn(P),
    strerror: unsafe extern "C" fn(c_int) -> *const c_char,
}

fn lib() -> Result<&'static Lib> {
    static LIB: OnceLock<Result<Lib, String>> = OnceLock::new();
    LIB.get_or_init(load).as_ref().map_err(|e| anyhow::anyhow!("{e}"))
}

/// A symbol address as the function pointer type `F`.
///
/// # Safety
/// `p` must be a function whose signature is `F`.
unsafe fn fn_ptr<F: Copy>(p: *mut c_void) -> F {
    assert_eq!(std::mem::size_of::<F>(), std::mem::size_of::<*mut c_void>());
    unsafe { std::mem::transmute_copy(&p) }
}

fn load() -> Result<Lib, String> {
    let h = unsafe { libc::dlopen(c"libpulse-simple.so.0".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
    if h.is_null() {
        return Err("recording sound needs libpulse-simple.so.0 (install libpulse, or pipewire-pulse on PipeWire desktops)".into());
    }
    macro_rules! sym {
        ($name:literal) => {{
            // dlsym on the handle also searches its dependencies, where
            // pa_strerror (libpulse) lives.
            let p = unsafe { libc::dlsym(h, concat!($name, "\0").as_ptr().cast()) };
            if p.is_null() {
                return Err(format!("libpulse-simple has no {}", $name));
            }
            // SAFETY: the symbol is the C function named, whose signature
            // matches the field it's stored in (from pulse/simple.h and
            // pulse/error.h).
            unsafe { fn_ptr(p) }
        }};
    }
    Ok(Lib {
        new: sym!("pa_simple_new"),
        read: sym!("pa_simple_read"),
        get_latency: sym!("pa_simple_get_latency"),
        free: sym!("pa_simple_free"),
        strerror: sym!("pa_strerror"),
    })
}

/// Fails, saying what to install, if libpulse-simple can't be loaded.
pub fn check() -> Result<()> {
    lib().map(drop)
}

fn error_text(lib: &Lib, err: c_int) -> String {
    let p = unsafe { (lib.strerror)(err) };
    if p.is_null() { format!("error {err}") } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() }
}

/// An open recording stream on the sound server.
pub struct Capture {
    lib: &'static Lib,
    s: P,
}

// SAFETY: a pa_simple is used from one thread at a time; it moves to the
// reading thread once and stays there.
unsafe impl Send for Capture {}

impl Drop for Capture {
    fn drop(&mut self) {
        unsafe { (self.lib.free)(self.s) };
    }
}

/// Connect to the sound server and start recording `src`.
pub fn open(src: Source) -> Result<Capture> {
    let lib = lib()?;
    let spec = SampleSpec { format: PA_SAMPLE_S16LE, rate: RATE, channels: CHANNELS as u8 };
    // fragsize: without it the server hands over much larger fragments,
    // seconds' worth on PulseAudio.
    let attr =
        BufferAttr { maxlength: DEFAULT, tlength: DEFAULT, prebuf: DEFAULT, minreq: DEFAULT, fragsize: CHUNK_FRAMES * BYTES_PER_FRAME };
    let mut err: c_int = 0;
    let s = unsafe {
        (lib.new)(
            std::ptr::null(),
            c"clipt".as_ptr(),
            PA_STREAM_RECORD,
            src.device().as_ptr(),
            c"Recording".as_ptr(),
            &spec,
            std::ptr::null_mut(),
            &attr,
            &mut err,
        )
    };
    if s.is_null() {
        bail!("can't record sound: {} (is a PulseAudio or pipewire-pulse server running?)", error_text(lib, err));
    }
    Ok(Capture { lib, s })
}

impl Capture {
    /// Read and stamp chunks, handing each to `sink`, until it returns false.
    pub fn run(self, mut sink: impl FnMut(Chunk) -> bool) -> Result<()> {
        let mut clock = SampleClock::new(Instant::now());
        let bytes = (CHUNK_FRAMES * BYTES_PER_FRAME) as usize;
        loop {
            let mut data = vec![0u8; bytes];
            let mut err: c_int = 0;
            if unsafe { (self.lib.read)(self.s, data.as_mut_ptr().cast(), bytes, &mut err) } < 0 {
                bail!("reading sound: {}", error_text(self.lib, err));
            }
            let now = Instant::now();
            let latency = unsafe { (self.lib.get_latency)(self.s, &mut err) };
            let at = clock.stamp(now, Duration::from_micros(latency), CHUNK_FRAMES as u64);
            if !sink(Chunk { at, data }) {
                return Ok(());
            }
        }
    }
}

/// Stamps chunks as an anchor plus samples read so far. Each read gives an
/// estimate of the anchor (when it returned, minus the latency and what had
/// been read); reads only ever return late, so the earliest estimate is the
/// best. Taking it over a sliding window lets it follow slow clock drift.
struct SampleClock {
    base: Instant,
    /// Frames read before the current chunk.
    frames: u64,
    /// (when, anchor estimate in seconds after `base`), anchors increasing:
    /// the front is the earliest within the window.
    window: VecDeque<(Instant, f64)>,
}

const WINDOW: Duration = Duration::from_secs(10);

impl SampleClock {
    fn new(base: Instant) -> Self {
        Self { base, frames: 0, window: VecDeque::new() }
    }

    /// When the first sample of a chunk of `frames` frames, whose read
    /// returned at `now` with `latency` still to go, was heard.
    fn stamp(&mut self, now: Instant, latency: Duration, frames: u64) -> Instant {
        let since_base =
            |t: Instant| t.saturating_duration_since(self.base).as_secs_f64() - self.base.saturating_duration_since(t).as_secs_f64();
        let end = since_base(now) - latency.as_secs_f64();
        let read = (self.frames + frames) as f64 / RATE as f64;
        let anchor = end - read;
        while self.window.back().is_some_and(|&(_, a)| a >= anchor) {
            self.window.pop_back();
        }
        self.window.push_back((now, anchor));
        while self.window.front().is_some_and(|&(t, _)| now.duration_since(t) > WINDOW) {
            self.window.pop_front();
        }
        let anchor = self.window.front().unwrap().1;
        let start = anchor + self.frames as f64 / RATE as f64;
        self.frames += frames;
        if start >= 0.0 {
            self.base + Duration::from_secs_f64(start)
        } else {
            self.base.checked_sub(Duration::from_secs_f64(-start)).unwrap_or(self.base)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources() {
        assert_eq!(Source::parse("none").unwrap(), None);
        assert_eq!(Source::parse("system").unwrap(), Some(Source::System));
        assert!(Source::parse("mic").is_err());
        assert!(Source::parse("loud").is_err());
    }

    /// Reads that return late by a varying amount (as PipeWire's blocks make
    /// them) still give evenly spaced stamps once the earliest one is seen.
    #[test]
    fn stamps_ignore_late_reads() {
        let base = Instant::now();
        let mut c = SampleClock::new(base);
        let chunk = Duration::from_millis(20);
        let lat = Duration::from_millis(5);
        // Sample 0 was heard at base + 100 ms; read k returns when its chunk
        // ends, plus the latency, plus up to 15 ms of lateness.
        let heard = base + Duration::from_millis(100);
        let mut stamps = Vec::new();
        for k in 0..20u32 {
            let late = Duration::from_millis([7, 0, 15, 3, 11][k as usize % 5]);
            let ret = heard + chunk * (k + 1) + lat + late;
            stamps.push(c.stamp(ret, lat, CHUNK_FRAMES as u64));
        }
        // From the second read on (the first on-time one), stamps are exact.
        for (k, s) in stamps.iter().enumerate().skip(1) {
            let want = heard + chunk * k as u32;
            let off = s.duration_since(want).as_secs_f64() - want.saturating_duration_since(*s).as_secs_f64();
            assert!(off.abs() < 0.0005, "chunk {k}: {:.1} ms off", off * 1000.0);
        }
    }

    /// The anchor follows a clock that drifts, rather than sticking to an
    /// estimate from long ago.
    #[test]
    fn window_follows_drift() {
        let base = Instant::now();
        let mut c = SampleClock::new(base);
        // The sound card runs 0.1 % slow: each 20 ms chunk takes 20.02 ms.
        let per = Duration::from_micros(20_020);
        let mut last = base;
        for k in 0..2000u32 {
            let ret = base + per * (k + 1);
            last = c.stamp(ret, Duration::ZERO, CHUNK_FRAMES as u64);
        }
        // Chunk 1999 started at 1999 × 20.02 ms; a fixed anchor would say
        // 1999 × 20 ms, 40 ms early. Within the 10 s window the error is
        // at most the drift over the window (10 ms).
        let want = base + per * 1999;
        let off = (last.duration_since(want).as_secs_f64() - want.saturating_duration_since(last).as_secs_f64()).abs();
        assert!(off < 0.011, "{:.1} ms off", off * 1000.0);
    }
}
