# Classic synths, clean-room: what to build, and in what order (2026-11-01)

The deep house test song's bass came from the built-in poly-synth and
sounded thin and "farty". This plan adds a family of instruments, each
built on the architecture of a classic synth, described from public
sources and written from scratch. No code, samples or factory sounds
from anyone else. Synths are described by their architecture and era,
not by product name, as the project's plan rule asks.

## Why the current synth sounds basic

The poly-synth (`crates/instruments/src/synth.rs`) is clean and correct,
and that is the problem. What a classic synth adds, and ours lacks:

| Classic synths | Ours |
|---|---
| A 4-pole filter whose stages saturate; resonance "sings" and the sound thickens as the input gets hotter | One linear 2-pole state-variable filter; no saturation anywhere in the path |
| Mixer and amplifier stages that overdrive gently | Linear mixer and amplifier |
| Oscillators that drift slightly and sit at random phases, so voices never line up | Exactly stable pitch, fixed unison phases |
| Envelopes with analogue curves and quirks (ADS with shared decay/release, attack that restarts from the current level, fast 1 ms attacks) | Linear attack, exponential falls, one shape |
| Oversampling around the nonlinear parts, so drive doesn't alias | None |
| Built-in chorus, sub-oscillators an octave or two down with their own shapes, hard sync, cross-modulation, pulse-width modulation from the envelope | Sub square only, no sync, no cross-mod, PWM only by hand |

The bass sounded "farty" because a fast resonant filter decay on a
linear 2-pole filter with no drive is exactly that: a clean "wah" with
nothing behind it.

## What to build

### Step 0: a shared vintage toolkit (one thread, lands first)

A module `crates/instruments/src/vintage/` the synths share. Every
later thread uses it, so it lands as an early PR with tests that check
each piece against its known response.

- **Filters**, all from published descriptions (see Sources):
  - 4-pole transistor ladder with saturating stages and resonance
    compensation (the 1970s American monosynth filter).
  - 4-pole diode ladder with inter-stage feedback and a high-pass in the
    resonance path (the acid bass filter).
  - 4-pole transconductance cascade with a 2-pole tap (the Japanese and
    Curtis-era poly filters), self-oscillating.
  - 2-pole state-variable with a continuous low-pass → notch → high-pass
    sweep and a band-pass switch (the American 2-pole poly/module filter).
  - 2-pole Sallen-Key pair, high-pass into low-pass, with a saturating
    resonance path that distorts on purpose (the Japanese semi-modular).
  - Non-resonant high-pass, stepped or continuous.
  All built by the topology-preserving (zero-delay feedback) method,
  with 2× oversampling around the nonlinear ones.
- **Oscillators**: band-limited saw, pulse, triangle and shark-tooth,
  hard sync with band-limited resets, pulse-width modulation from any
  source, sub-oscillator shapes (one and two octaves down, narrow
  pulse), per-voice drift (slow random walk plus a fixed per-voice
  offset) with a "vintage" amount, and a DCO mode (stable pitch, drifting
  pulse width and level) for the digitally controlled synths.
- **Envelopes**: analogue curves (RC-shaped attack, exponential decay),
  ADS with decay shared by release, ADR with an initial level and an
  attack level, retrigger-from-current-level, and an 8-value rate/level
  envelope for the FM synth.
- **Saturation**: soft clip for mixer overdrive, amplifier and output.
- **Bucket-brigade chorus** model: two channels modulated in opposite
  directions, band-limited and slightly noisy, with two rates and both at
  once.
- **Voice handling** shared by all: polyphonic, unison with stacking,
  mono with low/high/last priority and legato, glide (constant time and
  constant rate).
- **Settings and the assistant**: every synth uses the same `Params`
  table, so `get_instrument`, `set_parameter` and automation of
  instrument settings (the thread already running for that) work for all
  of them without more plumbing.

### The synths (one thread each)

| # | Synth (by architecture) | What it is for | Key features |
|---|---|---|---|
| S1 | **Three-oscillator ladder monosynth** (1970, American) | Bass, leads | 3 oscillators (osc 3 doubles as an LFO), triangle/shark-tooth/saw/three pulse widths, six footages, white and pink noise, mixer overdrive, 24 dB ladder with 0/⅓/⅔/full key tracking, two ADS(D) contours, glide, low/high/last priority |
| S2 | **Acid bass line synth** (1982, Japanese) | Acid and techno bass | Saw or square, 24 dB diode ladder, AD filter envelope, fixed-decay amplifier envelope, accent (shorter decay, louder, filter sweep that piles up on consecutive accents), slide from overlapping notes. Accent = velocity over a threshold, slide = overlapping notes, so patterns and note clips drive it |
| S3 | **One-oscillator bass/lead monosynth** (1982, Japanese) | Bass, leads, classic house | Saw + pulse with PWM from envelope/LFO/manual, sub-oscillator (−1 oct, −2 oct, −2 oct narrow pulse), noise, 24 dB cascade filter to self-oscillation, one ADSR, LFO with triangle/square/random/noise, envelope or gate amplifier, portamento |
| S4 | **Six-voice DCO polysynth with chorus** (1984, Japanese) | Pads, chords, house stabs, bass | Saw + pulse (PWM) + square sub + noise, 4-step high-pass, 24 dB self-oscillating low-pass, one invertible ADSR to filter and amp, triangle LFO with delay, stereo bucket-brigade chorus I / II / both, poly, unison |
| S5 | **Two-oscillator analogue polysynth with two filter boards** (1978–81, American) | Brass, pads, leads, bass | 2 VCOs (saw/tri/pulse, PWM), hard sync, poly-mod (filter env and osc 2 → osc 1 pitch, pulse width, cutoff), noise, filter board A: 4-pole cascade; board B: 2-pole state-variable (low/notch/high/band), two ADSRs with velocity, LFO with saw/tri/square all at once, unison stacking, poly glide, a "vintage" amount |
| S6 | **Eight-voice two-channel touch polysynth** (1977, Japanese) | Big pads, brass, expressive leads | Two full channels per voice (saw, square with PW/PWM, noise; sine added after the filter), resonant high-pass and low-pass per channel with a 12/24 dB switch, ADR filter envelope with initial and attack levels, amp ADSR, velocity and polyphonic aftertouch to level and cutoff, ribbon (pitch bend from first touch), ring modulator with its own envelope, chorus and tremolo |
| S7 | **Semi-modular dual-filter monosynth** (1978, Japanese) | Aggressive leads and bass, drones, and running guitar through its filters | 2 VCOs (triangle/saw/pulse/noise; saw/square/narrow pulse/ring mod), 12 dB high-pass into 12 dB low-pass, both self-oscillating and distorting, delay-attack-release and hold-ADSR envelopes, modulation generator with a shape control, an external input (the guitar) through the filters, a small patch matrix |
| S8 | **Six-operator FM synth** (1983, Japanese) | Electric pianos, bells, FM bass, brass, plucks | 6 sine operators, 32 algorithms, feedback, 8-value rate/level envelope per operator, frequency ratio or fixed, key level and rate scaling, velocity per operator, LFO to pitch and amplitude, pitch envelope. Reads the public bank format for the owner's own patches; ships no factory banks |
| S9 | **Eight-bit wavetable synth with an analogue filter** (1981, German) | Glassy pads, bells, 80s digital | Tables of 64 single-cycle waves made in code, 8-bit with the aliasing kept as a switch, position swept by envelope, LFO and key, two oscillators per voice, 24 dB cascade filter, ADSR envelopes |

Each synth ships with a set of sounds made in code across bass, pad,
lead and keys (the content browser already lists "made:" sounds), a
review-queue file, and screenshots.

### After these, if wanted

String machine with ensemble (divide-down organ), a vector/additive
digital poly, drum synth voices for the drum machine, and giving the
existing poly-synth the toolkit's filters and drive so old projects
improve too (small, could ride with Step 0).

## Order

1. **Step 0 toolkit** first, alone, as one PR: everything else needs it.
2. **Set 1, the basses and leads**: S1, S2, S3, S7 in parallel. These
   answer the complaint that started this.
3. **Set 2, the polys**: S4, S5, S6 in parallel, as soon as Set 1 shows
   the toolkit holds up (or at once, if preferred).
4. **Set 3, the digital pair**: S8, S9. Independent of the toolkit's
   filters apart from S9's; could run alongside Set 2.

S6 needs polyphonic aftertouch to reach instruments: the engine has the
events, so the thread wires them through and adds an aftertouch lane to
the note editor if there is none.

## Rules every synth thread follows

- Clean room: written from the public descriptions in the sources note,
  no borrowed code, tables or samples. The Sources appendix lists what
  each piece is based on, and each is a paper, book or specification,
  not a code base.
- The AI rule: commands in the shared table, an assistant tool, `// ai:`
  marks on the controls, an end-to-end test through the assistant
  interface that adds the synth, voices a bass or pad, and renders it.
- A golden render test per synth (a fixed patch renders to a known
  spectrum), so a later change to the toolkit can't silently alter it.
- CPU budget: a 16-voice poly at 2× oversampling must stay under a few
  percent of one core at 48 kHz in release builds, measured in a test.

## Sources (methods used, all public)

- Nonlinear transistor ladder: Huovilainen, "Non-Linear Digital
  Implementation of the Moog Ladder Filter", DAFx 2004.
- Ladder, diode ladder, state-variable, Sallen-Key, nonlinear feedback:
  Zavalishin, "The Art of VA Filter Design" (free book, copyright the
  author; methods used, text not copied).
- Band-limited oscillators: Välimäki and Huovilainen, polyBLEP (2007),
  already used by the current synth.
- Acid bass circuit behaviour (accent, slide, diode ladder): Kernmayer,
  "The Sound of Acid", FH St. Pölten thesis, 2020.
- Architectures: the makers' specification and reissue pages and
  long-form magazine reviews, read on 2026-11-01; the list with URLs is
  kept with the owner's project files, out of the repo, because it names
  products.

Nothing here is copied from another code base, so GPLv3 is not in
question; algorithms and circuit descriptions are not copyrightable, and
the one book with terms allows free use of its content as information.
