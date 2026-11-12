# ADR 0084: Extensions reach the audio engine through `ExtensionAudio`

Status: accepted, 2026-10-17 ([#52](https://github.com/chrischaps/Flint/pull/52)).
Backfilled 2026-11-12 (#374) from the ADR text in PR #52's description and
commit `69ddf42`.
Extended by [ADR 0086](0086-placed-sound-falloff-curve.md) (`rolloff`).

## Context

`ExtensionCtx` had the world, the camera, the GPU and the persistent store,
but not audio. An embedding game couldn't play a sound from Rust, couldn't
load a sound it made in memory, and couldn't set a mixer bus. The Capitol
worked round all three: it fired a custom event for every one-shot and had a
Rhai script call `play_sound` (a script hop and a frame of latency per UI
click); it wrote about 11 MB of synthesized WAVs into `audio/` before scene
load so the preload would find them (0.2-0.5 s on a first start); and its
volume settings scaled every sound itself instead of setting the buses.

## Decision

Give `ExtensionCtx` a narrow handle, `ExtensionAudio`, rather than
`&mut AudioSystem`. It exposes `play(name, &SoundOptions)`, `load_samples`
(one or two channels of f32), `load_bytes` (WAV/OGG/FLAC/MP3 in memory) and
`set_bus_volume`, plus `is_available` and `has_sound`.

- One cache. Loaded sounds go into the engine's existing sound cache, keyed
  by name, so a sound an extension loads can be played by scripts and named
  by an `audio_source`, and a preloaded `audio/<file>` can be played from
  Rust.
- Options, not positional arguments. `SoundOptions` (volume, pitch, bus,
  `at: Option<Vec3>`, `distances: (min, max)`) defaults to full volume,
  normal pitch, the sfx bus, flat, and 1-25 m, the falloff `play_sound_at`
  uses. A field can be added later without breaking callers.
- Silent without a device. Loading works with no device; `play` returns
  `Ok` and does nothing. An unknown name is still an error.
- Replace on load. Loading under an existing name replaces the cached data.
  Sounds already playing keep the old data.
- `flint-audio` gains `AudioEngine::load_sound_from_samples`,
  `load_sound_from_bytes` and `play_oneshot_at`; `play_at_position`
  delegates with its 1-25 m default, so scripts are unchanged.

## Alternatives considered

- Handing extensions `&mut AudioSystem`. A narrow handle was chosen; the
  surviving text gives no further reason.

## Consequences

- Games can synthesize or embed their audio and never touch the
  filesystem, set volume settings on the buses, and play UI sounds with no
  script hop.
- A looping `audio_source` whose sound isn't cached when the entity syncs
  stays silent, so callers load before they spawn.
- Not done: sound handles (stop, fade, retune a playing one-shot); looping
  sounds stay `audio_source` entities. Scripts couldn't yet choose a placed
  sound's falloff or a flat sound's pitch. Since then:
  [ADR 0086](0086-placed-sound-falloff-curve.md) did that, and added
  `SoundOptions::rolloff`.

## Measured

In The Capitol, the 16-sound synthesized library loads in 5 ms.

## Sources

- [#52](https://github.com/chrischaps/Flint/pull/52) / commit
  [`69ddf42`](https://github.com/chrischaps/Flint/commit/69ddf4290e4752a20daf3bfc22b528e6fbd30cec)
  "flint-player: extensions reach the audio engine (ADR 0084)"
- Vault: "The Capitol - sound falloff where it really is (H20, GH-94) -
  2026-10-17"
- Code now: `crates/flint-player/src/extension.rs` (`ExtensionAudio`,
  `SoundOptions`); book:
  [Player Extensions](../book/src/guides/player-extensions.md), "Sound",
  and [Audio](../book/src/concepts/audio.md)
