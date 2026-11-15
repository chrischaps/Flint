# ADR 0020: Scripts read the session through a per-frame conducted snapshot

Status: accepted, 2026-08-16 (commit
[`72e2b7e`](https://github.com/chrischaps/Flint/commit/72e2b7ebf3b24b319a23df39744dbd0c893cd7bb)).
Backfilled 2026-11-12 (#374) from commits `72e2b7e` and `27be5c4`, the code
that cites it, the book and the Vault note "2026-08-16 Star Child F4 -
Conducted Parameters". The original record was in the Starchild repo, which
no longer exists; this is a reconstruction. Amended in form by
[ADR 0116](0116-declared-script-api.md), which declares the getters as the
`conducted` module.

## Context

F6 would build a landscape that breathes with the player, written in Rhai
scene scripts. Those scripts needed the session's state: lean, target,
coherence, musical position, judged pulses and the disintegration ramps.
`flint-script` must not depend on `flint-music`.

## Decision

- **One producer.** `VisualFrame` becomes `ConductedFrame` (with bar, beat,
  section and pulses), built by `ChartCore::conducted_frame`, so the live
  session and headless replay share one deterministic producer. Pulse ages
  are suite-sample arithmetic; the wall-clock `Instant` is gone.
- **Pulses per frame.** `process()` fills a per-frame buffer of judged pulses
  (hit, miss or spurious, with `err_ms`), cleared at its top, and the
  judgment log stays byte-identical.
- **A plain mirror in `flint-script`.** `ConductedSnapshot` is POD with no
  `flint-music` dependency. The player converts the frame to it in one place
  and calls `set_conducted` each tick, after the session's tick and before
  `on_update`.
- **`conducted_*` getters** with neutral defaults when no session runs: a
  clean, settled world, coherence and reassembly 1.0. A binding such as
  `1 - conducted_coherence()` shows nothing in an ordinary scene. The Vault
  note: "the absence of music is peace, not zero."
- `music_session.bindings` is validated, not loaded
  ([ADR 0019](0019-music-session-component-in-the-player.md)'s deferral).

## Alternatives considered

- **Hide `err_ms` from scripts**, against the temptation to build a score UI
  from it. Chris chose to expose it; the risk is handled at scene review
  instead (Vault).

## Consequences

- The surface is read-only: nothing in Rhai can change a bus gain, a rung or
  the chart (the book).
- Ages jump backwards across a reintegration seam
  ([ADR 0014](0014-reintegration-seam-on-one-clock-tick.md)). The Vault note
  calls that correct: "the world runs on musical time, and musical time
  rewinds."
- The F3 direct ladder-to-post path was kept as a fallback
  ([ADR 0021](0021-ladder-visuals-through-the-post-stack.md)).
- Since then: the getter family grew (`conducted_next_target`,
  `conducted_next_pulse`, `conducted_cues` from [ADR 0033](0033-chart-cues-fire-with-params.md), `conducted_rewind`,
  `conducted_no_input`, `conducted_preroll` and others); under [ADR 0116](0116-declared-script-api.md) they
  are a declared module that #698 gates behind the music session. The
  2026-08-20 `tick` decomposition (`27be5c4`) made the session → conducted →
  context order readable at the call site.

## Measured

`tests/conducted_trace.rs`: bit-identical twice plus a field battery. A live
`music_dev` probe ran clean at 0.000 ms confirmed skew.

## Sources

- Commit [`72e2b7e`](https://github.com/chrischaps/Flint/commit/72e2b7ebf3b24b319a23df39744dbd0c893cd7bb)
  "F4: conducted-parameters script surface (ADR 0020)"
- Commit [`27be5c4`](https://github.com/chrischaps/Flint/commit/27be5c4fe45b0485402a384841aa98896f3e8fc2)
  "refactor(player_app): decompose tick into named steps"
- Vault: "2026-08-16 Star Child F4 - Conducted Parameters"
- Code now: `crates/flint-music/src/chart_session.rs` (`ConductedFrame`,
  `ChartCore::conducted_frame`), `crates/flint-script/src/context.rs`
  (`ConductedSnapshot`), `crates/flint-script/src/api/conducted.rs`,
  `crates/flint-player/src/player_app/frame.rs`,
  [Scripting](../book/src/concepts/scripting.md),
  [Music Sessions](../book/src/concepts/music-sessions.md), "Scripts"
