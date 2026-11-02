# Phase 4 spike: ducking, speech intelligibility and leakage

Status: spike, 2026-11-02. Findings for the smart balance phase in
`docs/audio-plan.md`; no clipt code. It answers two of the spike's three
questions: what ducking does to the Speech Intelligibility Index (SII), and
whether clipt can tell that the microphone hears the speakers. The third,
YAMNet's accuracy and cost, is issue #42. Voice detection and the YAMNet
licence are in `docs/audio-phase4-research.md`.

## Summary

- **The rules of thumb hold.** With the music bed plain, the
  10th-percentile SII reaches 0.75 with the music about 15 dB under the
  voice (instrumental: 0.74 at 15 dB) and a little more with vocals (0.70
  at 15 dB, 0.81 at 20 dB).
- **Ducking does most of the work.** Sidechain compression dipping the bed
  about 11 dB while the voice speaks reaches 0.75 with the bed set only
  10 dB under (0.84 instrumental, 0.77 with vocals).
- **The dynamic EQ adds little:** +0.03 to +0.09, most with loud beds, and
  its ffmpeg graph hung intermittently (below).
- **The at-stop re-mix is only needed for loud beds:** within about 5 dB of
  the voice, even ducking with the dynamic EQ stays at 0.56–0.78.
- **Voice clean-up helps at equal level** (+0.05 to +0.07), but must keep
  the voice's loudness: the chain as tried came out 5.5 dB quieter, which
  lost more than it gained.
- **Leakage is detectable in the pauses.** Correlating the microphone with
  the system sound during the microphone's quietest seconds catches music
  leaking in even 40 dB under the voice (peak correlation 0.45–0.79), while
  controls stay at or below 0.19. Over whole windows, speech swamps it.
- **A graph that hangs would hang clipt.** One ffmpeg filter graph
  (`acrossover` with `sidechaincompress` and `amix`) hung at end of stream
  in 11 of 30 runs at one point and in none of 150 later. clipt waits for
  ffmpeg to finish with no time limit, so any graph it runs live needs a
  watchdog on finishing.

## Material

48 kHz mono, 60 s each:

| Stem | Content | Source | Licence |
|---|---|---|---|
| voice | 20 s from each of three LibriVox readers, joined: natural reading with its own pauses (17 % of 400 ms frames are not speech) | [LibriVox Short Poetry Collection 190](https://archive.org/details/spc190_1904_librivox): `spc190_ahareyoudiggingonmygrave_wm` from 25 s, `spc190_arithmeticonthefrontier_js` from 25 s, `spc190_barter_agh` from 20 s | public domain mark 1.0 |
| music with vocals | 60 s from 0:30 of "Here Lies" | Josh Woodward, [*The Shade From Our Trees*](https://archive.org/details/JoshWoodward-TheShadeFromOurTrees) | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) |
| instrumental | the same 60 s of the instrumental version | Josh Woodward, [*The Shade From Our Trees* (instrumental)](https://archive.org/details/JoshWoodward-TheShadeFromOurTrees-NoVox) | CC BY 4.0 |

The same recordings as the research, a longer stretch of each. The voice is
set to −23 dBFS mean; the bed is set 0, 5, 10, 15 or 20 dB under it by
mean level.

## Ducking and SII

### What was measured

ffmpeg 9.0.2 made, for each bed and level, three versions of the bed,
keeping voice and bed as separate files so each can be measured:

- **plain**: the bed at that level;
- **ducked**: `sidechaincompress=threshold=0.02:ratio=8:attack=20:release=300`
  keyed by the voice (attack and release inside the rules' 10–50 ms and
  200–500 ms), which dips the bed about 11 dB while the voice speaks;
- **ducked with dynamic EQ**: the ducked bed split at 1 and 4 kHz
  (`acrossover`), the middle band through a second, harder
  `sidechaincompress` (ratio 20, attack 10 ms, release 250 ms) keyed by the
  voice, and the bands summed (`amix`, `normalize=0`).

And two voices: as recorded, and through the plan's clean-up chain
(`highpass=f=80`, `acompressor` 3:1 from −30 dB with 4 dB make-up,
`deesser`, a 3 dB presence boost at 3 kHz).

SII per 400 ms frame (numpy, Hann window, FFT power summed per octave band
at 250 Hz–8 kHz, band edges at fc/√2 and fc·√2):
SNRᵢ = voiceᵢ − bedᵢ in dB, Aᵢ = clamp((SNRᵢ + 15)/30, 0, 1),
SII = Σ Iᵢ·Aᵢ with weights 0.062, 0.167, 0.237, 0.265, 0.214, 0.055. Only
frames where the voice speaks count: within 15 dB of the voice's 90th
percentile frame level, measured on the clean voice stem (83 % of frames).
The broadband gap in brackets is the median voice-minus-bed level over
those frames; it matches the set level, which checks the measurement.

### Results

10th-percentile SII (median gap in dB). Target 0.75; 0.90 for a safe
margin.

Voice as recorded, instrumental bed:

| Bed under the voice by | 0 dB | 5 dB | 10 dB | 15 dB | 20 dB |
|---|---|---|---|---|---|
| plain | 0.30 (0.3) | 0.45 (5.3) | 0.61 (10.3) | 0.74 (15.3) | 0.86 (20.3) |
| ducked | 0.60 (11.5) | 0.73 (16.6) | **0.84** (21.6) | **0.92** (26.5) | 0.98 (31.5) |
| ducked + dynamic EQ | 0.69 (11.5) | **0.78** (16.8) | **0.87** (21.8) | **0.94** (26.5) | 0.98 (31.5) |

Voice as recorded, bed with vocals:

| Bed under the voice by | 0 dB | 5 dB | 10 dB | 15 dB | 20 dB |
|---|---|---|---|---|---|
| plain | 0.29 (0.2) | 0.40 (5.2) | 0.54 (10.2) | 0.70 (15.2) | **0.81** (20.2) |
| ducked | 0.58 (11.3) | 0.68 (16.3) | **0.77** (21.3) | **0.88** (26.4) | 0.95 (31.3) |
| ducked + dynamic EQ | 0.63 (11.5) | 0.74 (16.5) | **0.81** (21.5) | **0.90** (26.6) | 0.97 (31.5) |

Cleaned-up voice, instrumental bed (the chain made the voice 5.5 dB
quieter, so read across by gap, not by column):

| Bed under the voice by | 0 dB | 5 dB | 10 dB | 15 dB | 20 dB |
|---|---|---|---|---|---|
| plain | 0.23 (−5.3) | 0.38 (−0.3) | 0.51 (4.7) | 0.67 (9.7) | 0.81 (14.7) |
| ducked | 0.51 (6.3) | 0.64 (11.4) | 0.75 (16.4) | 0.86 (21.3) | 0.94 (26.3) |
| ducked + dynamic EQ | 0.54 (6.5) | 0.68 (11.6) | 0.79 (16.6) | 0.87 (21.5) | 0.95 (26.5) |

Cleaned-up voice, bed with vocals:

| Bed under the voice by | 0 dB | 5 dB | 10 dB | 15 dB | 20 dB |
|---|---|---|---|---|---|
| plain | 0.23 (−5.5) | 0.37 (−0.5) | 0.49 (4.5) | 0.63 (9.5) | 0.75 (14.5) |
| ducked | 0.43 (6.4) | 0.56 (11.4) | 0.68 (16.4) | 0.83 (21.4) | 0.91 (26.4) |
| ducked + dynamic EQ | 0.47 (6.4) | 0.61 (11.4) | 0.70 (16.4) | 0.84 (21.4) | 0.92 (26.4) |

At equal gap, the cleaned-up voice scores higher: plain, about 15 dB gap,
0.81 against 0.74 (instrumental) and 0.75 against 0.70 (vocals). The
compression lifting quiet consonants is worth about 0.05–0.07; the level
it lost cost more.

### What it means for phase 4

- **`duck` alone is worth shipping.** Ducking by about 11 dB takes a bed
  set 10 dB under the voice to the 0.75 target, vocals or not; that covers
  most podcast-style use, where the bed is already "under".
- **The dynamic EQ is optional.** It adds 0.03–0.09, least where ducking
  already gets there, and its graph is the one that hung. If it is built,
  use plain high- and low-pass filters for the split (no hangs in 90
  runs) or one filter thread, behind the watchdog below.
- **The at-stop re-mix matters for loud beds** (within ~5 dB of the
  voice). SII rises steadily with the gap, so the fix can search for the
  bed gain that brings the 10th percentile to 0.75, using the separate
  tracks.
- **Voice clean-up needs level matching:** measure the voice's loudness
  before and after the chain and make up the difference (or follow it with
  `loudnorm` on the voice alone), so the clean-up never lowers the gap.
- **Judging vocals by SII alone understates them:** the vocal bed needs
  only 1–2 dB more than the instrumental by SII, where the rule of thumb
  says 5 dB (20 against 15). That difference is the informational masking
  SII doesn't model, which is the case for the background classifier.

## Leakage

### What was measured

A simulated microphone: the voice, plus the instrumental (or vocal) bed
through a room (12 ms delay, a 0.35 s exponentially decaying reverb tail
from seeded noise, highs rolled off above 5 kHz), plus noise 55 dB under the
voice; the leak set 10, 20, 30 or 40 dB under the voice, or absent. Both
streams low-passed at 3.8 kHz and decimated to 8 kHz. For each window, the
peak over lags 0–250 ms of the normalised cross-correlation between the
microphone and the system stream (the bed as clipt would record it). Two
controls: no leak at all, and a leak against the wrong reference (the same
bed 17 s later, standing in for unrelated sound).

### Results

Over 3 s windows, the voice swamps the leak:

| Leak | Median peak (instrumental / vocals) |
|---|---|
| none (control) | 0.035 / 0.043 |
| 40 dB under | 0.036 / 0.043 |
| 30 dB under | 0.046 / 0.046 |
| 20 dB under | 0.074 / 0.077 |
| 10 dB under | 0.160 / 0.169 |

Only a leak 10 dB under stands out; at 20 dB it is within the range the
wrong-reference control reaches (up to 0.088). GCC-PHAT, often better in
reverberant rooms, did worse (0.036–0.045 throughout).

In the pauses (the quietest quarter of 1 s windows, where the microphone
holds mostly leak and noise), the leak shows. The maximum over those
windows:

| Leak | Highest peak in a pause (instrumental / vocals) |
|---|---|
| none (control) | 0.124 / 0.120 |
| wrong reference, leak 40 dB under (control) | 0.121 / 0.121 |
| wrong reference, leak 20 dB under (control) | 0.133 / 0.194 |
| 40 dB under | **0.453** / **0.776** |
| 30 dB under | **0.481** / **0.792** |
| 20 dB under | **0.484** / **0.794** |
| 10 dB under | **0.629** / **0.794** |

### What it means for phase 4

- **The check works, in the pauses:** "some quiet second correlates above
  about 0.3 with a system source" separates every leak level from both
  controls, down to a leak 40 dB under the voice, which no listener would
  notice. Cost: one 16 k-point FFT pair per second of quiet audio at 8 kHz.
- **It needs pauses** of about a second; speech has them, but a voice
  talking non-stop over music for the whole recording would go unchecked.
  Report "not enough pauses to check" rather than "no leak" then.
- **It is a simulation:** one room model, one song, one set of voices.
  Real speakers, rooms and microphones (and echo cancellation in some
  microphone chains, which would hide the leak but also remove it) should
  be tried before the threshold is fixed; log the peak with `CLIPT_DEBUG`
  when it's built so real recordings can set it.

## ffmpeg hangs

While making the stems, the dynamic-EQ graph
`acrossover=split=1000 4000` → `sidechaincompress` on the middle band →
`amix=inputs=3:normalize=0` sometimes stopped near the end of its input
and never exited; `timeout` sending SIGTERM didn't end it either (SIGKILL
did). Counts, ffmpeg 9.0.2, the same 60 s input:

| When | Graph | Hung |
|---|---|---|
| first test | as above, threaded | 11 of 30 |
| building all stems | as above, one filter thread | 2 of 10 calls |
| later, quiet machine | as above, threaded / one thread | 0 of 30 / 0 of 30 |
| later, all 16 cores busy | threaded / one thread / high- and low-pass split / plain ducking | 0 of 30 each |

The plain ducking graph never hung in any run. The trigger wasn't found;
it looks like a race at end of stream in the filter graph's scheduling.
Recordings already run ffmpeg graphs (`amix`, `asplit`), and none of the
e2e runs has hung, but clipt waits for ffmpeg to finish without a limit
(`Encoder::finish`). A watchdog there (kill ffmpeg if it hasn't finished a
while after its input closed, and say where the partial file is) protects
against this for every graph, now and in phase 4.

## How it was run

Two scripts outside the repository, numpy only (no scipy):

- the stems: ffmpeg commands as listed above, each under `timeout -s KILL`
  with retries so a hang is logged rather than stalls;
- SII as above, and the leakage check: decimation by FFT low-pass, the room
  by FFT convolution with the seeded tail, normalised cross-correlation by
  FFT over the lag range.

The scripts, audio and stems weren't committed; the parameters above are
enough to redo them.
