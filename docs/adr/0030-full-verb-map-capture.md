# ADR 0030: The capture thread speaks a selectable verb map; `full` adds sway, pressure, press and flick

Status: accepted, 2026-08-19 (commit
[`86d21ec`](https://github.com/chrischaps/Flint/commit/86d21ec404a5cf822956fcb4bf04058c6a2e3fe5)).
Backfilled 2026-11-12 (#374) from commit `86d21ec`, Vault note "Star Child
World II - The Undertow - 2026-08-17", the module doc of
`crates/flint-input-capture/src/lib.rs`, and the book.
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.

## Context

The prototype played on two verbs: `lean` (left stick) and `pulse` (South
or the right trigger). Star Child's World II, the Undertow, was chosen
because its fiction needed the systems still untested, the full verb space
among them. The engine side landed in one commit covering ADRs 0030-0037.

## Decision

- Physical-to-verb mapping is a `VerbMap` profile on the capture thread,
  chosen by `input_map` on the `music_session` component or `--input-map`
  on the CLI harnesses.
- `prototype` (the default) is the old behaviour byte for byte: left stick
  is `lean`, South or R2 is `pulse`.
- `full`: left stick is `lean`; South is `pulse`; trigger depth becomes the
  `pressure_l` and `pressure_r` streams, with a `press` event on a rising
  onset; right stick becomes `sway`, plus a flick detector (quiet to hard
  deflection within a beat-scale instant) that emits `flick` with a
  direction.
- Press depth is judged from the pressure stream, never carried on the
  event.
- Under `full`, R2 no longer emits a plain `pulse`: emitting both would put
  a spurious pulse on every squeeze.
- Charts name verbs, never buttons.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- Every committed prototype artifact stayed byte-identical through the
  change, checked by A/B runs before and after (the commit and the Vault
  note both say so).
- The new verbs reach judgment ([ADR 0031](0031-per-channel-judgment-and-coherence-weights.md)), coherence, the conducted frame
  and Rhai (`conducted_sway()`, `conducted_pressure_l()`,
  `conducted_pressure_r()`).
- Since then: the gilrs pad wrapper moved to `flint-runtime` (#748), and the
  crate builds without the `gamepad` feature (#221), where capture fails at
  its start. On wasm32 the capture thread compiles out; the verb maps still
  build.

## Sources

- Commit
  [`86d21ec`](https://github.com/chrischaps/Flint/commit/86d21ec404a5cf822956fcb4bf04058c6a2e3fe5)
  "World II baseline (engine): full verb map, per-channel judgment,
  alternates, cues, debug panels"
- Vault: "Star Child World II - The Undertow - 2026-08-17"
- Code now: `crates/flint-input-capture/src/lib.rs` (`VerbMap`),
  `crates/flint-music/src/input_stream.rs` (`SwaySample`, `PressureSample`),
  `crates/flint-cli/src/commands/play_chart.rs` (`--input-map`),
  [Music Sessions](../book/src/concepts/music-sessions.md), "Verb Space"
