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
//!
//! Listing the server's devices (`clipt list sound`) needs libpulse's full,
//! asynchronous API; only the first, stable fields of its records are read.

use anyhow::{Result, bail};
use std::collections::VecDeque;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

pub const RATE: u32 = 48_000;
pub const CHANNELS: u32 = 2;
/// 16-bit samples.
pub const BYTES_PER_FRAME: u32 = 2 * CHANNELS;
/// 20 ms.
const CHUNK_FRAMES: u32 = RATE / 50;

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

    fn server_name(&self) -> CString {
        let n = match &self.device {
            Device::System => "@DEFAULT_MONITOR@",
            Device::Mic => "@DEFAULT_SOURCE@",
            Device::Named(n) => n,
        };
        CString::new(n).unwrap_or_default()
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
    // The asynchronous API, for listing devices.
    mainloop_new: unsafe extern "C" fn() -> P,
    mainloop_get_api: unsafe extern "C" fn(P) -> P,
    mainloop_iterate: unsafe extern "C" fn(P, c_int, *mut c_int) -> c_int,
    mainloop_free: unsafe extern "C" fn(P),
    context_new: unsafe extern "C" fn(P, *const c_char) -> P,
    context_connect: unsafe extern "C" fn(P, *const c_char, c_int, P) -> c_int,
    context_get_state: unsafe extern "C" fn(P) -> c_int,
    context_disconnect: unsafe extern "C" fn(P),
    context_unref: unsafe extern "C" fn(P),
    context_get_server_info: unsafe extern "C" fn(P, ServerInfoCb, P) -> P,
    context_get_source_info_list: unsafe extern "C" fn(P, SourceInfoCb, P) -> P,
    operation_get_state: unsafe extern "C" fn(P) -> c_int,
    operation_unref: unsafe extern "C" fn(P),
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
            // pa_strerror and the asynchronous API (libpulse) live.
            let p = unsafe { libc::dlsym(h, concat!($name, "\0").as_ptr().cast()) };
            if p.is_null() {
                return Err(format!("libpulse-simple has no {}", $name));
            }
            // SAFETY: the symbol is the C function named, whose signature
            // matches the field it's stored in (from pulse/simple.h,
            // error.h, mainloop.h, context.h, introspect.h and operation.h).
            unsafe { fn_ptr(p) }
        }};
    }
    Ok(Lib {
        new: sym!("pa_simple_new"),
        read: sym!("pa_simple_read"),
        get_latency: sym!("pa_simple_get_latency"),
        free: sym!("pa_simple_free"),
        strerror: sym!("pa_strerror"),
        mainloop_new: sym!("pa_mainloop_new"),
        mainloop_get_api: sym!("pa_mainloop_get_api"),
        mainloop_iterate: sym!("pa_mainloop_iterate"),
        mainloop_free: sym!("pa_mainloop_free"),
        context_new: sym!("pa_context_new"),
        context_connect: sym!("pa_context_connect"),
        context_get_state: sym!("pa_context_get_state"),
        context_disconnect: sym!("pa_context_disconnect"),
        context_unref: sym!("pa_context_unref"),
        context_get_server_info: sym!("pa_context_get_server_info"),
        context_get_source_info_list: sym!("pa_context_get_source_info_list"),
        operation_get_state: sym!("pa_operation_get_state"),
        operation_unref: sym!("pa_operation_unref"),
    })
}

/// Fails, saying what's wrong, if these sources can't be recorded: no
/// libpulse-simple, or a device the server doesn't have. Devices are checked
/// against the server's list because pipewire-pulse, unlike PulseAudio,
/// accepts a stream from a device that doesn't exist and never delivers it.
pub fn check(sources: &[Source]) -> Result<()> {
    lib()?;
    let named: Vec<&str> = sources
        .iter()
        .filter_map(|s| match &s.device {
            Device::Named(n) => Some(n.as_str()),
            _ => None,
        })
        .collect();
    if !named.is_empty() {
        let have = devices()?;
        if let Some(n) = named.iter().find(|n| !have.iter().any(|d| d.name == **n)) {
            bail!("there's no sound device '{n}' (clipt list sound shows them)");
        }
    }
    Ok(())
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

/// pa_strerror's code for a device that doesn't exist.
const PA_ERR_NOENTITY: c_int = 5;

/// Connect to the sound server and start recording `src`.
pub fn open(src: &Source) -> Result<Capture> {
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
            src.server_name().as_ptr(),
            c"Recording".as_ptr(),
            &spec,
            std::ptr::null_mut(),
            &attr,
            &mut err,
        )
    };
    if s.is_null() {
        if let (PA_ERR_NOENTITY, Device::Named(n)) = (err, &src.device) {
            bail!("there's no sound device '{n}' (clipt list sound shows them)");
        }
        bail!("can't record sound from {}: {} (is a PulseAudio or pipewire-pulse server running?)", src.label(), error_text(lib, err));
    }
    Ok(Capture { lib, s })
}

// ---- listing devices --------------------------------------------------------

/// The start of pa_server_info (introspect.h); later fields aren't read.
#[repr(C)]
struct ServerInfo {
    user_name: *const c_char,
    host_name: *const c_char,
    server_version: *const c_char,
    server_name: *const c_char,
    sample_spec: SampleSpec,
    default_sink_name: *const c_char,
    default_source_name: *const c_char,
}

/// The start of pa_source_info (introspect.h); later fields aren't read.
#[repr(C)]
struct SourceInfo {
    name: *const c_char,
    index: u32,
    description: *const c_char,
}

type ServerInfoCb = unsafe extern "C" fn(P, *const ServerInfo, P);
type SourceInfoCb = unsafe extern "C" fn(P, *const SourceInfo, c_int, P);

/// A recordable source on the server.
pub struct DeviceInfo {
    pub name: String,
    pub description: String,
    /// What `system` records (the default output's monitor).
    pub is_system: bool,
    /// What `mic` records (the default input).
    pub is_mic: bool,
}

#[derive(Default)]
struct Listing {
    default_sink: String,
    default_source: String,
    sources: Vec<(String, String)>,
}

fn text(p: *const c_char) -> String {
    if p.is_null() { String::new() } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() }
}

unsafe extern "C" fn on_server_info(_: P, i: *const ServerInfo, ud: P) {
    // SAFETY: ud is the &mut Listing passed with the request, and i a valid
    // record for the duration of the call (or null).
    let (l, i) = unsafe { (&mut *(ud as *mut Listing), i.as_ref()) };
    if let Some(i) = i {
        l.default_sink = text(i.default_sink_name);
        l.default_source = text(i.default_source_name);
    }
}

unsafe extern "C" fn on_source_info(_: P, i: *const SourceInfo, eol: c_int, ud: P) {
    // SAFETY: as in on_server_info.
    let (l, i) = unsafe { (&mut *(ud as *mut Listing), i.as_ref()) };
    if let (0, Some(i)) = (eol, i) {
        l.sources.push((text(i.name), text(i.description)));
    }
}

/// The sources the server offers, with the ones `system` and `mic` mean marked.
pub fn devices() -> Result<Vec<DeviceInfo>> {
    const READY: c_int = 4;
    const FAILED: c_int = 5;
    const TERMINATED: c_int = 6;
    const OP_RUNNING: c_int = 0;
    let lib = lib()?;
    let mut l = Listing::default();
    unsafe {
        let ml = (lib.mainloop_new)();
        let ctx = (lib.context_new)((lib.mainloop_get_api)(ml), c"clipt".as_ptr());
        let result = (|| {
            if (lib.context_connect)(ctx, std::ptr::null(), 0, std::ptr::null_mut()) < 0 {
                bail!("can't reach the sound server (is a PulseAudio or pipewire-pulse server running?)");
            }
            loop {
                match (lib.context_get_state)(ctx) {
                    READY => break,
                    FAILED | TERMINATED => bail!("can't reach the sound server (is a PulseAudio or pipewire-pulse server running?)"),
                    _ => {
                        (lib.mainloop_iterate)(ml, 1, std::ptr::null_mut());
                    }
                }
            }
            let ud = &mut l as *mut Listing as P;
            for op in [(lib.context_get_server_info)(ctx, on_server_info, ud), (lib.context_get_source_info_list)(ctx, on_source_info, ud)]
            {
                if op.is_null() {
                    bail!("the sound server refused to list its devices");
                }
                while (lib.operation_get_state)(op) == OP_RUNNING {
                    if (lib.mainloop_iterate)(ml, 1, std::ptr::null_mut()) < 0 {
                        break;
                    }
                }
                (lib.operation_unref)(op);
            }
            Ok(())
        })();
        (lib.context_disconnect)(ctx);
        (lib.context_unref)(ctx);
        (lib.mainloop_free)(ml);
        result?;
    }
    let system = format!("{}.monitor", l.default_sink);
    Ok(l.sources
        .into_iter()
        .map(|(name, description)| DeviceInfo { is_system: name == system, is_mic: name == l.default_source, name, description })
        .collect())
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

    #[test]
    fn listing_records_match_libpulse() {
        // pa_server_info: four strings, a 12-byte sample spec, then the
        // default sink and source names (64-bit layout).
        if std::mem::size_of::<usize>() == 8 {
            assert_eq!(std::mem::offset_of!(ServerInfo, default_sink_name), 48);
            assert_eq!(std::mem::offset_of!(ServerInfo, default_source_name), 56);
            assert_eq!(std::mem::offset_of!(SourceInfo, description), 16);
        }
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
