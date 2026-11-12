# ADR 0015: The disintegration ladder is data: absolute rungs with hysteresis

Status: accepted, 2026-08-15 (commit
[`aad0772`](https://github.com/chrischaps/Flint/commit/aad0772098dcf4ccb67ac5429aa3a13cad1de1b0)).
Backfilled 2026-11-12 (#374) from commit `aad0772` (subject only), the
commits that built on it, `ladder.rs`'s module doc, the book and the Vault note
"Starchild Phase 3 - Where the Signal Actually Lives". The original record was
in the Starchild repo, which no longer exists; this is a reconstruction.

## Context

As coherence falls ([ADR 0010](0010-coherence-as-asymmetric-leaky-integrator.md)),
the world's audio and visuals should come apart, and re-gather as it rises,
"as one thing". The E1 commit's body is empty; what the decision says is read
from `ladder.rs` as committed then and now.

## Decision

- **Data, not code.** The ladder is `config/ladder.toml` in the game repo,
  hot-reloadable mid-session like the coherence config.
- **Ordered rungs**, each with a coherence threshold and its parameters, an
  audio half and a visual half.
- **Absolute parameters.** Each rung states the whole degraded state; a deeper
  rung's numbers include the shallower rungs' degradation.
- **Hysteresis in the thresholds** (`enter_below` < `exit_above`), so a noisy
  value at a boundary never flickers the world.
- **One source of truth.** The resolved `LadderParams` for the current rung
  drives both halves: the audio driver applies it as mixer tweens, the visual
  frame carries the rest.
- **Protected buses.** Gain and dropout never touch `foundation`, the world's
  anchor ("last to go" means it never goes in the prototype), or the motif
  buses (`home_theme`, `child_motif`), the lines reintegration leads with.
  The low-pass applies to every non-motif
  bus, foundation included: "filtering the whole world is the woozy intent;
  silencing its pulse is not."

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- E2 (`fa22e44`) applies the audio half as idempotent mixer tweens, with a
  deterministic warble LFO on degradable buses only; E3 (`e18d03a`) carries
  the visual half and arms the ladder only once coherence first clears the
  top rung, so a session never boots mid-disintegration.
- The full-fail threshold and reintegration sit on the ladder's config
  ([ADR 0014](0014-reintegration-seam-on-one-clock-tick.md)).
- Thresholds are measured, not derived. The first deep band was unreachable;
  `d8febe5` set rungs at 0.82/0.72/0.64 and full fail at 0.62 from the
  measured signal, and added a hold with its own hysteresis, because a
  sawtooth peak poking above `enter_below` would otherwise cancel it.
  `e66cd10` moved full fail to enter 0.66, exit 0.80.
- Since then: `2fc8606` decoupled arming from the top rung as `arm_above`
  (default 0.80), calibrated from a real session. The visual half reaches the
  engine's post stack through
  [ADR 0021](0021-ladder-visuals-through-the-post-stack.md).

## Measured

From `2fc8606`, replaying a real session: engaged play reaches 0.80 at about
31 s and lives at 0.77-1.0 (median 0.88); dead input peaks at 0.67 and never
arms. From the Phase 3 note: sustained neglect settles at 0.65-0.76 by meter.

## Sources

- Commit [`aad0772`](https://github.com/chrischaps/Flint/commit/aad0772098dcf4ccb67ac5429aa3a13cad1de1b0)
  "E1 ladder framework: rungs, hysteresis, LadderParams, config/ladder.toml
  (ADR 0015)"
- Commits [`fa22e44`](https://github.com/chrischaps/Flint/commit/fa22e444cee18f6f31b2a1301a225dc81b64a2b6),
  [`e18d03a`](https://github.com/chrischaps/Flint/commit/e18d03ac81c1140d3a59349f313578be02f986a6),
  [`d8febe5`](https://github.com/chrischaps/Flint/commit/d8febe570f4d1189fbb5f035073654e75968d116),
  [`e66cd10`](https://github.com/chrischaps/Flint/commit/e66cd10c6bebc4da14625669702a852358fe19b2),
  [`2fc8606`](https://github.com/chrischaps/Flint/commit/2fc8606a013ef2b9c2ddb33ea41696ca075155bc)
- Vault: "Starchild Phase 3 - Where the Signal Actually Lives (2026-08-15)"
- Code now: `crates/flint-music/src/ladder.rs` (`Ladder`, `LadderConfig`,
  `LadderParams`, `LadderDriver`, `FullFail`),
  [Music Sessions](../book/src/concepts/music-sessions.md), "The Ladder"
