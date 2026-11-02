# Phase 4 audio research: YAMNet weights and nnnoiseless voice detection

Status: research, 2026-11-02. This is a findings document; it changes
neither `docs/audio-plan.md` nor any code.

This supports the smart balance phase in `docs/audio-plan.md`. It answers the
two questions that phase left to a spike: the licence of the YAMNet weights
the plan considers, and whether nnnoiseless's voice probability is a usable
voice activity detector.

## Summary

- **YAMNet weights.** The ONNX export at
  `huggingface.co/audiomagic/yamnet-onnx` is Apache-2.0, as is the original
  `google/yamnet` model. clipt (MIT OR Apache-2.0) can download and use it,
  and the Flatpak can ship it, with the Apache-2.0 licence and attribution.
  The export is a straight tf2onnx conversion: the SHA-256 of the upstream
  Kaggle archive matches the one the exporter records.
- **The class map is separate.** Its labels come from the AudioSet ontology,
  which Google publishes under CC BY-SA 4.0 (the dataset is CC BY 4.0). If
  clipt ships the class map, attribute AudioSet and keep the share-alike
  terms; if it only needs a handful of class indices, hard-code those and
  leave the CSV out.
- **nnnoiseless is a good speech-versus-silence gate, not a speech detector.**
  On clean speech with pauses, threshold 0.5 with 100 ms smoothing marks 73%
  of speech frames and 2.6% of pause frames (AUC 0.98). On music with vocals,
  instrumental music, white noise and pink noise it does much worse: at the
  threshold that gives 90% recall on clean speech, 93% of music-with-vocals
  frames, 77% of instrumental frames and 100% of the noise frames also pass.
  No threshold separates them.
- **What that means.** The plan's "only frames with speech count" holds only
  when the voice source is clean (headphones, quiet room). With speakers, or
  a noisy room, the VAD includes music or noise and the SII frame selection is
  wrong. The plan's "speakers spoil it" risk is confirmed, and is worse than a
  constant level error.
- **Cost.** nnnoiseless costs about 0.09 ms of CPU per 20 ms of non-silent
  audio on one core (224x real time), under 0.5% of a core. It is cheap enough
  to run on its own thread.

## YAMNet weights licence

### The export

[`audiomagic/yamnet-onnx`](https://huggingface.co/audiomagic/yamnet-onnx) is
an ONNX export of Google's YAMNet, made with tf2onnx. The model card says no
weights were retrained, fine-tuned, pruned or quantized; the change is the
format conversion plus a copy of `yamnet_class_map.csv`.

The Hugging Face API returns `license: apache-2.0` for the repository, and the
repository has an [Apache-2.0
`LICENSE`](https://huggingface.co/audiomagic/yamnet-onnx/blob/main/LICENSE).
The model card also records the upstream model and licence in its provenance
table.

### The original

The upstream is [`google/yamnet`](https://www.kaggle.com/models/google/yamnet)
on Kaggle, formerly `tfhub.dev/google/yamnet/1`. The exporter records the
upstream licence as Apache 2.0. Google's [TensorFlow models
repository](https://github.com/tensorflow/models/tree/master/research/audioset/yamnet),
which holds the YAMNet code, is under the [Apache-2.0 repository
licence](https://github.com/tensorflow/models/blob/master/LICENSE).

One caveat on the check: the Kaggle model page renders its details with
JavaScript, and its static HTML and downloaded archive contain no licence
file. I took the upstream statement from the exporter's provenance table and
from Google's repository licence, and tied the two together with the source
archive hash below. If the plan wants a belt-and-braces check, open the Kaggle
page in a browser and look at its licence field; the archive itself will not
settle it.

### What clipt may do

clipt can download and use the export. Apache-2.0 is one of clipt's two
licences, and it is compatible with MIT in any case. The obligations are the
usual permissive ones:

- include a copy of the Apache-2.0 licence;
- keep the copyright, attribution and modification notices (the exporter
  documents the conversion as Apache-2.0 §4(b) requires);
- there is no `NOTICE` file upstream to reproduce.

For a non-Flatpak build, downloading to `~/.local/share/clipt/models` and
checking a known hash, as the plan proposes, is fine. Store the licence text
and attribution next to the model, or in the repository's third-party notices.

The Flatpak can ship the weights. Apache-2.0 allows redistribution, and the
Flatpak has no network at run time, so the manifest can fetch the 16 MB file
at build time, verify its SHA-256, install it under `/app/share/clipt/models`
and include the licence and attribution in the app. The file is
16,093,355 bytes, about the "15 MB" the plan expected.

### The class map

`yamnet_class_map.csv` is copied from the SavedModel's assets. Its 521 names
come from the AudioSet ontology. Google's [AudioSet download
page](https://research.google.com/audioset/download.html) says the dataset is
CC BY 4.0 and the ontology is CC BY-SA 4.0. If clipt ships the CSV, include
the AudioSet attribution and the CC BY-SA 4.0 terms. If it only needs the
handful of classes in the plan, hard-code their indices and leave the CSV out:

| Class | Index |
|---|---|
| Speech | 0 |
| Singing | 24 |
| Music | 132 |

The plan's "speech and singing against music" mapping needs a few more
speech/singing classes (for example `Conversation` 2, `Narration` 3, `Choir`
25, `Vocal music` 249) and the music subclasses; the class map is how you find
them, but the values themselves are facts and can live in a small table. That
avoids both the share-alike question and 14 KB of CSV that clipt would not use
at run time.

### Hash check

I downloaded the upstream archive through the Kaggle model endpoint and
computed its SHA-256. It matches the exporter's recorded source archive hash,
so the ONNX file is a conversion of the same SavedModel:

| Item | SHA-256 |
|---|---|
| Kaggle source archive (`yamnet-tensorflow2-yamnet-v1.tar.gz`) | `b80da2a1a56926fb0767205051a200dd7b3beaf3ea1ea126c42a53943996e5e0` |
| ONNX export (`yamnet.onnx`) | `d3835ffbbd4a1bb3e777f0ca217b5007907f5171dd5d17c4236b95b2af8f908e` |

The archive contains `saved_model.pb`, the variables and
`assets/yamnet_class_map.csv`; it has no licence file of its own. The ONNX
graph carries the TensorFlow op names and the tf2onnx marker, as a conversion
should.

### If the export were not trusted

It is clear enough here, but the same route is reproducible: download
`google/yamnet` from Kaggle (the archive hash above), convert its SavedModel
with tf2onnx, and keep the Apache-2.0 licence. A different export would need
its own check; the weights are the same, but the exporter's licence statement
is what you rely on.

### How it was checked

- Read the Hugging Face model API (`license: apache-2.0`), the raw model card
  and the repository `LICENSE`.
- Read the exporter's provenance table (upstream model, upstream licence,
  source archive SHA-256).
- Downloaded the Kaggle model archive and hashed it; compared the hash with
  the exporter's.
- Downloaded and hashed the ONNX file; looked at its strings for the tf2onnx
  marker.
- Read Google's TensorFlow models `LICENSE` and the YAMNet README.
- Read the AudioSet download page for the dataset and ontology licences.
- Read `yamnet_class_map.csv` from the archive for the class indices.

## nnnoiseless as voice activity detection

### What was run

I wrote a throwaway Rust program under `/tmp/nn-vad`, outside clipt's crate.
It is not committed, and neither is the test audio.

- `nnnoiseless = "0.5.2"` (BSD-3-Clause; built-in model, no model file) and
  `hound = "3.5"` for WAV reading. Release build, Rust 1.95.0.
- The program reads a 48 kHz mono 16-bit WAV. It scales each sample to the
  16-bit range `[-32768, 32767]` that the crate documents, then calls
  `DenoiseState::process_frame(out, in)` on 480-sample (10 ms) frames and
  writes the returned voice probability for each frame.
- A fresh `DenoiseState` starts at the beginning of each file. The crate says
  to discard the first output because it has fade-in artifacts; the effect is
  at most one 10 ms frame out of 3650, and for the speech and mixture files
  that frame is in the leading silence.
- For CPU timing the program repeats the whole pass 10–20 times, creating a
  fresh state each time, and times only the `process_frame` call. The copy and
  scale around it are timed separately and add about 0.0002 ms per frame.
- The analysis reads the probabilities in Python: a causal moving average of
  the last N frames, then a threshold. A frame is speech if its centre falls
  inside one of the speech intervals in the edited track. Everything outside
  those intervals is non-speech. The reference is the edit, not a hand label;
  short pauses inside a segment count as misses, which is why the eight
  segments were chosen from continuous speech runs.
- For music-only and noise-only files every frame is non-speech. For the
  mixtures the speech intervals are the same; the gaps are music-only frames.

### Test material

All material is 48 kHz mono. The sources are freely licensed:

| File | Content | Source | Licence |
|---|---|---|---|
| `speech.wav` | 36.5 s: 2 s lead, eight speech segments of 2.2–3.3 s with 1.5 s gaps, 2 s tail; peak −0.9 dBFS, active speech RMS −23.3 dBFS | [LibriVox Short Poetry Collection 190](https://archive.org/details/spc190_1904_librivox), tracks `spc190_ahareyoudiggingonmygrave_wm`, `spc190_arithmeticonthefrontier_js`, `spc190_barter_agh` | public domain mark 1.0 |
| `music_with_vocals.wav` | 36.5 s from 0:30 of "Here Lies" | [Josh Woodward, *The Shade From Our Trees*](https://archive.org/details/JoshWoodward-TheShadeFromOurTrees), track 1 | CC BY 4.0 |
| `music_instrumental.wav` | the same 36.5 s of the instrumental version | [*The Shade From Our Trees* (instrumental)](https://archive.org/details/JoshWoodward-TheShadeFromOurTrees-NoVox), "Here Lies" | CC BY 4.0 |
| `speech_music_10db.wav` | `speech.wav` plus the instrumental at 10 dB below the active speech RMS | made from the two above | as above |
| `speech_music_20db.wav` | the same at 20 dB below | made from the two above | as above |
| `speech_music_vocals_10db.wav` | `speech.wav` plus the sung version at 10 dB below; a harder extra case | made from the two above | as above |
| `noise_white.wav` | 36.5 s synthetic white noise, −20 dBFS RMS | NumPy, seed 20261101 | none |
| `noise_pink.wav` | 36.5 s synthetic pink noise, −20 dBFS RMS | ffmpeg `anoisesrc=color=pink` | none |

The mixes are scaled so their peaks stay below 0.91; nothing clips. The active
speech RMS is −23.3 dBFS, so the instrumental in the two mixes sits at −33.3
and −43.3 dBFS RMS. The music is one song, but using its vocal and
instrumental versions controls for the music itself.

The speech segments are from three readers, so the VAD sees more than one
voice. The gaps and the lead/tail are digital silence; the segments were
chosen from runs with no long internal pause.

### Clean speech

With 100 ms causal smoothing, the clean speech track gives:

| Threshold | Speech frames marked | Pause frames marked | Precision | F1 |
|---|---|---|---|---|
| 0.3 | 83.6% | 3.5% | 96.8% | 0.90 |
| 0.5 | 73.2% | 2.6% | 97.3% | 0.84 |
| 0.7 | 60.4% | 1.4% | 98.2% | 0.75 |
| 0.9 | 46.1% | 0.8% | 98.6% | 0.63 |
| 0.99 | 30.2% | 0.3% | 99.2% | 0.46 |

The high precision is partly because 44% of the track is silence. The VAD is
conservative: raising the threshold buys pause precision and loses speech
recall quickly. A threshold around 0.3–0.5 is the useful range.

Smoothing length at threshold 0.5:

| Smoothing | Speech frames marked | Pause frames marked | F1 |
|---|---|---|---|
| 10 ms | 73.1% | 0.8% | 0.84 |
| 50 ms | 73.0% | 1.5% | 0.84 |
| 100 ms | 73.2% | 2.6% | 0.84 |
| 200 ms | 77.6% | 4.3% | 0.86 |
| 300 ms | 81.9% | 6.1% | 0.88 |

Smoothing mostly trades pause false positives for a little recall; 100 ms is a
reasonable compromise, and the analysis below uses it.

A hangover after the probability falls is another way to raise recall. At
threshold 0.7 with 100 ms smoothing, a 200 ms hangover raises speech recall
from 60% to 92% but pause false positives from 1.4% to 10%; it also raises the
music false-positive rate. It is not a fix for the non-speech problem, only a
way to smooth the speech/silence edges.

### Music and noise

Every frame in these files is non-speech, so the percentage marked is a false
positive rate. The longest run is the longest continuous stretch above the
threshold; a long run would hold ducking down for that long.

| Signal | Threshold 0.5 | 0.7 | 0.9 | 0.99 |
|---|---|---|---|---|
| Music with vocals | 74.4% (3.1 s) | 59.5% (2.4 s) | 41.7% (1.1 s) | 16.9% (0.7 s) |
| Instrumental music | 55.2% (4.7 s) | 42.2% (3.7 s) | 29.0% (2.3 s) | 10.7% (0.7 s) |
| White noise | 90.4% (13.6 s) | 65.2% (8.0 s) | 26.1% (4.5 s) | 1.5% (0.3 s) |
| Pink noise | 100.0% (36.5 s) | 99.6% (36.2 s) | 98.9% (36.1 s) | 96.4% (35.2 s) |

Music with vocals is the case the plan asks about. At 0.7 it marks 60% of the
music and stays above the threshold for 2.4 s at a time. Raising the threshold
to 0.99 cuts that to 17%, but clean-speech recall falls to 30% and pink noise
still passes almost all the time.

The VAD is not simply measuring level. A pure 1 kHz tone reads 0.997, while
digital silence reads 0.0. Pink noise at −20 and −40 dBFS reads 0.986 and
0.949; at −60 dBFS it falls to 0.005 because the crate's silence gate takes
over. White noise at −20, −40 and −60 dBFS read 0.76, 0.79 and 0.75. So the
high non-speech values are not an artefact of one playback level.

At the threshold that gives 90% recall on clean speech (0.135 with 100 ms
smoothing), the false positives are:

| Signal | Frames marked at 90% clean-speech recall |
|---|---|
| Music with vocals | 92.6% |
| Instrumental music | 76.5% |
| White noise | 100% |
| Pink noise | 100% |

That is the clearest way to read the result: there is no operating point that
keeps speech and rejects the rest.

### Speech over music

This is the voice source picking up the desktop (speakers, or a microphone
near them). The speech frames are the same as before; the pause frames now
contain music. With 100 ms smoothing:

| Mix | Threshold | Speech frames marked | Pause frames marked | Longest pause run |
|---|---|---|---|---|
| Instrumental, −10 dB | 0.5 | 76.0% | 17.7% | 1.16 s |
| Instrumental, −10 dB | 0.7 | 63.4% | 13.0% | 0.82 s |
| Instrumental, −10 dB | 0.9 | 43.2% | 7.1% | 0.56 s |
| Instrumental, −20 dB | 0.5 | 74.8% | 6.9% | 0.34 s |
| Instrumental, −20 dB | 0.7 | 62.4% | 4.8% | 0.31 s |
| Instrumental, −20 dB | 0.9 | 47.0% | 3.4% | 0.29 s |
| Vocals, −10 dB | 0.5 | 81.5% | 48.1% | 0.97 s |
| Vocals, −10 dB | 0.7 | 68.9% | 34.6% | 0.85 s |
| Vocals, −10 dB | 0.9 | 52.6% | 23.1% | 0.83 s |

At −20 dB the leakage is mild enough that the VAD is mostly right about the
pauses (5% false positives at 0.7). At −10 dB with instrumental music it marks
13% of the music-only pauses; with singing under the speech it marks 35%.
The plan already says speakers spoil the SII measurement; this shows the VAD
part is spoiled too, not just the levels.

When the music is a separate source and the microphone is clean, none of this
leakage is present, and the clean-speech figures apply. The mixture is the
speaker/leakage case.

### Separation

A threshold is only as good as the overlap between the speech and non-speech
probability distributions. The area under the ROC curve summarises it: 0.5 is
chance, 1.0 is perfect separation. With 100 ms smoothing, using the clean
speech frames as the positive set:

| Comparison | AUC |
|---|---|
| Speech against pauses in the speech track | 0.984 |
| Speech against music with vocals | 0.544 |
| Speech against instrumental music | 0.655 |
| Speech against white noise | 0.555 |
| Speech against pink noise | 0.195 |

The clean-speech case is excellent. Every non-speech case is near chance,
and pink noise is worse than chance: its frames are more likely to score high
than the speech frames. That is why no threshold in the table works.

### A check against reference RNNoise

nnnoiseless is a port of Xiph's RNNoise, and the crate's changelog says 0.5.1
reverted to the RNNoise weights "which work better for voice detection". I
built the current [xiph/rnnoise](https://github.com/xiph/rnnoise) C library
(commit `70f1d25`, 2025-02-22) and ran its `rnnoise_process_frame` on the same
S16 files. Its VAD is not the same as the crate's:

| Comparison | nnnoiseless 0.5.2 | reference RNNoise |
|---|---|---|
| Speech against pauses | 0.984 | 0.995 |
| Speech against music with vocals | 0.544 | 0.687 |
| Speech against instrumental music | 0.655 | 0.911 |
| Speech against white noise | 0.555 | 0.933 |
| Speech against pink noise | 0.195 | 0.931 |

The reference is better on instrumental music and noise, but still not
reliable on music with vocals: at the threshold giving 90% clean-speech recall
it marks 66% of the music-with-vocals frames, 21% of the instrumental frames
and about 9% of each noise file. Some of the difference may be the model
version; the point is that the crate's own numbers are what clipt would get.

### CPU

On this machine (AMD Ryzen 7 3800X, one thread, release build):

| Audio | Per 10 ms frame | Per 20 ms of audio | Real-time factor |
|---|---|---|---|
| Non-silent (music with vocals) | 0.044–0.045 ms | 0.089 ms | 224x |
| Speech with pauses | 0.038 ms | 0.076 ms | 262x |

The speech file is faster because the crate skips the network on frames its
silence gate catches. At real time, one thread uses about 0.45% of a core
(0.089 ms per 20 ms). The copy and scale around the call add about 0.0002 ms
per frame, so the whole loop is essentially the same as `process_frame`. The
measurement is wall-clock on an otherwise idle machine, repeated 10–20 times
per file; the spread across repeated runs was under 0.001 ms per frame.

### What it means for phase 4

- Use nnnoiseless as a speech-versus-silence gate on a known-clean voice
  source. At threshold 0.5 and 100 ms smoothing it keeps about 73% of speech
  frames and passes 2.6% of pauses; at 0.3 it keeps 84% and passes 3.5%. A
  200 ms hangover at 0.7 reaches 92% recall with 10% pause false positives.
- Do not use it to reject music or noise. If the microphone hears the
  desktop, the VAD passes music-only frames, and the 10th-percentile SII is
  measured on a mix of speech and music. The plan's "speakers spoil it" risk is
  real, and the frame selection is wrong as well as the levels.
- The practical choices for the plan are now clearer: either make a clean
  voice source a requirement for `smart` (headphones, quiet room; say so in
  the UI and README), or add a second discriminator. Possibilities include
  comparing the voice source with the other sources (the same music appearing
  in both means leakage), running YAMNet on the voice source, or using a
  different VAD (Silero, webrtc-vad) despite the dependency cost the plan
  lists. The reference RNNoise result says switching to the C library alone
  would not solve music with vocals.
- The plan's e2e idea uses speech-shaped, syllable-rate modulated noise as a
  stand-in for a voice. That will exercise the ducking path, but it will not
  catch the failure above, which is about music and noise being scored as
  speech. Add a case with music under the voice, or a music-only source, if the
  tests are meant to cover this.
- YAMNet itself is still worth doing on the licence side: Apache-2.0, a
  verified hash, 16 MB, shippable in the Flatpak. The class map is the only
  licence wrinkle, and hard-coding the indices avoids it.

## Limitations

- The speech reference is the edit, not a human label. The segments were
  chosen from continuous runs, but brief internal pauses still count as
  misses, so the speech recall figures are slightly pessimistic.
- The music is one song, played from 0:30. A different track, mix or level
  could score differently. Using the vocal and instrumental versions of the
  same song controls for the music but not for other music.
- The noise is synthetic. Real room noise, fans and traffic may differ; pink
  noise is a deliberately hard case, not a typical office.
- The VAD was run on mono. clipt's voice source is stereo; it would have to
  downmix or use one channel. nnnoiseless itself is a 48 kHz mono model, and
  clipt's audio is already 48 kHz, so no resampling is needed.
- The CPU figures are from one machine. The crate is pure Rust and single
  threaded, and the cost is small enough that the exact figure is unlikely to
  change the decision.
- The Kaggle page's licence field could not be read from its static HTML. The
  upstream licence statement rests on the exporter's provenance table and
  Google's repository licence, tied to the same archive by the SHA-256.
