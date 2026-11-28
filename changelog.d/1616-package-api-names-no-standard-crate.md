### The Package API names no standard package: physics, the sound and the particles are run resources (#1616)
**Package.** engine
**What changed.** `flint-package` no longer depends on flint-physics, flint-audio or flint-particles (ADR 0138 decision 1), so a package's crate stops linking them through the API. Four things moved:
- `FixedCtx::physics` is gone. Physics' state is a run resource, in every context's `resources`: `flint_physics::of_mut(ctx.resources)` in a fixed step (and `flint_physics::of` at any stage). `PhysicsSystem::fixed_update_in(resources, world, dt, events, between)` is the step over it.
- `flint_package::physics` is gone: Rapier is `flint_physics::rapier`.
- `SceneCtx::audio` and `StageCtx::audio` are gone. The player lends the sound to each hook through the resources: `flint_audio::engine::of_mut(ctx.resources)` (`None` in a capture or a headless run).
- `PanelCtx::particles` is gone. A panel reads `ctx.resources.get_mut::<flint_particles::ParticleSystem>()` and its `sync`.

`Resources` is `flint_core::resources::Resources` now, still re-exported as `flint_package::Resources`, and gains `Resources::lend`. The player's `GameCore::physics` field is the `physics()` / `physics_mut()` methods.
**What to do.** In a game's own package that uses physics: add `flint-physics` to its crate's dependencies, replace `ctx.physics` with `flint_physics::of_mut(ctx.resources)` (return if it's `None`), and replace `flint_package::physics::` with `flint_physics::`. A package that played on `ctx.audio` adds `flint-audio` and reads `flint_audio::engine::of_mut(ctx.resources)`. A game binary that read `app.core.physics` calls `app.core.physics()` or `physics_mut()`. Scenes and scripts need nothing.
