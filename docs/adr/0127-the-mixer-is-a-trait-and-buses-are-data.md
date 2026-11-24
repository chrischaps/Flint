# ADR 0127: The mixer is one trait with no defaults, and buses are data

Status: accepted, 2026-11-17 (#730).

## Context

`flint_audio::AudioEngine` mixes in one of three places: nowhere yet
(before a device, or the web's Begin), kira on this thread (the desktop,
and the browser's fallback), or kira in the browser's `AudioWorklet`, reached
by commands (#136). Until now that was `enum Mixer { Silent, Local, Remote }`,
and each engine method matched it inline, many with `_ =>`, `if let
Mixer::Remote` or `let Mixer::Local(..) else`. A new call compiled with no
worklet arm, so a call could work on the desktop and be silent in the
browser, a bug found only on a phone. The worklet's `Command` enum was a
second list kept equal by hand.

Buses were `enum Bus { Sfx, Music }`. A game wanting a `voice` or `ui` bus
had to edit the engine. And `AudioEngine::manager_mut()` handed the kira
manager to the music session ([ADR 0017](0017-one-shared-audio-manager.md)),
returning `None` from the worklet by a wildcard arm.

## Decision

- **`MixerBackend`** (`crates/flint-audio/src/engine/backend.rs`) is every
  operation the mixers do differently: listener, spatial tracks, plays (on a
  bus or a track, and the old mix's one-shot on its own track), bus creation
  and volume, the cutoff, the sounds it holds of its own, counts, clock and
  latency, failing, and hosting. **No method has a default body.**
  `Silent`, `Local` (`engine/local.rs`) and `Remote` (`engine/remote_mix.rs`)
  implement it, and the engine holds a `Box<dyn MixerBackend>`: it can no
  longer name a variant, so a new operation is written three times or
  nothing compiles. What is the same everywhere (the sound cache, decoding,
  placed one-shots and keyed sounds, the listener's place, the bus table and
  volumes) stays in the engine.
- **The worklet's side** is held the same way: `Command::encode` and the
  worklet's `Mixer::command` match every command with no wildcard, and
  `every_op_reads_back` fails when a command's op has no reader arm.
- **Buses are a table**, `flint_audio::Buses`: `sfx` (0) and `music` (1)
  always, then a game's, each with a parent (another bus, or the main track)
  and a starting volume. `Bus` is the number (`Bus::SFX`, `Bus::MUSIC`); the
  old variant names stay as deprecated constants for one release. A game
  declares buses in `flint.toml`'s `[audio.buses.<name>]` (`parent`,
  `volume`), read by `Buses::for_project`; the player applies it at start and
  at each scene (`PlayerApp::declare_audio_buses`). The worklet gets each bus
  as `Command::BusNew { bus, parent, db }`, numbered in order. `audio_source.bus`
  and a script's `bus` take any name, and `set_bus_volume(name, v)` is a
  script function. A held page quiets the top-level buses; a child goes with
  its parent.
- **Hosting replaces `manager_mut`.** `AudioEngine::host(|manager| ..)` runs
  content that builds its own kira graph and clock (a music suite) on the
  mixer's manager for one call; `can_host()` asks first. `MixerBackend::host`
  answers: `Local` lends its manager, `Silent` and `Remote` return an
  `Unhosted` saying why. ADR 0017's drop order is unchanged. (Since #1216,
  [ADR 0135](0135-clocked-content-plays-on-a-mixer-seam.md): `host` lends a
  `ClockedMixer`, which `Remote` implements too; only `Silent` refuses.)

## Alternatives considered

- **Keep the enum and ban wildcard arms** (clippy's
  `wildcard_enum_match_arm`, or review). The lint is crate-wide, not per
  enum, and `if let` / `let .. else` aren't wildcards: the parity would still
  be kept by hand.
- **An enum delegating through one exhaustive `backend()` match.** Keeps the
  variants nameable, so the inline matches could come back.
- **Running a suite in the worklet now.** It needs commands for a clock,
  clock-timed starts and tweens, stem tracks with filters and a reported
  clock time; that is its own ticket (named in the PR). Until then the
  worklet says it can't host, and a music scene on the web mixes on the
  main thread as before.
- **Buses in a scene table.** Buses are the game's, not a scene's: a
  transition would have to agree with the last scene's table.

## Consequences

- An operation the browser can't do yet must say so in its `Remote` body
  (as `host` does), rather than falling through an arm nobody wrote.
- A second, different table (another project's, at a transition) only adds
  buses: one already here keeps its number and the volume the game gave it.
  The same table again changes nothing, so volumes set during play survive
  scene changes.
- `--music-volume` / `--sfx-volume` scale the volume the game's table starts
  those buses at, instead of replacing it.
- Not done here: user volume settings saved through `user_files`, ducking,
  and per-bus effects beyond the master low-pass. The book's Audio page,
  Mixer Buses, teaches the table.
