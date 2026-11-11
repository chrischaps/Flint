# flint-particles: particle effects (moved out of the root CLAUDE.md by #755)

The particle draw gotcha: `crates/flint-render/CLAUDE.md`. The effect editor and `--particle-time`: `crates/flint-cli/CLAUDE.md`.

- **Particles** -- inline `particle_emitter` or reusable `particles/*.particles.toml` effects via `particle_effect` (multi-emitter, curves, forces, bursts, sub-emitters; ADR 0068). Deterministic: per-emitter seeds from entity *names*, fixed-step `simulate_to`, `flint render --particle-time`. One shared `ParticleInstance` type; every consumer uploads through `SceneRenderer::update_particles_from` after `ParticleSystem::pack`. Scripts: `play_effect`/`stop_effect`/`set_effect_param`. F3 Particles panel in the player; F4 Particles section in the viewer
