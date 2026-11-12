# Architecture Decision Records

Flint's commits, the book, the CLAUDE.md files and code comments cite
decisions by number ("ADR 0068", "ADR 0110"). Every number cited in the tree
has a file here, one per decision, and `scripts/adr-check.sh` keeps it that
way: `scripts/ci.sh` runs it, so CI and `issue.sh land` fail on a citation
with no record.

## Where the records came from

- **0111 on** were written here, as files, when the decision was made.
- **0068-0110** lived only in commit messages and PR descriptions. Most of
  those PR bodies hold the record's full text, so these files are close to
  the original.
- **0001-0067** (and 0071-0075) were files in the Starchild game repo's
  `docs/adr/`, which no longer exists. Engine work for Starchild landed in
  Flint through the subtree, so what survives is Flint's commit messages,
  code comments, the book and the session notes in Claude's Vault.

All records before 0111 were backfilled on 2026-11-12 (#374). Each says so
under its title and names its sources. Where the reasons didn't survive, the
record says that rather than supplying new ones. Where the code has moved on
since, a "Since then" line under Consequences says how. A few numbers are
placed by inference (a batch commit that lists its decisions in order with
only some numbered); the record says so.

Numbers with no record: 0028, 0029, 0038, 0041, 0067 and 0077. Nothing in
the tree cites them and nothing about them survived. The Vault places 0028
and 0029 in the World II plan (whose engine commit numbers it 0030-0037) and
0038 and 0041 in the Starchild hygiene pass with 0039 and 0040. 0067 was
taken in Starchild. 0077 was never used.

Three numbers were raced by parallel PRs and renumbered at merge. A commit
message citing the old number names a different decision: `974c998` says
0085 for the sound falloff that merged as [0086](0086-placed-sound-falloff-curve.md),
`89a10ba` says 0086 for the `Projector` that merged as
[0087](0087-native-code-projects-with-frame-camera.md), and `2754576` says
0088 for the grass that merged as [0089](0089-compute-grass-over-any-ground.md).
The files follow the merged numbers.

## Writing one

- One file per decision: `NNNN-short-slug.md`, numbered after the highest
  ADR cited anywhere in the repo or its git log. Check
  `git log --all --format=%B | grep -o "ADR 0[0-9]*" | sort -u | tail -1`
  and `ls docs/adr | tail -2` before taking a number, and take it in the PR
  that makes the decision.
- Sections: Context, Decision, Alternatives considered, Consequences, and
  Measured when there are numbers.
- Cite a record as `ADR NNNN` in code comments and commits, and as a link in
  the book and in other records. Link the book chapter that teaches the
  feature, and have the chapter link back.
- A range ("ADRs 0071-0075") cites every number in it, so `adr-check.sh`
  wants a file for each.

## Index

| ADR | Decision |
|---|---|
| [0001](0001-kira-is-the-music-backend.md) | Kira is the music backend: one `ClockHandle`, `StartTime::ClockTime`, per-bus gain and low-pass; no per-track pitch (Starchild spike, Vault) |
| [0002](0002-music-runtime-in-flint-music.md) | The music runtime lives in a new `flint-music` crate, with a shared-AudioManager accessor deferred (Vault; crate in 5f94ff8) |
| [0003](0003-shape-tolerant-parsers-and-coded-validator.md) | Suite manifests and charts parse shape-tolerantly; a validator reports stable issue codes pinned by golden fixtures; `flint validate-suite` (5f94ff8) |
| [0004](0004-lossless-stems-only.md) | Stems are lossless (FLAC/WAV); the validator errors `StemPrimingUnsafe` on encoder-primed audio (2c4d9c3, 4db77a6) |
| [0005](0005-clock-counts-samples-tempo-map-speaks-music.md) | The Kira clock counts samples and never changes speed; only `TempoMap`/`Conductor` speak bars and beats (15b113c) |
| [0006](0006-deterministic-offline-render.md) | Sessions render offline through the live `SuiteSession` on a custom Kira backend, bit-identical; `flint render-suite` (9814fca, amended e66cd10) |
| [0007](0007-suite-playback-keeps-its-own-manager.md) | Live suite playback keeps its own AudioManager; the shared accessor waits (partly recovered; Vault, 15b113c) |
| [0008](0008-gamepad-capture-on-its-own-thread.md) | Gamepad capture runs on its own 1 kHz thread in `flint-input-capture`, stamping verb-space events with compensated suite samples (762b6b0) |
| [0009](0009-session-replay-in-compensated-samples.md) | Sessions record as JSONL compensated-sample events and replay headless through the same judgment; `flint replay-chart` (f693ee9, amended 226ac74) |
| [0010](0010-coherence-as-asymmetric-leaky-integrator.md) | Coherence is one 0..1 asymmetric leaky integrator with bar-denominated time constants and bounded pulse impulses (b10dc43) |
| [0011](0011-gilrs-on-the-xinput-backend.md) | gilrs runs on the XInput backend, since WGI ignores unfocused console apps, and a silent input path says so loudly (d1314a7) |
| [0012](0012-windowed-feel-harness-with-wordless-cues.md) | `play-chart --window`: a bare wgpu window with wordless glow cues that also absorbs mapper keystrokes (e652fdc) |
| [0013](0013-arrival-lean-mode.md) | Lean is judged by arrival at beat-anchored targets (±0.3 beats) by default; `track` stays the config default (8a5fb9a) |
| [0014](0014-reintegration-seam-on-one-clock-tick.md) | A reintegration seam fades out and re-plays every stem from the re-entry section on one clock tick, with `suite = raw - offset` (89e6e10) |
| [0015](0015-disintegration-ladder-as-data.md) | The disintegration ladder is `config/ladder.toml`: absolute rungs, hysteresis thresholds, one `LadderParams` for audio and visuals, protected buses (aad0772) |
| [0016](0016-reactive-loop-in-flint-music.md) | The reactive loop (`ChartSession`/`ChartCore`) moves from the CLI into `flint-music` so the player can host it (963b2b0) |
| [0017](0017-one-shared-audio-manager.md) | Music and world sound share one Kira AudioManager (`manager_mut`), dropped producer → handles → device; skew double-read (187e756) |
| [0018](0018-gamepad-handoff-and-inputstate-downsample.md) | A session takes the gamepad; capture events are down-sampled into `InputState` (lean and pulse only, pulse released next tick) (187e756) |
| [0019](0019-music-session-component-in-the-player.md) | A scene declares its suite with a game-side `music_session` component; the player runs the session lifecycle; F2 spike deleted (d95dc8b) |
| [0020](0020-conducted-parameters-script-surface.md) | Scripts read a per-frame `ConductedSnapshot` through `conducted_*` getters with neutral defaults, from one deterministic producer (72e2b7e) |
| [0021](0021-ladder-visuals-through-the-post-stack.md) | Ladder visuals drive the engine post stack (ash-grey desaturation, radial blur ×0.6) as authored base plus rung, written every frame (bd4eb73) |
| [0022](0022-f6-conducted-scene-and-camera-roll.md) | The F6 prototype scene is conducted through its environment; scripts get `set_camera_roll`, level again on any frame that doesn't ask (27c5ff4) |
| [0023](0023-conducted-next-target-lookahead.md) | `conducted_next_target()`: the next authored lean key and the beats until it, live in preroll, a 1e6 sentinel past the last key (27c5ff4) |
| [0024](0024-error-driven-audio-gradient.md) | Error-driven audio gradient: lean error sets the depth of a zero-mean pitch wobble (not a detune), summed into the one mixer writer (27c5ff4) |
| [0025](0025-rumble-through-direct-xinput.md) | Rhythm rumble writes the motors directly through XInput from the 1 kHz capture thread, bypassing gilrs's 50 ms ff server (27c5ff4) |
| [0026](0026-haptic-entrainment-architecture.md) | Haptics are a pure, event-shaped decision layer (never error-driven) feeding a capture-thread rumble engine that drops late bursts (27c5ff4) |
| [0027](0027-script-owned-depth-of-field.md) | Depth of field: a single-pass CoC-weighted disc blur in the composite shader, set by `set_dof`/`set_dof_focus`, owned by scripts (27c5ff4) |
| [0030](0030-full-verb-map-capture.md) | The capture thread speaks a selectable verb map; `full` adds sway, pressure, press and flick, and `prototype` stays byte-identical (86d21ec) |
| [0031](0031-per-channel-judgment-and-coherence-weights.md) | Per-channel judgment of the new verbs, with their coherence weights inert by default (inferred from batch order) (86d21ec) |
| [0032](0032-degraded-alternate-playback.md) | A composed degraded alternate plays as a muted, sample-locked twin on its bus's track, crossfaded in by ladder rungs (86d21ec) |
| [0033](0033-chart-cues-fire-with-params.md) | Chart cues fire into the conducted frame with their flat `params` table; a rewind drops them, a seam re-arms them (86d21ec) |
| [0034](0034-world-ii-conducts-the-path-not-the-camera.md) | World II conducts the path, never the camera; the conducted frame gains sway and pressure (86d21ec) |
| [0035](0035-music-guide-debug-panel.md) | A summonable Music Guide panel (Backquote, `debug-hud` only) shows the input the chart expects, from a purpose-shaped `GuideFrame` (86d21ec) |
| [0036](0036-quit-on-finish.md) | `music_session.quit_on_finish` exits the player when a session ends on its own (86d21ec) |
| [0037](0037-manifest-map-timeline-panel.md) | A Manifest Map strip (Backslash) of the whole suite and this run's pulses and seams, from write-only histories (86d21ec) |
| [0039](0039-recover-from-mutex-poisoning.md) | Script-context and clock-bridge locks recover from mutex poisoning (`lock_or_recover`) instead of panicking on every later call (993118d) |
| [0040](0040-loud-config-errors-and-version-policy.md) | A missing config `schema_version` warns instead of passing silently; bad calibration files are named and skipped; saturating casts became checked (48e48b6) |
| [0042](0042-clay-look-research-spike.md) | A clay-look research spike: scenes, a generator, a technique menu, a Tier 0 photography pass (decision not recovered) (e3a4ff0) |
| [0043](0043-scene-authored-hemisphere-ambient.md) | A scene can author the hemisphere ambient (`[environment] ambient_sky/ambient_ground`), reset on every scene transition (inferred from batch order) (e3a4ff0) |
| [0044](0044-diffuse-wrap.md) | `[environment] diffuse_wrap` softens the diffuse terminator, carried in `ambient_sky.w` as `1 + wrap` so 0 is the old shading (e3a4ff0) |
| [0045](0045-deterministic-shadow-caster-order.md) | Lights sort by name, then the strongest directional takes the shadow slot, so the caster no longer changes run to run (e3a4ff0) |
| [0046](0046-viewer-applies-scene-post-process.md) | The scene viewer applies the scene's `[post_process]` on load, through one `post_process_config_from_def` shared with the player and CLI (e3a4ff0) |
| [0047](0047-viewer-dof-follow.md) | The viewer's DoF follow (F12) focuses on the selected entity, in the shader's halved depth units until the convention was fixed (e3a4ff0) |
| [0048](0048-clay-lighting-levers-oren-nayar-and-sheen.md) | Clay levers in `[environment]`: `oren_nayar` diffuse blend and a tinted Charlie sheen (`sheen_color`/`sheen_strength`); absent = exact legacy shading (95a260a) |
| [0049](0049-upload-shadow-texel-size-per-resolution.md) | The shadow texel size is uploaded in `cascade_splits.w` (0 = legacy 1/2048), so `--shadow-resolution` (default 2048) is real (95a260a) |
| [0050](0050-color-grade-film-grain-and-fxaa.md) | Post levers: ASC-CDL grade after ACES, 24 Hz film grain on the shared post time, FXAA as a lazy pass, off by default; no Rhai setters (95a260a) |
| [0051](0051-scene-viewer-opens-on-authored-camera.md) | The scene viewer seeds its orbit camera from the scene's `[camera]` (Space returns to it); `--auto-orbit` turntable for scenes (aac2d27) |
| [0052](0052-authorable-ssao-sample-count.md) | `[post_process] ssao_samples` (1-64, default 64) strides the kernel; the clay scenes author 16, about 4x cheaper (7d0144b) |
| [0053](0053-one-rendering-and-effects-menu-on-f4.md) | One F4 Rendering & Effects panel replaces the per-effect F-keys: live mirror, write-through, freeze switch for script overrides (aac2d27) |
| [0054](0054-tier-2-renderer-pass-scope.md) | Scope doc for the Tier 2 renderer pass; text not recovered (aac2d27) |
| [0055](0055-wgpu-zero-to-one-depth-convention.md) | Perspective projection and every depth reader use wgpu's `[0, 1]` depth, so DoF distances are metres (aac2d27) |
| [0056](0056-source-radius-area-lights.md) | Area lights: `source_radius` (point/spot) widens specular, representative-point style; `angular_size` (directional) feeds PCSS; 0 = verbatim legacy (aac2d27) |
| [0057](0057-pcss-contact-hardening-shadows.md) | PCSS on the directional cascades when the sun has an `angular_size`; `pcss.w = 0` keeps 3x3 PCF verbatim (aac2d27) |
| [0058](0058-opt-in-4x-msaa-for-scene-passes.md) | Opt-in 4x MSAA for scene passes with a sample-0 depth resolve; default 1 keeps pixel gates single-sample (aac2d27) |
| [0059](0059-clay-prologue-neutral-tint-set.md) | Starchild: one neutral clay prop set, each prop's hue set in the scene through `base_color_*` scalars (aac2d27) |
| [0060](0060-skinned-stick-figures-in-the-prologue.md) | Starchild: skinned stick figures in the prologue; decision not recovered (aac2d27) |
| [0061](0061-flight-trails-from-script-driven-emitters.md) | Flight trails: `particle_emitter` gains `shape_offset`, an oriented box and per-frame script-driven shape fields, in world axes (aac2d27) |
| [0062](0062-player-app-split-into-lifecycle-files.md) | `player_app` split into named lifecycle files (pure motion), then named steps, with no behaviour change (7742022..e5184cd) |
| [0063](0063-cloud-sea-as-sculpted-meshes.md) | Starchild: the prologue's cloud sea is generated opaque meshes lit by the clay levers, not a volumetric renderer (Vault only) |
| [0064](0064-debug-keyboard-fallback-for-music-sessions.md) | Debug-hud keyboard fallback for music sessions with no gamepad (arrows = lean, Space = pulse); F9 force-fail follow-up (81e54fc) |
| [0065](0065-intro-treadmill-with-credits-in-fog.md) | Starchild: the opening is a treadmill flight through a tiled cloud sea, with credits as world quads in the fog (Vault only) |
| [0066](0066-scene-audio-preload-opt-out.md) | `[scene] preload_audio = false` skips the blanket `audio/` preload; default true (81e54fc) |
| [0068](0068-particle-effect-assets-and-deterministic-sim.md) | Particles rebuilt: reusable `*.particles.toml` effects, deterministic name-seeded CPU sim, one `ParticleInstance` layout, editor (98886da) |
| [0069](0069-physics-joints-and-world-space-bodies.md) | Rigidbodies placed from world matrices; `joint` component (hinge/prismatic/spherical/fixed) on Rapier impulse joints; rest-space rotation API (44e51af) |
| [0070](0070-two-bone-ik-on-transform-chains.md) | `ik_two_bone` on transform chains, solved by `IkPass` each frame after scripts, physics and animation; engine IK over Rhai (f3fc3a5) |
| [0071](0071-player-extensions-in-a-game-binary.md) | A game binary installs `PlayerExtension`s into `PlayerApp` (`add_extension`): script API, `pre_update` before `on_update`, window events, `ui` (#41) |
| [0072](0072-custom-events-reach-scripts.md) | `GameEvent::Custom` reaches scripts as `on_event(name, data)`, in entity-id order, data always a map; `ExtensionCtx::fire_event` same frame (#41) |
| [0073](0073-game-egui-layer-and-input-routing.md) | A game egui layer between script UI and debug panels (now Foreground areas); egui sees input first, a release follows its press (#41) |
| [0074](0074-runtime-meshes-and-frustum-culling.md) | Runtime meshes upload, replace and remove without leaks (`model.runtime = true`), and every static model draw is frustum-culled (#41) |
| [0075](0075-screen-to-ground-picking.md) | `screen_to_ground` in f64 (`flint_core::projection`, Rhai `[x, z]`), a straight-down camera basis, scripts' camera refreshed before `on_draw_ui` (#41) |
| [0076](0076-emissive-materials.md) | Emission is a material property added after lighting, before tonemapping; per-vertex form with alpha as surface/light mask; glTF mapping (#43, #57) |
| [0078](0078-fog-reaches-everything-drawn.md) | A pixel is sky only at depth exactly 1.0, so fog, SSAO, DoF and volumetrics reach everything drawn; the ocean's self-fog is gone (#45) |
| [0079](0079-mouse-wheel-input.md) | The player feeds the mouse wheel to `InputState` in lines (50 px a line), after egui and extensions; `mouse_wheel_x/y`, `wheel` verb (#47) |
| [0080](0080-extensions-reach-the-persistent-store.md) | `ExtensionCtx::persist`: extensions share the scripts' `PersistentStore` and its save policy, from `pre_update` only (#48) |
| [0081](0081-project-fonts-name-egui-defaults.md) | `fonts.toml [egui]` names egui's default faces, families fall back to egui's, and `PlayerExtension::fonts` edits definitions before each install (#49) |
| [0082](0082-input-script-mouse.md) | Input scripts play the mouse (`mouse`, `click`, button names) through egui's path, with a press and a release each waiting a frame (#50) |
| [0083](0083-depth-tested-world-lines.md) | `draw_world_line`: CPU-built anti-aliased quads drawn after composite and FXAA, depth-tested against resolved depth, with 0.2% depth slack (#51) |
| [0084](0084-extensions-reach-the-audio-engine.md) | `ExtensionCtx::audio`, a narrow `ExtensionAudio`: play with `SoundOptions`, load samples or bytes into the shared cache, set bus volumes (#52) |
| [0085](0085-script-drawing-over-egui.md) | `push_over_ui`/`pop_over_ui` paint script HUD items on one non-area egui layer on `Order::Middle`, above game panels and below popups (#53) |
| [0086](0086-placed-sound-falloff-curve.md) | A placed sound's falloff curve is Kira's `OutPowf` exponent, `rolloff` (default 2); scripts get pitch and a `play_sound_at` options map (#54) |
| [0087](0087-native-code-projects-with-frame-camera.md) | `flint_script::Projector`, a cloneable handle so native script functions project with the frame's camera, the same answer as Rhai (#55, #56) |
| [0088](0088-shoreline-foam-from-scene-depth.md) | Shoreline foam from the grab pass's scene depth, measured as depth below the surface; opt-in `shore_foam_width`, no mask (#58) |
| [0089](0089-compute-grass-over-any-ground.md) | Compute grass on a game's own ground (mask, height, world rectangle, `grass_ground`), dispatched only within reach of the camera (#59, #60) |
| [0090](0090-emission-scale-after-upload.md) | `material.emissive_scale` multiplies a mesh's emission every frame with no re-upload; per-vertex emission gets its own uniform flag (#62) |
| [0091](0091-shore-foam-measured-across-the-water.md) | Shoreline foam measured across the water by a screen-space shore search, `shore_foam_reach` (#63) |
| [0092](0092-grass-culled-to-view-drawn-indirect.md) | Grass culled to the camera frustum and drawn indirect; the count read-back never waits (#64) |
| [0093](0093-repaint-grass-mask-in-place.md) | `update_grass_mask` repaints part of a grass ground's mask in place, without reinstalling grass (#65) |
| [0094](0094-water-on-any-mesh.md) | Water on any mesh: lakes and rivers drawn with the ocean's shader, `water_body` component (#68) |
| [0095](0095-ray-against-placed-height-field.md) | A game picks its ground with a ray against a placed height field, `GroundField` and `screen_to_heightfield` (#67) |
| [0096](0096-glow-surface-after-upload.md) | `material.glow_surface` sets a light face's lit-surface share per entity, after upload (#69) |
| [0097](0097-input-script-keys-reach-egui.md) | Input scripts' keys reach egui as a real keyboard's would; `type` verb (#70) |
| [0098](0098-grass-stands-on-blended-heights.md) | Grass stands on a bilinear blend of its ground's heights: corners for terrain, cell centres for maps (#71) |
| [0099](0099-water-body-scales-shore-band.md) | A water body scales the ocean's shoreline band, `shore_foam` (#72) |
| [0100](0100-entity-can-opt-out-of-shadows.md) | `material.cast_shadows = false` keeps an entity out of the shadow map, inherited from the parent (#73) |
| [0101](0101-height-fog-layer-and-aerial-tint.md) | Height fog becomes an exponential layer integrated along the ray; an aerial tint under the fog (#74) |
| [0102](0102-per-entity-vertex-tint.md) | `material.vertex_tint_r/g/b` tints a vertex-coloured mesh per entity, every frame (#75) |
| [0103](0103-extensions-see-scripted-keys.md) | `PlayerExtension::on_key` lets extensions see real and scripted keys (#77) |
| [0104](0104-depth-tested-world-text.md) | `draw_world_text`: depth-tested world text, laid out by the player with egui's fonts (#78) |
| [0105](0105-rays-against-drawn-meshes.md) | Runtime meshes can be ray targets (BVH); `raycast_meshes`, `screen_to_mesh` (#80) |
| [0106](0106-ground-fog-on-own-floors.md) | Ground fog on floors of its own, from a game-supplied `[floor, amount]` map, `set_ground_fog` (#82) |
| [0107](0107-debug-panels-from-the-game.md) | A game names and draws its own F3 debug panels through its player extension (#83) |
| [0108](0108-gpu-timing-from-timestamp-queries.md) | `GpuTimer`: GPU time per frame phase from timestamp queries, read back without waiting (#83) |
| [0109](0109-tracy-frame-profiler.md) | flint-player's `profile` feature: Tracy frame profiling through `profiling`, nothing when off (#83) |
| [0110](0110-capture-and-focus-gates.md) | Capture and focus gates for mouse-look and pads; amended by #88 and #91 (#84) |
| [0111](0111-stylized-rendering.md) | Stylized rendering: per-entity selection on one screen grid, two stages, the descriptor-driven style framework and the id-only style mask (#260, #261) |
| [0112](0112-user-authored-styles.md) | User-authored styles: `styles/<name>.shader.toml` + one WGSL `fn style`, a generated prelude, naga check with file:line errors, passthrough fallback, append-only registry (#274, #275) |
| [0113](0113-one-glyph-atlas-for-text.md) | One glyph atlas for text: `ui_text` sprite fonts are layout over a `GlyphAtlas` (`builtin`, a `.glyphs.toml` set, or a `.font.toml` PNG grid kept for compatibility), uploaded once as white RGBA with coverage alpha (#283) |
| [0114](0114-project-manifest-and-one-resolver.md) | A project manifest (`flint.toml`) and one resolver: `Project` in flint-core over the vfs, one schema resolver, one asset search rule with a `tried` list (#706) |
| [0115](0115-scene-diagnostics-and-complete-schemas.md) | One list of scene diagnostics from one pass, a schema for every engine component, entity and asset field types, and typed component reads that report (#683) |
| [0116](0116-declared-script-api.md) | A declared script API: one registration builder that records name, argument kinds and doc, coerces numbers, resolves entities, and feeds `flint script api` and the checker (#695) |
| [0117](0117-player-subsystems-and-one-game-step.md) | Player subsystems on named stages, one `enter_scene`, and one windowless `GameCore::step` shared with headless runs (#724) |
| [0118](0118-renderer-owners-for-pipelines-draws-and-frame-inputs.md) | The renderer gets one owner each for sample-count pipelines, draw kinds (`DrawItem`) and per-frame inputs (`FrameInputs`, `bind_scene`) (#736) |
| [0119](0119-game-generators-through-player-extensions.md) | A game registers its own procgen generators through `PlayerExtension::register_generators`, into the player's resolver, which keeps them across the disk-tier rebuild (#195) |
