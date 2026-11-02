# Phase 4 YAMNet measurement: accuracy and cost through tract

Status: spike findings, 2026-11-02. This is a findings document; it changes
neither `docs/audio-plan.md` nor any code.

This is the remaining YAMNet question in the phase 4 spike. The licence and
the model file are settled in
[audio-phase4-research.md](audio-phase4-research.md); this measures whether
the model works well enough to earn its 16 MB over the plan's safe 20 dB
default, and what it costs to run through tract.

## Summary

- **It runs through tract, as the plan requires.** `tract-onnx` 0.23.9 loads
  the hash-checked ONNX export in 45.8 ms and runs a 0.96 s window in
  4.15 ms on one core. With the 0.48 s hop that is 8.5 ms of inference
  per second of audio, plus 0.55 ms of resampling. The whole path is about
  0.9 % of one core.
- **Clean speech, instrumental music and noise are easy; music with vocals
  is not.** After smoothing, on windows away from transitions, YAMNet finds
  97 % of speech, 93 % of instrumental music and 100 % of noise, but only
  68 % of the windows whose song has a lead vocal. Overall 90.9 % of settled
  windows are right; including the windows around every change it is 83.4 %.
- **The singing score is a weak vocal detector.** Its AUC against
  instrumental music is 0.86. At the threshold used here it flags 66 % of the
  vocal windows and 6 % of the instrumental windows. The cost of missing a
  third of the vocals is the reason the model does not replace the 20 dB
  default.
- **As a second voice check it does what nnnoiseless could not.** The speech
  score separates clean speech from music with vocals, instrumental music,
  white noise and pink noise with AUC 1.00, where nnnoiseless was 0.20–0.66.
  With music under the speech the separation is less complete: AUC 0.83 at
  −10 dB and 0.87 at −20 dB.
- **Verdict.** YAMNet does not earn its 16 MB for choosing between the 15 and
  20 dB background rules: the vocal/instrumental split is too weak. It is
  useful as the plan's "stronger second discriminator" on the voice source,
  if the background model is built for another reason. The 20 dB default
  stays.
- **Cost.** The stripped binary grows by 18.9 MiB on top of the 16 MB model,
  and a clean release build takes 248 s with tract against 2.6 s without.

## What was run

A throwaway Rust program under `/var/tmp/yamnet-work`, outside clipt's crate.
It is not committed, and neither is the model or the audio.

- Rust 1.95.0, `tract-onnx` 0.23.9 (default features) and `hound` 3.5.1 for
  WAV reading. Release build with clipt's profile from `Cargo.toml`:
  `lto = true`, `codegen-units = 1`, `strip = true`, `panic = "abort"`,
  `opt-level = 3`.
- The model is the ONNX export from
  [`audiomagic/yamnet-onnx`](https://huggingface.co/audiomagic/yamnet-onnx),
  `yamnet.onnx`, 16,093,355 bytes, SHA-256
  `d3835ffbbd4a1bb3e777f0ca217b5007907f5171dd5d17c4236b95b2af8f908e`.
  The SHA-256 was checked against the value in the research.
- The program reads 48 kHz mono 16-bit WAV, resamples to 16 kHz, and runs the
  model on 0.96 s windows (15,360 samples) every 0.48 s (7,680 samples). The
  48 → 16 kHz resampler is a 63-tap Blackman-windowed sinc low-pass at 7.2 kHz
  followed by taking every third sample; it is written in the program, not
  taken from a crate.
- A 36.5 s file gives 75 windows. The model takes a one-dimensional
  `[15360]` `f32` waveform and returns a `[1, 521]` score tensor (sigmoid
  outputs), a `[1, 1024]` embedding and a `[96, 64]` spectrogram. Only the
  scores are used. The input shape has to be pinned before tract will type
  the graph; with the shape left dynamic, tract fails on the model's internal
  `Pad`.
- All runs were single-threaded (`RAYON_NUM_THREADS=1`) on the machine in
  *Cost* below.

The model input is one frame; the internal framing in the ONNX graph is not
used. Windows were taken by the program, so overlapping windows are
independent model calls.

## Test material

All material is 48 kHz mono. The speech, noise and the first song's files
are the ones built for the nnnoiseless research, so the two measurements use
the same signal paths.

| File | Content | Source | Licence |
|---|---|---|---|
| `speech.wav` | 36.5 s: 2 s lead, eight speech segments of 2.0–3.3 s with 1.5 s gaps, 2 s tail | [LibriVox Short Poetry Collection 190](https://archive.org/details/spc190_1904_librivox), tracks `spc190_ahareyoudiggingonmygrave_wm`, `spc190_arithmeticonthefrontier_js`, `spc190_barter_agh` | public domain mark 1.0 |
| `music_with_vocals.wav`, `music_instrumental.wav` | 36.5 s from 0:30 of "Here Lies", vocal and NoVox versions | [Josh Woodward, *The Shade From Our Trees*](https://archive.org/details/JoshWoodward-TheShadeFromOurTrees) and [the NoVox release](https://archive.org/details/JoshWoodward-TheShadeFromOurTrees-NoVox) | CC BY 4.0 |
| `HalfLife_vox.wav`, `HalfLife_instrumental.wav` | the same 36.5 s of "Half Life", vocal and NoVox | as above | CC BY 4.0 |
| `Bully_vox.wav`, `Bully_instrumental.wav` | the same 36.5 s of "Bully", vocal and NoVox | as above | CC BY 4.0 |
| `speech_music_10db.wav`, `speech_music_20db.wav` | `speech.wav` plus the "Here Lies" instrumental at 10 dB and 20 dB below the active speech RMS | made from the two above | as above |
| `speech_music_vocals_10db.wav` | the same with the sung version at −10 dB; a harder extra case | made from the two above | as above |
| `noise_white.wav` | 36.5 s synthetic white noise, −20 dBFS RMS | NumPy, seed 20261101 | none |
| `noise_pink.wav` | 36.5 s synthetic pink noise, −20 dBFS RMS | ffmpeg `anoisesrc=color=pink` | none |

The speech edit takes these runs: from `spc190_ahareyoudiggingonmygrave_wm`
4.7–7.1, 9.0–12.3 and 24.9–27.2 s; from
`spc190_arithmeticonthefrontier_js` 10.5–13.8, 15.5–18.0 and 29.0–31.0 s;
and from `spc190_barter_agh` 3.5–6.1 and 14.6–16.7 s. Each run is faded over
10 ms; 2 s of lead, 1.5 s gaps and 2 s of tail make the 36.5 s file.

The music excerpts are peak-normalised to 0.9; the mixes are scaled so their
peaks stay below 0.99. The three songs are one artist and one album, but the
vocal and NoVox versions of each song control for the backing music.

### Vocal reference labels

Each song's vocal and NoVox versions are the same backing track with and
without the vocals. After aligning the two versions at zero lag and fitting a
single gain, the difference is the vocal plus any mastering or encoding
difference. A window is labelled **music with vocals** when the difference's
RMS is no more than 8 dB below the vocal mix's RMS (that is, the difference
is substantial). Vocal windows with a weaker difference are treated as
ambiguous and left out of the confusion table, and all NoVox windows are
labelled **instrumental music**. This gives 146 vocal windows out of the 225
vocal-version windows, and 225 instrumental windows. The rest are excluded.

This is a physical label, not a hand label: it says the vocal version
contains something the instrumental version does not, and it is strong
enough to be the lead vocal. It cannot tell a loud vocal from a different
instrumental mix, which is a limitation below.

Speech windows are labelled by the edit intervals: a window is **speech**
when at least half of it falls inside a speech interval, and silence
otherwise. In the mixes, the windows outside the speech intervals are
**instrumental music** (the music continues under the gaps).

## How windows were classified

The class indices used are hard-coded, as the research recommends; the CSV is
not needed at run time.

| Group | AudioSet indices | Classes |
|---|---|---|
| Speech | 0, 1, 2, 3, 5 | Speech, Child speech, Conversation, Narration, Speech synthesizer |
| Singing | 24–32, 249, 250, 261 | Singing, Choir, Yodeling, Chant, Mantra, Child singing, Synthetic singing, Rapping, Humming, Vocal music, A capella, Song |
| Music | 132, 211–276 | Music, and the genre and style classes under it |
| Noise | 507, 508, 509, 514, 515 | Noise, Environmental noise, Static, White noise, Pink noise |

The singing and music groups overlap at 249, 250 and 261; those classes are
evidence for both. Each group score is the sum of its class scores. The sums
are not probabilities and they are not comparable in size, so the rule is a
set of comparisons rather than a single argmax:

1. A window with RMS below −50 dBFS is **silence** and is not classified.
2. **Noise** when the noise sum is over 0.5 and over the music sum.
3. **Speech** when the speech sum is over 0.1 and over half the music sum.
4. Otherwise the window is music: **music with vocals** when the singing sum
   is over 0.006, else **instrumental music**.

The plan's "speech and singing against music" is split here because the four
classes need the two voice types separately: the speech sum is compared with
the music sum, and the singing sum then decides which kind of music it is.

The scores are smoothed causally with a three-window mean (1.44 s) before the
comparisons. The three-window mean is why a decision takes a window or two to
settle after a change; the figures below show both the cost and the settled
result.

The 0.5 speech-to-music ratio and the 0.006 singing threshold were chosen
after looking at these scores. They are not held-out settings; a deployment
would need a separate validation set. The singing threshold is the more
sensitive one: the sweep below shows the trade-off.

## Accuracy

The evaluation has 789 windows: 172 speech, 146 music with vocals, 321
instrumental music and 150 noise. Windows labelled silence are left out of
the confusion table; the RMS gate removes the digital-silence ones. The table
counts all of them, including the windows where the causal smoother is still
catching up.

| Reference | Speech | Music with vocals | Instrumental | Noise | Recall |
|---|---:|---:|---:|---:|---:|
| Speech | 163 | 3 | 6 | 0 | 0.948 |
| Music with vocals | 0 | 97 | 49 | 0 | 0.664 |
| Instrumental music | 52 | 21 | 248 | 0 | 0.773 |
| Noise | 0 | 0 | 0 | 150 | 1.000 |

Overall accuracy is 0.834. All 52 instrumental windows called speech are within
two or three windows after a speech segment ends, where the causal
mean still carries the speech score. Excluding the two windows after each
reference change leaves 582 settled windows:

| Reference | Speech | Music with vocals | Instrumental | Noise | Recall |
|---|---:|---:|---:|---:|---:|
| Speech | 74 | 0 | 2 | 0 | 0.974 |
| Music with vocals | 0 | 73 | 34 | 0 | 0.682 |
| Instrumental music | 0 | 17 | 232 | 0 | 0.932 |
| Noise | 0 | 0 | 0 | 150 | 1.000 |

The settled accuracy is 0.909. The weak row is the one that matters for the
background rule: about one window in three with a lead vocal is called
instrumental, and about one instrumental window in fifteen is called vocal.

Per file, before settling (the figures differ from the aggregate because the
counts per class differ):

| File | Windows right |
|---|---:|
| `speech.wav` | 43/43 = 1.00 |
| `music_with_vocals.wav` | 47/60 = 0.78 |
| `music_instrumental.wav` | 74/75 = 0.99 |
| `HalfLife_vox.wav` | 26/44 = 0.59 |
| `HalfLife_instrumental.wav` | 63/75 = 0.84 |
| `Bully_vox.wav` | 24/42 = 0.57 |
| `Bully_instrumental.wav` | 75/75 = 1.00 |
| `speech_music_10db.wav` | 59/75 = 0.79 |
| `speech_music_20db.wav` | 52/75 = 0.69 |
| `speech_music_vocals_10db.wav` | 45/75 = 0.60 |
| `noise_white.wav` | 75/75 = 1.00 |
| `noise_pink.wav` | 75/75 = 1.00 |

The −20 dB mix scores lower than the −10 dB mix because its music-only gaps
are quiet: they are above the −50 dBFS gate, but the speech score after a
speech segment is still above the comparison threshold for a few windows.
The settled figures remove that artefact; the gate is doing its job on
digital silence, not on quiet music.

### How long a decision takes to settle

A reference onset is counted when the new label lasts at least five windows.
Settling is the number of windows after the onset at which the smoothed
class first matches. One window is 0.48 s.

| Transition | Onsets | Median | 90th percentile | Worst |
|---|---:|---:|---:|---:|
| Silence or music → speech | 24 | 0 | 1 | 3 |
| Instrumental → music with vocals | 10 | 0 | 4 | 4 |

Speech is usually right in the first window that contains it. Vocal entries
are slower, and the four-window cases are 1.9 s; they are the quiet or
weakly sung entries that also produce the missed vocal windows.

## As a second voice check

The plan's question is whether YAMNet's speech score can say "someone is
speaking" where nnnoiseless also heard a voice in music or noise. The
AUC is computed the same way as the research's: one positive set of speech
windows and one negative set of non-speech windows, 0.5 is chance, 1.0 is
perfect. YAMNet's speech score is the smoothed sum of its speech classes.

| Comparison | nnnoiseless (research) | YAMNet speech score |
|---|---:|---:|
| Speech against pauses in the speech track | 0.984 | 0.926 |
| Speech against music with vocals | 0.544 | 1.000 |
| Speech against instrumental music | 0.655 | 1.000 |
| Speech against white noise | 0.555 | 1.000 |
| Speech against pink noise | 0.195 | 1.000 |
| Speech in the −10 dB instrumental mix against its music-only gaps | — | 0.828 |
| Speech in the −20 dB instrumental mix against its music-only gaps | — | 0.872 |
| Speech in the −10 dB sung mix against its music-only gaps | — | 0.770 |

This is the strongest result in the spike. On a clean voice source YAMNet
separates speech from music and noise completely, where nnnoiseless was near
chance. The separation weakens when the music is under the speech, but 0.83
at −10 dB is still far better than nnnoiseless's music results: the speech
score still works with music under the speech, where nnnoiseless's did not.

The lower speech-against-pauses figure (0.926 against 0.984) is a window
artefact: a 0.96 s window can straddle the edit boundary, and the reference
label then disagrees with what the model hears. The research's frame-level
labels did not have that problem.

### The singing score

The weak half of the result is the distinction between music with vocals and
instrumental music. The singing score has AUC 0.859 on the 146 strong vocal
windows against the 225 instrumental windows. The threshold sweep:

| Singing sum threshold | Vocal recall | Instrumental false positive |
|---:|---:|---:|
| 0.002 | 0.81 | 0.29 |
| 0.004 | 0.71 | 0.13 |
| 0.006 (used here) | 0.66 | 0.06 |
| 0.008 | 0.58 | 0.02 |
| 0.010 | 0.51 | 0.01 |
| 0.015 | 0.35 | 0.00 |

The threshold used here leans toward recall because calling a vocal
instrumental is the worse error for intelligibility. Even so it misses one
vocal window in three. There is no threshold that both keeps most vocals and
rejects most instrumentals; the distributions overlap.

## Cost

On this machine: AMD Ryzen 7 3800X, 8 cores / 16 threads, Linux 7.2.9
(CachyOS), Rust 1.95.0, one thread for the model. Timings are from the
release build with clipt's release profile.

| Item | Value |
|---|---:|
| Model load, parse + type + optimise + runnable | 45.8 ms |
| Inference per 0.96 s window | 4.15 ms (median), 4.77 ms (95th percentile) |
| Windows per second of audio at the 0.48 s hop | 2.05 |
| Inference per second of audio | 8.5 ms |
| Resampling, 48 → 16 kHz, per second of audio | 0.55 ms |
| Whole path per second of audio | 9.1 ms |
| Share of one core while audio plays | 0.9 % |
| Binary size without tract | 302,320 bytes |
| Binary size with tract, default features | 20,077,808 bytes |
| Binary size with tract, default features off | 19,343,944 bytes |
| Added by tract, default features | 19,775,488 bytes (18.9 MiB) |
| Added by tract, default features off | 19,041,624 bytes (18.2 MiB) |
| Clean release build without tract | 2.56 s |
| Clean release build with tract | 247.70 s |

The load figure is the median of five runs (44.4–47.0 ms; the first run
after the build was 59.1 ms) and is the time from opening the 16 MB file to
a runnable plan; it is paid once before recording starts. The inference
figure is the wall-clock time of `runnable.run` over 900 windows; the
resampling figure is ten passes over each 36.5 s file. The binary figures
are stripped release builds of a tiny program that loads the model and runs
one all-zero window,
and of the same program without tract or the model call. The 16 MB model
file is not embedded in either binary, so the feature costs its download
plus this growth on disk. The default-features build includes tract's
`transformers` support, which YAMNet does not use; turning it off saves about
0.7 MiB. The compile time is for the default features, and it is a clean
build, not an incremental one.

## What it means for phase 4

- **Keep the 20 dB default.** YAMNet's vocal/instrumental distinction is not
  reliable enough to choose 15 dB for instrumental music: it misses about a
  third of the vocals, and a missed vocal gets 5 dB less ducking than the
  plan's "singing or talking" rule calls for. A model that is wrong one time
  in three is worse than the safe default.
- **Use it, if at all, as the second voice check.** On the voice source it
  tells speech from music and noise far better than nnnoiseless. That is the
  plan's "stronger second discriminator" use, and it is independent of the
  background question. It would need the model download, the feature flag
  and the tract dependency, so it only makes sense if that path is built for
  another reason.
- **Do not build it just for the background split.** The 16 MB download,
  the extra binary and the load time buy a 66 % vocal detector; the
  correlation leakage check and the 20 dB default cover the decision without
  a model.
- **If the background model is built anyway,** the speech score is worth
  using as the leakage discriminator and the singing score is a weak extra
  hint, not a decision. The pipeline is cheap enough: under 1 % of a core
  for the inference and resampling.

## Reproducing

The program is not committed. To redo it:

1. Put `yamnet.onnx` (SHA-256
   `d3835ffbbd4a1bb3e777f0ca217b5007907f5171dd5d17c4236b95b2af8f908e`)
   somewhere, and add `tract-onnx = "0.23.9"` and `hound = "3.5.1"` to a
   throwaway binary using clipt's release profile.
2. Read 48 kHz mono 16-bit WAV, resample as described in *What was run*, and
   cut windows of 15,360 samples every 7,680. Pin the model input fact to
   `[15360]` before typing; feed one window as a 1-D `f32` array.
3. Keep the score output `[1, 521]`, sum the indices in the table, apply the
   gate, the three-window causal mean and the four rules above.
4. Test audio: the three Josh Woodward song pairs from the archive.org items
   in *Test material*, the LibriVox speech edit (the eight intervals are
   2.0–4.4, 5.9–9.2, 10.7–13.0, 14.5–17.8, 19.3–21.8, 23.3–25.3,
   26.8–29.4 and 30.9–33.0 s), the mixes and the two noise files. The runs
   and the noise generation are described in *Test material*.
5. Set `RAYON_NUM_THREADS=1` for single-threaded timings.

The model's class map was downloaded only to choose and document the
indices; the program does not use the CSV.

## Credits

- **Music:** Josh Woodward, *The Shade From Our Trees* (2019), tracks
  "Here Lies", "Half Life" and "Bully", from
  [the album](https://archive.org/details/JoshWoodward-TheShadeFromOurTrees)
  and [the NoVox release](https://archive.org/details/JoshWoodward-TheShadeFromOurTrees-NoVox).
  Licensed under [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/).
  The excerpts used here are 36.5 s from 0:30 of each track, peak-normalised;
  the NoVox versions were used as the instrumental reference. The mixes were
  made by adding the instrumental under the speech. No other changes were
  made.
- **Speech:** LibriVox, *Short Poetry Collection 190*, public domain mark 1.0;
  the readers are credited on the collection page.
- **Model:** `audiomagic/yamnet-onnx`, Apache-2.0; Google's YAMNet weights are
  Apache-2.0. See
  [audio-phase4-research.md](audio-phase4-research.md) for the licence work.
- **Class names:** the AudioSet ontology, CC BY-SA 4.0. Only the indices are
  used, not the names or the CSV.

## Limitations

- **The thresholds are not held out.** The class sets, the 0.5
  speech-to-music ratio and the 0.006 singing threshold were chosen after
  looking at these scores. The accuracy figures describe this test set, not
  a deployment; a separate validation set is needed before the rule is used.
- **The vocal labels come from the vocal-minus-instrumental difference, not
  from listening.** A window with a strong difference is labelled vocal; a
  quiet vocal or a different instrumental mix can be mislabelled. The 146
  high-confidence windows are a subset of the 225 vocal-version windows; the
  rest were excluded rather than guessed.
- **Three songs, one artist, one album, 36.5 s each.** The three songs have
  different mixes, but they are all Josh Woodward recordings with a similar
  production style. Other music, especially dense mixes or heavily processed
  vocals, may score differently.
- **The speech track is an edit of eight runs, not a natural recording.**
  Its pauses are digital silence, which makes the gate easy and the clean
  speech case optimistic. The mix cases add music to the same edit, so the
  relative comparison is fair, but real room noise is absent.
- **Windows are 0.96 s and the labels are window-level.** A window that
  straddles a speech/pause or vocal/instrumental boundary is labelled by its
  majority; the model sees the mixed content, so some "errors" are boundary
  windows. The settled table removes the smoothing part of that, not the
  label part.
- **The model was not fine-tuned, quantised or run with a different
  front-end.** The ONNX export is the stock YAMNet with its own mel
  spectrogram; the numbers are what that export does.
- **The test levels differ and the model is not level-normalised.** Music,
  speech and noise were peak-normalised or set to different RMS levels; a
  much quieter or louder voice source could score differently; both the
  model's scores and the gate depend on level.
- **One machine.** The CPU figures are from one Ryzen 7 3800X; tract's
  single-thread performance on other machines will differ, though the
  decision does not turn on a factor of two.
- **Only the score output was used.** The embedding and spectrogram outputs
  were ignored; a small classifier on the embedding might separate vocals
  better, but that would be a different, larger piece of work.
