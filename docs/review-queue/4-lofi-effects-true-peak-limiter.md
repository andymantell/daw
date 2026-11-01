# True-peak limiter and lo-fi effects

Same `daw.exe`. The limiter now holds its ceiling between samples too,
and three new effects in **Add effect…** under "Built in" give lo-fi
character: **Vinyl**, **Tape** and **Bit crusher**. Built in the cloud;
nothing here has been listened to yet.

## Limiter

The limiter used to watch only the samples, so a peak between two
samples (what a player's converter or an MP3 encoder makes of them) could
go over the ceiling: in the assistant's song test a -1.5 dB ceiling let
-1.3 dBTP through. It now watches the true peak, 4× oversampled with the
filter of ITU-R BS.1770 Annex 2 (the same one the meters use), and aims
0.05 dB under the ceiling to allow for the gain moving. Its delay grew by
6 frames (78 frames at 48 kHz), which the track's delay compensation
takes care of.

- [ ] **Limiter on the master.** Put the Limiter last on the master,
      Ceiling -1.0 dB, and push Drive until it works hard on a loud mix.
      Pass: the master meter's true peak never reads above -1.0 dBTP,
      and an exported WAV analysed in another tool agrees.
- [ ] **Limiter sound.** Compare with the old build if you have it.
      Pass: no new pumping or dullness at the same settings.

## Vinyl

Hiss, crackle (how often, and how loud), rumble, and Wear, which
narrows the band as a worn record does (highs from 20 kHz down to
5 kHz, lows thinned below about 120 Hz). It makes sound of its own, so
it hisses and crackles even when the track is silent.

- [ ] **Vinyl.** On a drum loop or a piano. Pass: the crackle sounds
      like dust on a record rather than digital clicks; turning Crackle
      up gives more of it; Rumble is a low, felt rumble; Wear at 100%
      sounds like an old, thin record. Say whether you'd like a scratch
      or a once-a-turn tick added.

## Tape

Wow (a slow, slightly uneven pitch wander) and flutter (a faster
waver), each with its speed; Saturation (the app's saturation, one warm
stage); Highs (a steep low pass, off at 20 kHz); and Hiss (off by
default). It delays the sound by about 4.6 ms, which delay compensation
takes care of.

- [ ] **Tape.** On keys or a guitar chord. Pass: the defaults sound like
      a worn cassette, not seasick; full Wow at the default speed is
      clearly woozy; Flutter adds a shimmer; Saturation warms without
      getting louder.

## Bit crusher

Bits (1 to 24, Off at 24), Sample rate (400 Hz to 48 kHz, Off at the
top), Drive (how much of the range the sound uses before it's rounded,
with the level put back after), Smooth steps (low passes either side of
the hold, which take most of the aliasing away; off by default for the
gritty sound), and Mix.

- [ ] **Bit crusher.** On drums. Pass: 8 bits and 12 kHz sounds like an
      early sampler; fewer bits gets noisier and fizzier; lower rates
      get darker and more metallic; Smooth steps makes it duller and
      cleaner.

## Assistants

Every setting of the three effects is reached by value:
`list_effect_kinds` lists `vinyl`, `tape` and `bit-crusher`,
`add_effect` adds them, and `list_parameters` and `set_parameter` reach
each setting (tested in `agent_routing`).
