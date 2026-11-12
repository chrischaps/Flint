# ADR 0086: A placed sound's falloff curve

Status: accepted, 2026-10-17 ([#54](https://github.com/chrischaps/Flint/pull/54)).
Backfilled 2026-11-12 (#374) from the ADR text in PR #54's description and
commit `974c998`, whose message calls it [ADR 0085](0085-script-drawing-over-egui.md): #53 took 0085 at the
same time, and the PR's title and body were renumbered to 0086.

## Context

Since [ADR 0084](0084-extensions-reach-the-audio-engine.md) a placed
one-shot could choose its `(min, max)` distances from Rust, but not the
curve between them. Kira fades a spatial track by `-60·r^p` dB, where `r`
is the listener's fraction of the way from min to max. Flint fixed `p` at 2
(`Easing::OutPowf(2.0)`). That curve holds a sound up for most of the way
and drops it near max, which is right for a room and wrong for open ground.
A real sound loses about 6 dB per doubling of distance, steeply near and
slowly far. The Capitol's soundscape was tuned by ear on `(32/d)^1.2`; the
best min and max on the fixed curve were still up to 6 dB off between 32 and
160 m. Separately, scripts could choose neither distance nor curve, so a
script sound further than 25 m away was silent, and a flat `play_sound` had
no pitch.

## Decision

Expose the exponent as `rolloff`, beside `distances`, everywhere a placed
one-shot is made.

- One number, Kira's own. `rolloff` is the `OutPowf` exponent, so there's
  no new curve maths. The docs give the formula and two rules of thumb: 2
  holds a sound up; below 1 fades it like a sound in the open. Values under
  0.01 are clamped.
- Defaults keep today's sound. The default rolloff is 2
  (`DEFAULT_ROLLOFF`), and the default distances are 1 and 25 m.
  `audio_source` entities aren't changed: they still use
  `create_spatial_track` with the fixed curve.
- An options map for scripts, like `draw_text_opts`, rather than a
  nine-argument overload: `play_sound_at(name, x, y, z, opts)` with
  `volume`, `pitch`, `min_distance`, `max_distance`, `rolloff`, every key
  optional. `play_sound(name, volume, pitch)` gives a flat sound pitch.
- Rust: `SoundOptions::rolloff`; `create_spatial_track_with(position,
  (min, max), rolloff, bus)`; `play_oneshot_at` takes a `rolloff` before
  `bus`.

## Alternatives considered

- Other curve families (Kira's `InPowf` and the rest). Not done, because
  one exponent fits the cases there were.
- A nine-argument script overload, instead of the options map.

## Consequences

- A game can place a sound where it really is, at any scale, and fade it
  like a real one. The Capitol dropped its workaround of putting every
  world sound 3 m from the camera and setting the level itself.
- Breaking: `AudioEngine::play_oneshot_at` (added in #52) gained an
  argument, and `ScriptCommand::PlaySound` / `PlaySoundAt` gained fields.
- Not done: `audio_source.rolloff`, which would be a schema-level change,
  and sound handles for playing one-shots. Still true: an `audio_source`
  plays on `DEFAULT_ROLLOFF` (`flint-audio/src/sync.rs`), and its schema
  has no `rolloff` field.

## Measured

Rolloff 0.6 over 32 to 1250 m is within 1.5 dB of `(32/d)^1.2` out to
300 m (a Capitol unit test). A 30 s offline run played 7 birds with no
audio warnings.

## Sources

- [#54](https://github.com/chrischaps/Flint/pull/54) / commit
  [`974c998`](https://github.com/chrischaps/Flint/commit/974c998a9f6eb77a8980a50f102797578ce24b72)
  "[H19] Flint: a placed sound's falloff curve, and script pitch and
  falloff ([ADR 0085](0085-script-drawing-over-egui.md))"
- Vault: "The Capitol - sound falloff where it really is (H20, GH-94) -
  2026-10-17"
- Code now: `crates/flint-audio/src/engine.rs` (`DEFAULT_ROLLOFF`,
  `play_oneshot_at`, `create_spatial_track_with`),
  `crates/flint-player/src/extension.rs` (`SoundOptions::rolloff`); book: [Audio](../book/src/concepts/audio.md)
