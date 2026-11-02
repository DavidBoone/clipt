//! How clearly the voice comes through the rest of the sound, for
//! `--audio-balance=smart`. ffmpeg sends back the voice and each other
//! source as they go into the mix (after the clean-up and the ducking), as
//! 48 kHz float samples, voice first; this measures them as they arrive and
//! reports at the stop.
//!
//! The measurement is the phase 4 spike's (docs/audio-phase4-spike.md), so
//! its numbers carry over:
//! - **Speech Intelligibility Index**, octave bands at 250 Hz–8 kHz, per
//!   400 ms frame: SNRᵢ = voiceᵢ − restᵢ in dB, SII = Σ Iᵢ·clamp((SNRᵢ +
//!   15)/30, 0, 1). Only frames within 15 dB of the voice's 90th-percentile
//!   level count as speech; the 10th percentile of those is the result.
//! - **Leakage**: through speakers the microphone hears the rest too, which
//!   makes the voice look clearer than it is. In the voice's pauses (the
//!   quietest quarter of 1 s windows with other sound playing) the leak
//!   correlates with the rest at some delay up to 250 ms; above 0.3 in any of
//!   them means the microphone hears it.

use std::fmt;

pub const RATE: f32 = 48_000.0;
/// Octave band centres and their importance (ANSI S3.5 octave-band weights).
const BANDS: [f32; 6] = [250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0];
const WEIGHTS: [f32; 6] = [0.062, 0.167, 0.237, 0.265, 0.214, 0.055];
const FRAME: usize = 19_200; // 400 ms
const FFT: usize = 32_768;
/// Leakage is checked at 8 kHz, over 1 s windows, at delays up to 250 ms.
const DECIMATE: usize = 6;
const WINDOW: usize = 8_000;
const MAX_LAG: usize = 2_000;
/// A window with the rest quieter than this has nothing to leak (quiet
/// music is still around -55 dBFS once ducked).
const REST_FLOOR_DB: f32 = -60.0;
const LEAK_THRESHOLD: f32 = 0.3;
/// The target, and a safe margin, from the spike.
const TARGET: f32 = 0.75;
const GOOD: f32 = 0.90;

/// A low-pass biquad section (RBJ cookbook), transposed direct form II.
#[derive(Clone, Copy)]
struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    z: [f32; 2],
}

impl Biquad {
    fn low_pass(f: f32, q: f32) -> Self {
        let w = std::f32::consts::TAU * f / RATE;
        let (sin, cos) = w.sin_cos();
        let alpha = sin / (2.0 * q);
        let a0 = 1.0 + alpha;
        let b = [(1.0 - cos) / 2.0, 1.0 - cos, (1.0 - cos) / 2.0];
        Self { b: b.map(|x| x / a0), a: [-2.0 * cos / a0, (1.0 - alpha) / a0], z: [0.0; 2] }
    }

    fn run(&mut self, x: f32) -> f32 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// A fourth-order Butterworth low-pass, for decimating to 8 kHz.
#[derive(Clone, Copy)]
struct LowPass([Biquad; 2]);

impl LowPass {
    fn new(f: f32) -> Self {
        Self([Biquad::low_pass(f, 0.541_196), Biquad::low_pass(f, 1.306_563)])
    }

    fn run(&mut self, x: f32) -> f32 {
        let y = self.0[0].run(x);
        self.0[1].run(y)
    }
}

/// An in-place radix-2 FFT. `re` and `im` are a power of two long.
fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let (ws, wc) = (-std::f64::consts::TAU / len as f64).sin_cos();
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0, 0.0);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let (tr, ti) = (re[b] * cr - im[b] * ci, re[b] * ci + im[b] * cr);
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                (cr, ci) = (cr * wc - ci * ws, cr * ws + ci * wc);
            }
        }
        len <<= 1;
    }
}

/// Each octave band's power in the voice and the rest of one frame: a Hann
/// window, an FFT (the voice as the real part, the rest as the imaginary,
/// separated after), and the power summed per band with edges at fc/√2
/// and fc·√2. Sharp edges matter: the voice is far stronger low down than
/// at 4 and 8 kHz, and gentle filter skirts let that into the high bands
/// (fourth-order Butterworth bands read 0.12 high against the spike).
fn band_powers(voice: &[f32], rest: &[f32]) -> ([f64; 6], [f64; 6]) {
    let n = FFT;
    let (mut re, mut im) = (vec![0.0f64; n], vec![0.0f64; n]);
    for (i, (&v, &r)) in voice.iter().zip(rest).enumerate() {
        let w = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / voice.len() as f64).cos();
        re[i] = v as f64 * w;
        im[i] = r as f64 * w;
    }
    fft(&mut re, &mut im);
    let (mut pv, mut pr) = ([0.0; 6], [0.0; 6]);
    for (b, &fc) in BANDS.iter().enumerate() {
        let lo = (fc / std::f32::consts::SQRT_2 / RATE * n as f32).ceil() as usize;
        let hi = (fc * std::f32::consts::SQRT_2 / RATE * n as f32).ceil() as usize;
        for k in lo..hi.min(n / 2) {
            let (zr, zi, cr, ci) = (re[k], im[k], re[n - k], -im[n - k]);
            // X = (Z[k] + conj Z[n-k]) / 2, Y = (Z[k] - conj Z[n-k]) / 2i.
            let (xr, xi) = ((zr + cr) / 2.0, (zi + ci) / 2.0);
            let (yr, yi) = ((zi - ci) / 2.0, -(zr - cr) / 2.0);
            pv[b] += xr * xr + xi * xi;
            pr[b] += yr * yr + yi * yi;
        }
    }
    (pv, pr)
}

fn db(power: f64) -> f32 {
    (10.0 * power.max(1e-15).log10()) as f32
}

/// One 400 ms frame: broadband and per-band levels in dB.
#[derive(Clone, Copy, Debug)]
struct Frame {
    voice: f32,
    rest: f32,
    voice_bands: [f32; 6],
    rest_bands: [f32; 6],
}

impl Frame {
    fn sii(&self) -> f32 {
        (0..6).map(|i| WEIGHTS[i] * ((self.voice_bands[i] - self.rest_bands[i] + 15.0) / 30.0).clamp(0.0, 1.0)).sum()
    }
}

/// One 1 s leakage window: the voice's and the rest's level, and the
/// highest correlation between them at any delay.
#[derive(Clone, Copy, Debug)]
struct Window {
    voice: f32,
    rest: f32,
    peak: f32,
}

pub struct Analyzer {
    channels: usize,
    /// The current frame's voice and rest.
    voice: Vec<f32>,
    rest_frame: Vec<f32>,
    frames: Vec<Frame>,
    decimators: [LowPass; 2],
    phase: usize,
    /// The voice's current window and the rest from `MAX_LAG` before it.
    mic: Vec<f32>,
    rest: Vec<f32>,
    windows: Vec<Window>,
}

impl Analyzer {
    /// `channels`: the voice, then each other source.
    pub fn new(channels: usize) -> Self {
        Self {
            channels,
            voice: Vec::with_capacity(FRAME),
            rest_frame: Vec::with_capacity(FRAME),
            frames: Vec::new(),
            decimators: [LowPass::new(3_800.0); 2],
            phase: 0,
            mic: Vec::with_capacity(WINDOW),
            rest: vec![0.0; MAX_LAG],
            windows: Vec::new(),
        }
    }

    /// Interleaved samples, `channels` to a frame.
    pub fn push(&mut self, samples: &[f32]) {
        for f in samples.chunks_exact(self.channels) {
            let v = f[0];
            let r: f32 = f[1..].iter().sum();
            self.sample(v, r);
        }
    }

    fn sample(&mut self, v: f32, r: f32) {
        self.voice.push(v);
        self.rest_frame.push(r);
        if self.voice.len() == FRAME {
            let level = |x: &[f32]| db(x.iter().map(|&s| (s * s) as f64).sum::<f64>() / FRAME as f64);
            let (pv, pr) = band_powers(&self.voice, &self.rest_frame);
            self.frames.push(Frame {
                voice: level(&self.voice),
                rest: level(&self.rest_frame),
                voice_bands: pv.map(db),
                rest_bands: pr.map(db),
            });
            self.voice.clear();
            self.rest_frame.clear();
        }

        let dv = self.decimators[0].run(v);
        let dr = self.decimators[1].run(r);
        self.phase += 1;
        if self.phase == DECIMATE {
            self.phase = 0;
            self.mic.push(dv);
            self.rest.push(dr);
            if self.mic.len() == WINDOW {
                self.windows.push(window(&self.mic, &self.rest));
                self.mic.clear();
                self.rest.drain(..WINDOW);
            }
        }
    }

    /// What the recording sounded like. `None` without a full frame.
    pub fn report(self) -> Option<Report> {
        let debug = std::env::var_os("CLIPT_DEBUG").is_some();
        if self.frames.is_empty() {
            return None;
        }
        let mut levels: Vec<f32> = self.frames.iter().map(|f| f.voice).collect();
        levels.sort_by(f32::total_cmp);
        let p90 = levels[(levels.len() - 1) * 9 / 10];
        let speech: Vec<&Frame> = self.frames.iter().filter(|f| f.voice >= p90 - 15.0 && f.voice > -70.0).collect();
        // Four seconds of speech, or there is nothing to judge.
        if speech.len() < 10 {
            return Some(Report::TooLittleSpeech);
        }
        let mut sii: Vec<f32> = speech.iter().map(|f| f.sii()).collect();
        sii.sort_by(f32::total_cmp);
        let mut gaps: Vec<f32> = speech.iter().map(|f| f.voice - f.rest).collect();
        gaps.sort_by(f32::total_cmp);
        let clarity = sii[sii.len() / 10];
        let gap = gaps[gaps.len() / 2];

        let mut with_rest: Vec<&Window> = self.windows.iter().filter(|w| w.rest > REST_FLOOR_DB).collect();
        with_rest.sort_by(|a, b| a.voice.total_cmp(&b.voice));
        let quiet = &with_rest[..with_rest.len().div_ceil(4)];
        let peak = quiet.iter().map(|w| w.peak).fold(0.0, f32::max);
        let leak = if with_rest.is_empty() {
            Leak::NothingPlaying
        } else if quiet.len() < 3 {
            Leak::Unchecked
        } else if peak > LEAK_THRESHOLD {
            Leak::Heard
        } else {
            Leak::NotHeard
        };
        if debug {
            eprintln!(
                "clipt: clarity: {} of {} frames speech (90th percentile {p90:.1} dB); SII 10th {clarity:.2}, median {:.2}; gap {gap:.1} dB; \
                 leak peak {peak:.2} over {} quiet of {} windows with sound",
                speech.len(),
                self.frames.len(),
                sii[sii.len() / 2],
                quiet.len(),
                with_rest.len()
            );
        }
        Some(Report::Measured { clarity, gap, leak })
    }
}

/// The highest normalised correlation of `mic` (one window) with `rest`
/// (`MAX_LAG` samples longer, ending with the same sample) at delays of 0 to
/// `MAX_LAG` samples.
fn window(mic: &[f32], rest: &[f32]) -> Window {
    let n = mic.len();
    let mic_power: f64 = mic.iter().map(|&x| (x * x) as f64).sum();
    // Energy of each delayed stretch of `rest`, by prefix sums.
    let mut prefix = Vec::with_capacity(rest.len() + 1);
    prefix.push(0.0f64);
    for &x in rest {
        prefix.push(prefix.last().unwrap() + (x * x) as f64);
    }
    let mut peak = 0.0f32;
    if mic_power > 0.0 {
        for lag in 0..=MAX_LAG {
            let start = MAX_LAG - lag;
            let r = &rest[start..start + n];
            let rest_power = prefix[start + n] - prefix[start];
            if rest_power <= 0.0 {
                continue;
            }
            let c = (dot(mic, r) / (mic_power * rest_power).sqrt()) as f32;
            peak = peak.max(c);
        }
    }
    let level = |p: f64| db(p / n as f64);
    Window { voice: level(mic_power), rest: level(prefix[rest.len()] - prefix[MAX_LAG]), peak }
}

/// A dot product in eight lanes, so it vectorises.
fn dot(a: &[f32], b: &[f32]) -> f64 {
    let mut acc = [0.0f32; 8];
    let (ac, ar) = a.split_at(a.len() / 8 * 8);
    let (bc, br) = b.split_at(ac.len());
    for (x, y) in ac.as_chunks::<8>().0.iter().zip(bc.as_chunks::<8>().0) {
        for i in 0..8 {
            acc[i] += x[i] * y[i];
        }
    }
    let tail: f32 = ar.iter().zip(br).map(|(x, y)| x * y).sum();
    acc.iter().map(|&x| x as f64).sum::<f64>() + tail as f64
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Leak {
    /// Nothing else was playing, so there was nothing to hear.
    NothingPlaying,
    /// Too few pauses with other sound to tell.
    Unchecked,
    NotHeard,
    /// The microphone hears the other sound: the clarity can't be trusted.
    Heard,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Report {
    TooLittleSpeech,
    /// `clarity`: the 10th-percentile SII; `gap`: the median dB the voice is
    /// over the rest while speaking.
    Measured {
        clarity: f32,
        gap: f32,
        leak: Leak,
    },
}

impl Report {
    /// For the recording's summary line and notification.
    pub fn summary(&self) -> String {
        match *self {
            Report::TooLittleSpeech => "too little speech to judge".into(),
            Report::Measured { gap, leak: Leak::Heard, .. } => format!("voice {gap:.0} dB over the rest"),
            Report::Measured { clarity, .. } => format!("speech clarity {clarity:.2}"),
        }
    }
}

/// The full sentence printed at the stop.
impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Report::TooLittleSpeech => write!(f, "speech clarity: too little speech to judge"),
            Report::Measured { gap, leak: Leak::Heard, .. } => write!(
                f,
                "the microphone hears the other sound (speakers?), so speech clarity can't be measured; \
                 the voice is {gap:.0} dB over the rest (15 dB is enough, 20 with singing or talking under it). \
                 Headphones give a reliable result"
            ),
            Report::Measured { clarity, leak, .. } => {
                let verdict = if clarity >= GOOD {
                    "very good"
                } else if clarity >= TARGET {
                    "good"
                } else if clarity >= 0.45 {
                    "fair: the other sound covers some words"
                } else {
                    "poor: the other sound covers much of the voice"
                };
                write!(f, "speech clarity {clarity:.2}, {verdict}")?;
                if leak == Leak::Unchecked {
                    write!(f, " (too few pauses to check the microphone doesn't hear the speakers)")?;
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic white noise in -1..1.
    struct Noise(u64);

    impl Noise {
        fn next(&mut self) -> f32 {
            self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            ((self.0 >> 40) as f32 / (1u64 << 23) as f32) - 1.0
        }
    }

    /// Syllables: on for 250 ms, quieter for 150 ms, and a 1.2 s pause
    /// every 4 s, so there are frames to skip and windows to check.
    fn envelope(i: usize) -> f32 {
        let t = i as f32 / RATE;
        if t % 4.0 > 2.8 {
            0.0
        } else if t % 0.4 < 0.25 {
            1.0
        } else {
            0.3
        }
    }

    /// `secs` of a modulated noise voice and a steady noise rest `gap` dB
    /// under it; `leak` dB under the voice, the rest also reaches the voice
    /// 30 ms late, as through speakers.
    fn run(secs: usize, gap: f32, leak: Option<f32>) -> Report {
        let mut a = Analyzer::new(2);
        let (mut nv, mut nr, mut nh) = (Noise(1), Noise(2), Noise(4));
        let rest_gain = 0.1 * 10f32.powf(-gap / 20.0);
        let delay = (0.030 * RATE) as usize;
        let mut history = vec![0.0f32; delay];
        let mut buf = Vec::new();
        for i in 0..secs * RATE as usize {
            let r = nr.next() * rest_gain;
            history.push(r);
            let late = history.remove(0);
            // The microphone's own noise, 55 dB under the voice.
            let hiss = nh.next() * 0.1 * 10f32.powf(-55.0 / 20.0);
            let v = nv.next() * 0.1 * envelope(i) + hiss + leak.map_or(0.0, |l| late / rest_gain * 0.1 * 10f32.powf(-l / 20.0));
            buf.extend([v, r]);
        }
        a.push(&buf);
        a.report().unwrap()
    }

    fn clarity(r: Report) -> f32 {
        match r {
            Report::Measured { clarity, .. } => clarity,
            other => panic!("not measured: {other:?}"),
        }
    }

    /// With the same spectrum in both, every band has the broadband SNR, so
    /// SII = clamp((SNR + 15) / 30). Each frame holds one syllable, on and
    /// quieter, 1.8 dB under its peak level.
    #[test]
    fn sii_follows_the_gap() {
        for (gap, expect) in [(-5.0, 0.27), (5.0, 0.61), (30.0, 1.0)] {
            let c = clarity(run(12, gap, None));
            assert!((c - expect).abs() < 0.05, "gap {gap} dB: clarity {c:.2}, expected about {expect}");
        }
    }

    #[test]
    fn silence_on_the_rest_is_clear() {
        let r = run(12, 200.0, None);
        assert!(matches!(r, Report::Measured { leak: Leak::NothingPlaying, .. }), "{r:?}");
        assert!(clarity(r) > 0.99);
    }

    #[test]
    fn a_leak_40_db_under_is_heard() {
        let leaky = run(30, 15.0, Some(40.0));
        assert!(matches!(leaky, Report::Measured { leak: Leak::Heard, .. }), "{leaky:?}");
        let clean = run(30, 15.0, None);
        assert!(matches!(clean, Report::Measured { leak: Leak::NotHeard, .. }), "{clean:?}");
    }

    #[test]
    fn too_little_speech() {
        let mut a = Analyzer::new(2);
        a.push(&vec![0.0; 2 * FRAME * 3]);
        assert_eq!(a.report(), Some(Report::TooLittleSpeech));
        assert_eq!(Analyzer::new(2).report(), None);
    }

    #[test]
    fn the_rest_is_every_other_source() {
        // Two other sources, each 3 dB under the one in `sii_follows_the_gap`,
        // sum to it.
        let mut a = Analyzer::new(3);
        let (mut nv, mut n1, mut n2) = (Noise(1), Noise(2), Noise(3));
        let g = 0.1 * 10f32.powf(-5.0 / 20.0) / 2f32.sqrt();
        let buf: Vec<f32> = (0..12 * RATE as usize).flat_map(|i| [nv.next() * 0.1 * envelope(i), n1.next() * g, n2.next() * g]).collect();
        a.push(&buf);
        let c = clarity(a.report().unwrap());
        assert!((c - 0.61).abs() < 0.05, "clarity {c:.2}");
    }
}
