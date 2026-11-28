### The renderer links no terrain or particle crate; seven Rust paths moved (#1615)
**Package.** engine
**What changed.** `flint-render` no longer depends on `flint-terrain` or
`flint-particles`: it draws the plain data they hand it, so neither package
has to be in a build that renders. Scenes, scripts, captures and pictures are
unchanged (every golden is byte-identical, and a capture written before this
replays). Moved, for Rust that names them (the games in Flint's repos have
none):
`ParticleInstance` and `ParticleBlendMode` are defined in
`flint_render::particles` (still re-exported from `flint_particles`);
`flint_particles::ParticleDrawData` is `flint_render::ParticleDraw` (no `key`
field), and `flint_render::ParticleDrawData` is gone into it too, so
`SceneRenderer::update_particles` takes `&[ParticleDraw]`;
`FrameInputs::particles` is `Option<&[ParticleDraw]>`, from
`ParticleSync::draw_data()`, instead of `Option<&ParticleSync>`;
`flint_render::load_scene_particle_textures` takes `&sync.texture_names()`;
`ParticleInstance::from_particle(p, ..)` is `p.instance(..)`;
`TerrainChunk` and `GrassConfig` are defined in flint-geometry
(`terrain_chunk`, `grass`; re-exported from `flint_terrain` and
`flint_render`). `SceneRenderer::bind_scene` no longer builds the terrain:
`flint_terrain::scene::bind_terrain(renderer, device, queue, world, &mut bound)`
(flint-terrain's `render` feature) does, right after it, and returns the
`LoadedTerrain` that was `BoundScene::terrain`.
**What to do.** Nothing, unless a game's own Rust names one of those paths:
change the `use` line, call `bind_terrain` after `bind_scene` if it binds a
scene itself, and pass `Some(&sync.draw_data())` as a frame's particles.
