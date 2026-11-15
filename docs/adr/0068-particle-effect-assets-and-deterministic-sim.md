# ADR 0068: Particles: reusable effect assets, a deterministic CPU sim, one instance layout

Status: accepted, 2026-09-04 (commit
[`98886da`](https://github.com/chrischaps/Flint/commit/98886daa5ab190590c498193c6178f1865beb08e)).
Backfilled 2026-11-12 (#374) from commits `98886da` and `4b0d36e`, the Vault
note "2026-09-04 Flint Particles V2 Effect Assets And Editor", and the code
and book lines that cite it. This record was never a file; it lived in the
commit message and code comments.

## Context

`flint-particles` was first written in February 2026. The Vault note lists
what was wrong with it: it re-parsed every emitter's TOML each frame,
allocated a fresh GPU buffer per emitter per frame, drew in `HashMap`
order, and `flint render` could not see particles at all. A multi-emitter
effect, or one a script could start by name, had no unit to name.

## Decision

- **Assets.** Reusable `*.particles.toml` effects, placed by a
  `particle_effect` component: `ParticleEffect { name, seed, budget,
  emitters }`. The inline `particle_emitter` stays, and one `EmitterDef`
  parser reads both. Assets reject unknown keys; inline stays lenient.
- **Emitters** gain multi-key curves, per-particle ranges, wind, drag,
  noise, vortex and attractor forces, burst timelines, emission over
  distance, velocity inheritance, and birth and death sub-emitters (one hop
  a frame), under a global budget.
- **Deterministic simulation on the CPU.** Each emitter's RNG is seeded from
  the effect seed, the entity's *name* and the emitter index; instances are
  ordered (`BTreeMap`); `simulate_to(t, 1/60)` steps at a fixed rate.
  `sim.rs` (`step_emitter`) and the single `ParticleInstance` layout are the
  seam a GPU path would replace.
- **Renderer.** One shared 64-byte `ParticleInstance` POD, one persistent
  instance buffer grown on demand and written once a frame, four blend
  modes drawn far to near with additive last, a headless pipeline, and a
  soft-disc sprite when no texture is given. Every host uploads through
  `SceneRenderer::update_particles_from`.
- **Tools.** `flint render --particle-time`; an effect editor,
  `flint edit fx.particles.toml`; live particles in the scene viewer with
  an F4 section; an F3 Particles panel in the player; curve and gradient
  widgets shared from `flint_debug_ui::widgets`. Rhai `play_effect`,
  `stop_effect`, `set_effect_param`.

## Alternatives considered

Chris's calls, from the Vault note:

- **Extending the inline component alone**, rejected for reusable assets:
  multi-emitter effects and `play_effect` need a nameable unit.
- **GPU compute simulation**, deferred: richer features and determinism
  first, with the GPU seam kept.
- **One tool only** (editor, viewer or panel), rejected for all three.

## Consequences

- Seeds first came from entity ids, which differ run to run, so two renders
  differed (Measured, below). Deriving them from names fixed it, and that
  became a repo-wide rule (root CLAUDE.md).
- Inline emitters default `local_axes` to `false`, keeping
  [ADR 0061](0061-flight-trails-from-script-driven-emitters.md)'s trails;
  assets default it to `true`.
- Deferred, with some fields already parsed: GPU compute, ribbons and trails
  (the `trail` key is reserved), mesh particles, soft particles and
  distance fade, collision.
- Since then: effects and particle textures are found by the project's
  asset search ([ADR 0114](0114-project-manifest-and-one-resolver.md)).

## Measured

From the Vault note: with id-derived seeds, two headless renders of one
scene had 74 and 67 particles alive. With name-derived seeds, two
`flint render --particle-time 2` runs are byte-identical.

## Sources

- Commit
  [`98886da`](https://github.com/chrischaps/Flint/commit/98886daa5ab190590c498193c6178f1865beb08e)
  "particles: effect assets, deterministic sim, editor, viewer/player
  integration (ADR 0068)"
- Commit
  [`4b0d36e`](https://github.com/chrischaps/Flint/commit/4b0d36e0fbf6956bc8dc973fa24da9748a3f7668)
  "docs: particles v2 — effect assets, editor guide, CLI/format/panel
  references"
- Vault: "2026-09-04 Flint Particles V2 Effect Assets And Editor"
- Code now: `crates/flint-particles/src/` (`effect.rs`, `sim.rs`,
  `rand.rs`, `sync.rs`, `particle.rs`), `crates/flint-render/src/particle_pipeline.rs`,
  `crates/flint-render/src/scene_renderer/particles.rs`,
  `crates/flint-cli/src/commands/particle_edit/`,
  `crates/flint-script/src/api/particle.rs`,
  `schemas/components/particle_effect.toml`
- Book: [Particles](../book/src/concepts/particles.md);
  [Particle Editor](../book/src/guides/particle-editor.md)
