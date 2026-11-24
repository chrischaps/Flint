# ADR 0135: Clocked content plays on a mixer seam the worklet implements

Status: accepted, 2026-11-23 (#1216). Follows
[ADR 0127](0127-the-mixer-is-a-trait-and-buses-are-data.md) and
[ADR 0017](0017-one-shared-audio-manager.md). Book:
[Music Sessions](../book/src/concepts/music-sessions.md), Keeping Time.

## Context

Since #136 the browser mixes in an `AudioWorklet`: the page posts commands
and the worklet's own kira plays them, off the main thread. A music suite
(`flint_music::SuiteSession`) didn't fit. It built a kira graph of its own:
a clock counting samples, a sub-track with a low-pass per bus, every stem
started on one `StartTime::ClockTime`, and clock-timed tweens for the event
script's changes and a rhythm game's timeline jumps. The protocol had no
command for any of that, so after #730 `MixerBackend::host` answered
`Unhosted::Worklet` in the browser and `PlayerApp::start_audio` forced a
scene with a `music_session` onto the main thread. #136 had measured what
that costs on a phone: 26.9% of the main thread, and 46 late buffers in
1356 on Float. A music game paid all of it.

What a suite asks of kira is small. Make a clock and start it. Make a stem
bus with a low-pass. Play a stem on a bus from a frame, at a tick. Change a
bus's gain or cutoff, or a stem's volume or rate, at a tick, over seconds.
Stop a stem at a tick. Read the clock, and a stem's position and end.

## Decision

- **The seam is `flint_audio::clocked::ClockedMixer`**: `add_clock`,
  `add_stem`, `add_bus`, and the handles they return (`MixClock`, `Stem`,
  `StemBus`, `StemVoice`). Every change takes a `Ramp`: linear, over
  seconds, starting `Start::Now` or `Start::Tick(n)` on the clock its bus was
  made on. No kira type is in the suite's API any more; `flint-music`
  re-exports the module as `flint_music::clocked`.
- **Two implementations.** `AudioManager<B>` for any kira backend (the
  engine's stream, the device, the offline renderer): the handles wrap
  kira's. `RemoteMixer` over a `Link`: each call is a `protocol::Command`
  (`ClockNew`, `ClockStart`, `StemBusNew`, `StemBusVolume`, `StemBusCutoff`,
  `StemPlay`, `VoiceVolumeAt`, `VoiceRateAt`, `VoiceStopAt`, and a drop for
  each kind), a stem is sent once as samples, and an `At { clock, tick }`
  carries the start. Handles behave like kira's: dropping a voice lets it
  play on, dropping a bus or the clock takes what is on it.
- **The clock comes back in the reports.** The worklet's `Mixer::report`
  lists each clock beside the watched voices (#658), with its ticks and
  `state::CLOCK`. `MixClock::ticks` reads the last report carried forward on
  the audio clock (`Link::clock`), by at most 0.1 s, and never reads less
  than it read before. Stem voices are watched, so their positions and ends
  come back the same way.
- **`MixerBackend::host` lends a `&mut dyn ClockedMixer`**: `Local` its kira
  manager, `Remote` its `RemoteMixer`, `Silent` refuses. `Unhosted::Worklet`
  is gone, and `start_audio` no longer moves a music scene to the main
  thread.
- **A moved mix restarts the suite.** `AudioEngine::mixer_epoch` counts the
  mixers installed. If the worklet fails and the engine falls back to the
  main thread, the suite's handles went with the worklet, so
  `MusicSubsystem` sees the epoch change and starts the suite again on the
  new mixer.

## Alternatives considered

- **Keep music on the main thread** (the state before). It works, and it
  costs a music game the main-thread mix #136 measured.
- **The trait in `flint-music`, its remote side in `flint-player`.** That
  would keep `flint-music` off `flint-audio`. But the remote handles are the
  worklet protocol's, and the protocol, the `Link` and the worklet's tests
  are `flint-audio`'s. `flint-music` now depends on `flint-audio` (without
  its `device` feature).
- **A command that sets the start time of the commands after it.** That
  would be fewer new ops, but the reader would have to keep state, and a
  dropped command would shift the timing of the ones after it. Each timed
  command carries its own `At`.
- **Report the clock on a separate channel.** The voice report already
  crosses every 1024 frames with an audio-time stamp. A clock is one more
  row in it.

## Consequences

- A suite plays the same on the desktop and in the browser. Both are kira at
  128-frame chunks, so a stem lands on the same sample:
  `crates/flint-music/tests/worklet_parity.rs` renders one suite offline and
  on the worklet's `Mixer` through a `Link`. All 16 clicks start on the same
  samples, and the gain and low-pass changes land on the same beats. kira
  starts a clock-timed sound at the start of the chunk its tick falls in,
  128 samples early. A change lands on its tick, gliding across that chunk.
  That is kira's behaviour on both sides, not something this seam adds.
- `flint-rhythm` runs on the seam too (`ChartSession::open_shared(cfg,
  &mut dyn ClockedMixer)`, the ladder's and reintegration's ramps, the
  pickup cue as a `StemVoice`), so a charted suite in the browser plays in
  the worklet as well. Its subsystem doesn't yet restart when the mix
  moves.
- A stem crosses to the worklet as samples, once. A long suite's stems are
  as large in the worklet as they are decoded on the desktop.
- A new kind of clocked change is a method on a handle with both arms, and a
  `Command` with its op, reader arm and example (`every_op_reads_back`).

## Measured

- `worklet_parity.rs`: the click onsets in both renders are identical,
  sample for sample (23 872, 47 872, ..., 383 873). The 12 dB step at the
  second bar comes out as the same ratio on both sides (within 2%). The hum's
  RMS between clicks is 0.035 before the low-pass at the third bar and
  3.5e-5 after it, in both renders. After four bars the session's clock,
  read from the worklet's reports, is within one quantum of what was
  rendered.
