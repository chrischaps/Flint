//! The replay host (#1089, ADR 0126 decisions 4 and 6): a captured frame
//! drawn again, in a headless renderer, through the same load every other
//! `flint render` stage takes.
//!
//! [`ReplayHost::load`] loads the **authored scene** (`Stage::load`: the
//! binding, models, splines, living props' procgen, terrain and grass), lays
//! the captured world over it **by entity name** (each captured entity's
//! archetype, components and parent set; one the load lacks spawned; one the
//! capture lacks despawned), runs **no game step**, then writes back what the
//! renderer held (`SceneRenderer::restore_replay_state`: bone, shadow and
//! echo palettes, part tints, motion holds, the selection), the camera, the
//! drawn config and quality. Each asset the capture names is checked
//! against its recorded hash: a missing or changed one is named
//! ([`AssetCheck`]) and the load draws it as it draws any missing asset.
//!
//! [`ReplayHost::draw`] then draws the frame under [`Controls`] (entities
//! hidden, the scene pass stopped after N draws, passes switched off) and
//! writes it as a capture of its own, so every `flint frame` view reads the
//! replay as it reads the original.
//!
//! A child of `render` so it takes the `Stage` as the stage path does.

use super::*;
use flint_render::frame_capture::{AssetDescription, FrameDescription};
use flint_render::frame_replay::{OwnedFrameInputs, ReplayInputs, ReplayParticles};
use std::path::PathBuf;

/// What a replay leaves out of the frame, by name (#1089).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Controls {
    /// Entities drawn in no pass.
    pub hidden: Vec<String>,
    /// The pass `draws` steps, by its name in the frame's description
    /// (`Scene`, `Shadow cascade 0`, `Style mask`...; #1204). `None` is the
    /// scene pass.
    pub pass: Option<String>,
    /// That pass's first N draws only.
    pub draws: Option<usize>,
    /// Passes switched off, by name (`Bloom`, `SSAO`, a style's).
    pub skip: Vec<String>,
    /// The entity whose pixels the selection outline marks, in place of
    /// the captured selection.
    pub outline: Option<String>,
}

impl Controls {
    /// The pass `draws` steps: the scene's unless `pass` names another,
    /// or why the name can't be stepped.
    pub fn stepped(&self) -> Result<flint_render::SteppedPass, String> {
        let Some(name) = &self.pass else {
            return Ok(flint_render::SteppedPass::Scene);
        };
        flint_render::SteppedPass::from_name(name).ok_or_else(|| {
            let names: Vec<&str> = flint_render::SteppedPass::all().map(|p| p.name()).collect();
            format!(
                "no pass named {name:?} steps a draw at a time (these do: {})",
                names.join(", ")
            )
        })
    }

    /// The stepped pass's name as the description has it (`Scene`).
    pub fn stepped_name(&self) -> &str {
        self.stepped()
            .map(|p| p.name())
            .unwrap_or_else(|_| self.pass.as_deref().unwrap_or("Scene"))
    }
}

/// One asset the capture names, as the replay found it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AssetCheck {
    pub kind: String,
    pub name: String,
    /// Where the capture found it.
    pub path: String,
    /// `same`, `moved` (found elsewhere with the same bytes), `changed`
    /// (its bytes differ) or `missing`.
    pub state: &'static str,
    /// Where it is now, when it isn't where it was.
    pub now: Option<String>,
}

impl AssetCheck {
    /// Whether the replay draws it as the capture did.
    pub fn same_bytes(&self) -> bool {
        matches!(self.state, "same" | "moved")
    }
}

/// A pass a replay can switch off, and what switching it off does.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Skippable {
    pub pass: String,
    pub does: String,
}

/// The frame drawn again (its capture is the folder `draw` was given).
pub struct Drawn {
    /// Controls that named nothing (an entity not in the world, a pass that
    /// can't be switched off), a line each.
    pub refused: Vec<String>,
}

/// A captured frame, loaded and ready to draw again. See the module docs.
pub struct ReplayHost {
    ctx: HeadlessContext,
    stage: Stage,
    inputs: OwnedFrameInputs,
    particles: ReplayParticles,
    /// The drawn config and quality as captured.
    config: flint_render::PostProcessConfig,
    quality: flint_render::QualitySettings,
    camera: Camera,
    /// The shadow map's size as captured.
    shadow_resolution: Option<u32>,
    /// The entity the captured frame had selected.
    selected: Option<String>,
    scene: String,
    game_time: Option<f64>,
    /// What the load found, a line each: a scene that changed, an entity
    /// that names nothing, the engine the capture came from.
    pub notes: Vec<String>,
    pub assets: Vec<AssetCheck>,
    /// The capture's own list of what a replay draws fresh.
    pub fresh_only: Vec<String>,
    /// The passes this frame can be drawn without.
    pub skippable: Vec<Skippable>,
    /// Each style's frame-to-frame history as captured (#1136), handed
    /// back before every draw.
    history: Vec<flint_render::stylize::history::ImportedHistory>,
    /// What the styles did with it at the last draw, a line each.
    pub history_notes: Vec<String>,
}

/// A capture's file under `dir`.
fn read(dir: &Path, name: &str) -> Result<Vec<u8>> {
    std::fs::read(dir.join(name)).with_context(|| format!("reading {name}"))
}

/// Where the capture's scene is: as given, else under the capture
/// folder's ancestors (a capture written from the project root, opened
/// from elsewhere).
fn find_scene(recorded: &str, capture: &Path) -> Option<PathBuf> {
    let path = PathBuf::from(recorded);
    if path.is_file() {
        return Some(path);
    }
    let absolute = std::fs::canonicalize(capture).ok()?;
    absolute
        .ancestors()
        .skip(1)
        .map(|dir| dir.join(recorded))
        .find(|p| p.is_file())
}

impl ReplayHost {
    /// Load the capture at `dir` (described by `description`) for replay.
    /// `scene` overrides where its scene is; `schemas` is `--schemas`.
    pub fn load(
        dir: &Path,
        description: &FrameDescription,
        scene: Option<&Path>,
        schemas: &[String],
    ) -> Result<Self> {
        let shown = dir.display().to_string().replace('\\', "/");
        let replay = description.replay.as_ref().filter(|r| r.available);
        let Some(replay) = replay else {
            anyhow::bail!(
                "{shown} holds no replay inputs (written by {}): capture with \
                 `flint render --frame-capture` or the player's --capture-frame-at, \
                 which write replay/",
                description
                    .written_by
                    .as_deref()
                    .unwrap_or("an older writer"),
            );
        };
        let inputs: ReplayInputs = serde_json::from_slice(&read(dir, "replay/inputs.json")?)
            .context("replay/inputs.json")?;
        if inputs.version > 1 {
            anyhow::bail!(
                "{shown}'s replay inputs are version {}, newer than this engine's 1",
                inputs.version
            );
        }
        let world_toml = String::from_utf8(read(dir, "replay/world.toml")?)
            .context("replay/world.toml isn't text")?;
        let skinning = read(dir, "replay/skinning.bin")?;
        // Each style's history (#1136): a capture from before it has none,
        // and its styles draw fresh as they always did
        let (history, history_missing) = flint_render::frame_replay::read_style_history(
            &dir.join("replay"),
            &inputs.renderer.style_history,
        );
        let particle_bytes = read(dir, "replay/particles.bin")?;

        let mut notes = history_missing;
        let recorded = description.scene.as_ref().map(|s| s.path.clone());
        let scene_path = match (scene, &recorded) {
            (Some(path), _) => path.to_path_buf(),
            (None, Some(recorded)) => find_scene(recorded, dir).with_context(|| {
                format!(
                    "the capture's scene {recorded} isn't there (from here or above {shown}): \
                     name it with --scene"
                )
            })?,
            (None, None) => anyhow::bail!("{shown} names no scene: name it with --scene"),
        };
        let scene_text = scene_path.to_string_lossy().replace('\\', "/");
        let recorded_hash = description.scene.as_ref().and_then(|s| s.sha256.clone());
        if let (Some(was), Some((now, _))) = (
            recorded_hash,
            flint_render::frame_replay::hash_file(&scene_path),
        ) {
            if was != now {
                notes.push(format!(
                    "the scene file {scene_text} has changed since the capture: the replay \
                     loads it as it is now"
                ));
            }
        }
        // Which engine drew it against which draws it now (#1208: a render's
        // capture names its commit too, so both sides usually can): the
        // commit this binary was built from, not the checkout it runs in
        // (#1290)
        let captured_at = description.engine.as_ref().and_then(|e| e.commit.as_ref());
        let here = flint_player::provenance::engine();
        if let Some(note) = commit_note(
            captured_at.map(String::as_str),
            here.commit.as_deref(),
            here.dirty,
        ) {
            notes.push(note);
        }

        // The render a capture of this frame would be, as words
        let (width, height) = (description.frame.size[0], description.frame.size[1]);
        // The quality as captured; the shadow map's own size is set after
        // it, since a host's --shadow-resolution (128 in a small fixture) is
        // below what the quality takes
        let shadow_resolution = description
            .targets
            .iter()
            .find(|t| t.id == "shadow_map")
            .map(|t| t.size[0]);
        let quality_text: Vec<String> = inputs
            .renderer
            .quality
            .iter()
            .filter(|(k, _)| k.as_str() != "shadow_resolution" || shadow_resolution.is_none())
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        let mut words: Vec<String> = vec![
            "render".into(),
            scene_text.clone(),
            "--width".into(),
            width.to_string(),
            "--height".into(),
            height.to_string(),
            "--no-grid".into(),
            "--msaa".into(),
            inputs.renderer.msaa.max(1).to_string(),
        ];
        if !quality_text.is_empty() {
            words.push("--quality".into());
            words.push(quality_text.join(","));
        }
        for dir in schemas {
            words.push("--schemas".into());
            words.push(dir.clone());
        }
        #[derive(clap::Parser)]
        struct Words {
            #[command(flatten)]
            args: RenderArgs,
        }
        let args = <Words as clap::Parser>::try_parse_from(&words)
            .map_err(|e| anyhow!("the replay's render: {e}"))?
            .args;

        let ctx = pollster::block_on(HeadlessContext::new(width, height))
            .context("Failed to create headless render context")?;
        let binding = flint_render::SceneBinding::for_scene(&scene_path);
        for message in binding.style_messages() {
            notes.push(format!("style: {message}"));
        }
        let (registry, _) = crate::commands::common_args::load_schemas(Some(&scene_text), schemas)?;
        let binding = binding.with_archetypes(&registry);
        let (world, scene_file) = load_settled(&scene_text, &registry)?;
        let config =
            post_process_config(&args, scene_file.post_process.as_ref(), binding.styles())?;
        let mut stage = Stage::load(
            &args,
            &ctx,
            StageScene {
                world,
                scene_file,
                binding: &binding,
                index: 0,
                cell: (0, 0),
                grid: None,
                config,
                look: None,
                timeline: None,
                // The player resolves a scene's procgen specs (textures,
                // meshes) and a render doesn't: draw as the writer drew
                resolve_procgen: description.written_by.as_deref() == Some("flint-player"),
            },
        )?;

        // The world as it stood, by name, over the load
        let beside = scene_path.with_file_name(".replay.scene.toml");
        // Quietly: a saved world's model parts name the sub-meshes the load
        // registered, which no search finds as files
        let (captured, _, _) =
            flint_scene::load_scene_text_diagnosed(&world_toml, &beside, &registry)
                .context("replay/world.toml")?;
        notes.extend(lay_world(stage.game.world_mut(), &captured));

        // The particles' textures, when the frame drew any
        let particles = ReplayParticles::read(&particle_bytes, &inputs.particles);
        if inputs.frame.particles {
            load_particles(
                &stage.project,
                &scene_text,
                &mut stage.game,
                &mut stage.renderer,
                &ctx,
            )?;
        }

        // What the renderer held
        let world = stage.game.world();
        notes.extend(stage.renderer.restore_replay_state(
            &ctx.device,
            &ctx.queue,
            world,
            &inputs.renderer,
            &skinning,
        ));
        if let Some(&(_, mode)) = crate::commands::common_args::DEBUG_MODES
            .iter()
            .find(|(_, m)| format!("{m:?}") == inputs.renderer.debug_mode)
        {
            stage.renderer.set_debug_mode(mode);
        }
        let owned = inputs.frame.owned(world);
        for name in &owned.unknown {
            notes.push(format!(
                "the frame's inputs name {name:?}, which the replay's world has no entity of"
            ));
        }

        let mut quality = flint_render::QualitySettings::default();
        for e in quality.apply_str(&quality_text.join(",")) {
            notes.push(format!("quality: {e}"));
        }
        let config = captured_config(
            stage.renderer.post_process_config().clone(),
            description,
            &inputs,
            &mut notes,
        );
        let skippable = skippable(description, &config);
        let assets = check_assets(&replay.assets, &scene_text);
        for a in assets.iter().filter(|a| !a.same_bytes()) {
            notes.push(match a.state {
                "missing" => format!(
                    "{} {} is missing (was {}): its entities draw as the load draws a missing asset",
                    a.kind, a.name, a.path
                ),
                _ => format!(
                    "{} {} has changed since the capture ({}): the replay draws it as it is now",
                    a.kind,
                    a.name,
                    a.now.as_deref().unwrap_or(&a.path)
                ),
            });
        }
        Ok(Self {
            camera: inputs.camera.camera(),
            shadow_resolution,
            selected: inputs.renderer.selected.clone(),
            ctx,
            stage,
            inputs: owned,
            particles,
            config,
            quality,
            scene: scene_text,
            game_time: description.frame.game_time,
            notes,
            assets,
            fresh_only: replay.fresh_only.clone(),
            skippable,
            history,
            history_notes: Vec::new(),
        })
    }

    /// Draw the frame under `controls` and write it as a capture into `out`
    /// (made, or replaced if it holds a capture).
    pub fn draw(&mut self, controls: &Controls, out: &Path) -> Result<Drawn> {
        let mut refused = Vec::new();
        let mut config = self.config.clone();
        let mut quality = self.quality;
        for pass in &controls.skip {
            if !switch_off(pass, &mut config, &mut quality) {
                refused.push(format!("the {pass} pass can't be switched off"));
            }
        }
        let ctx = &self.ctx;
        let renderer = &mut self.stage.renderer;
        renderer.set_quality(&ctx.device, &quality);
        if let Some(size) = self.shadow_resolution {
            if renderer.shadow_resolution() != size {
                renderer.set_shadow_resolution(&ctx.device, size);
            }
        }
        renderer.set_post_process_config(config);
        renderer.ensure_style_resources(&ctx.device);
        renderer.ensure_fxaa_resources(&ctx.device);
        // Every replay starts from the captured frame's past, so two of
        // one frame agree: no history of the last draw, and the capture's
        // own (#1136) where it holds one
        renderer.reset_style_history();
        let _ = renderer.take_style_history_notes();
        renderer.import_style_history(self.history.clone());
        renderer.set_motion_time(self.inputs.state.time);

        let world = self.stage.game.world();
        let mut hidden = Vec::new();
        for name in &controls.hidden {
            match world.get_id(name) {
                Some(id) => hidden.push(id),
                None => refused.push(format!("no entity named {name:?} to hide")),
            }
        }
        // The outline marks the entity asked for, else the captured selection
        let outlined = controls.outline.as_ref().or(self.selected.as_ref());
        let outline = outlined.and_then(|name| world.get_id(name));
        if let (Some(name), None) = (&controls.outline, outline) {
            refused.push(format!("no entity named {name:?} to outline"));
        }
        renderer.set_selected_entity(outline);
        let limit = match (controls.draws, controls.stepped()) {
            (Some(draws), Ok(pass)) => Some(flint_render::DrawLimit { pass, draws }),
            (Some(_), Err(why)) => {
                refused.push(why);
                None
            }
            (None, _) => None,
        };
        let draw_controls = flint_render::DrawControls { hidden, limit };
        let any = !draw_controls.hidden.is_empty() || draw_controls.limit.is_some();

        let out_text = out.to_string_lossy().replace('\\', "/");
        let wanted = crate::commands::render_frame_dump::Wanted {
            dump: None,
            capture: Some(&out_text),
        };
        let armed = crate::commands::render_frame_dump::arm(ctx, renderer, wanted);
        let inputs = flint_render::FrameInputs {
            draw_controls: any.then_some(&draw_controls),
            ..self.inputs.inputs()
        };
        renderer.prepare(&ctx.device, &ctx.queue, world, &inputs);
        if self.inputs.state.particles {
            renderer.update_particles(&ctx.device, &ctx.queue, &self.particles.draws());
        }
        renderer.render_frame_to(
            &ctx.device,
            &ctx.queue,
            ctx.depth_target(),
            &self.camera,
            &ctx.color_view,
            Some(world),
        );
        pollster::block_on(ctx.read_pixels()).context("reading the replayed frame")?;
        self.history_notes = renderer.take_style_history_notes();

        let mut notes = vec!["replayed by flint frame replay".to_string()];
        notes.extend(self.history_notes.iter().map(|n| format!("history: {n}")));
        if !controls.hidden.is_empty() {
            notes.push(format!("hidden: {}", controls.hidden.join(", ")));
        }
        if let (Some(n), Some(pass)) = (controls.draws, limit.map(|l| l.pass)) {
            notes.push(format!("the {} pass stopped after {n} draws", pass.name()));
        }
        if !controls.skip.is_empty() {
            notes.push(format!("switched off: {}", controls.skip.join(", ")));
        }
        if let Some(name) = &controls.outline {
            notes.push(format!("outlined: {name}"));
        }
        crate::commands::render_frame_dump::write(
            armed,
            ctx,
            renderer,
            Some(world),
            wanted,
            crate::commands::render_frame_dump::Origin {
                scene: &self.scene,
                game_time: self.game_time,
                written_by: "flint frame replay",
                notes,
            },
            None,
        )?;
        Ok(Drawn { refused })
    }
}

/// The note on which engine commit drew the capture and which draws the
/// replay: the same one, another (the comparison is across commits), or
/// only the captured one known. Nothing when the capture names none.
fn commit_note(captured: Option<&str>, here: Option<&str>, dirty: Option<bool>) -> Option<String> {
    let captured = captured?;
    let short = |c: &str| c.chars().take(9).collect::<String>();
    let changes = if dirty == Some(true) {
        "; this engine's source has uncommitted changes"
    } else {
        ""
    };
    Some(match here {
        Some(here) if here == captured => {
            format!("captured at this engine's commit, {}{changes}", short(here))
        }
        Some(here) => format!(
            "captured at engine commit {} and replayed at {}{changes}: the comparison \
             below is across commits",
            short(captured),
            short(here)
        ),
        None => format!(
            "captured at engine commit {captured}: the comparison below says whether this \
             engine draws it the same"
        ),
    })
}

/// Lay `captured` over `world` by entity name (ADR 0126 decision 4): every
/// captured entity's archetype, components and parent; one `world` lacks
/// spawned; one the capture lacks despawned. What it did, a line each.
fn lay_world(world: &mut flint_ecs::FlintWorld, captured: &flint_ecs::FlintWorld) -> Vec<String> {
    let mut said = Vec::new();
    let mut spawned = Vec::new();
    let names: Vec<String> = captured
        .all_entities()
        .into_iter()
        .map(|i| i.name)
        .collect();
    for name in &names {
        if world.get_id(name).is_none() {
            match world.spawn(name.clone()) {
                Ok(_) => spawned.push(name.clone()),
                Err(e) => said.push(format!("couldn't spawn the captured {name}: {e}")),
            }
        }
    }
    for name in &names {
        let (Some(from), Some(to)) = (captured.get_id(name), world.get_id(name)) else {
            continue;
        };
        if let Some(c) = captured.get_components(from) {
            let (data, archetype) = (c.data.clone(), c.archetype.clone());
            if let Some(mut m) = world.get_components_mut(to) {
                m.set_archetype(archetype);
                for (k, v) in data {
                    m.set(k, v);
                }
            }
        }
        let parent = captured
            .get_parent(from)
            .and_then(|p| captured.get_name(p))
            .and_then(|p| world.get_id(p));
        match parent {
            Some(p) if world.get_parent(to) != Some(p) => {
                if let Err(e) = world.set_parent(to, p) {
                    said.push(format!(
                        "couldn't put {name} under its captured parent: {e}"
                    ));
                }
            }
            None if world.get_parent(to).is_some() => said.push(format!(
                "{name} has a parent in the load and none in the capture: left under it"
            )),
            _ => {}
        }
    }
    let extra: Vec<String> = world
        .all_entities()
        .into_iter()
        .map(|i| i.name)
        .filter(|n| captured.get_id(n).is_none())
        .collect();
    for name in &extra {
        if let Some(id) = world.get_id(name) {
            let _ = world.despawn(id);
        }
    }
    if !spawned.is_empty() {
        said.push(format!(
            "the capture's world has {} entities the load doesn't: spawned ({})",
            spawned.len(),
            spawned.join(", ")
        ));
    }
    if !extra.is_empty() {
        said.push(format!(
            "the load has {} entities the capture's world didn't: left out ({})",
            extra.len(),
            extra.join(", ")
        ));
    }
    said
}

/// The config the frame drew with: `base` (the scene's, as loaded), the
/// manifest's flat `[post_process]` keys over it, then each style as the
/// capture's renderer had it (its reveal region kept from the scene: the
/// capture holds it only as text).
fn captured_config(
    mut config: flint_render::PostProcessConfig,
    description: &FrameDescription,
    inputs: &ReplayInputs,
    notes: &mut Vec<String>,
) -> flint_render::PostProcessConfig {
    for (key, value) in &description.frame.post_process {
        let Ok(value) = serde_json::from_value::<toml::Value>(value.clone()) else {
            continue;
        };
        match config.param_mut(key) {
            Some(param) => {
                if let Err(e) = param.set(&value) {
                    notes.push(format!("post_process.{key}: {e}"));
                }
            }
            None => notes.push(format!(
                "post_process.{key} is no key of this engine's: left as the scene has it"
            )),
        }
    }
    let names: Vec<String> = config.styles.iter().map(|(n, _)| n.to_string()).collect();
    for name in names {
        if !inputs.renderer.styles.contains_key(&name) {
            config.styles.set_enabled(&name, false);
        }
    }
    for (name, s) in &inputs.renderer.styles {
        let reveal = config.styles.reveal(name).cloned().unwrap_or_default();
        config.styles.put(
            name,
            Some(flint_render::stylize::StyleSetting {
                enabled: s.enabled,
                params: s.params.clone(),
                options: s.options.clone(),
                order: s.order,
                feeds_bloom: s.feeds_bloom,
                blend: s.blend,
                reveal,
                follows_tear: s.follows_tear,
                off_everywhere: s.off_everywhere,
            }),
        );
    }
    config
}

/// The fixed passes a replay switches off, by the key each one has.
const SWITCHES: [(&str, &str); 6] = [
    ("Shadows", "shadows off (quality shadows = false)"),
    ("SSAO", "ssao_enabled = false"),
    ("Volumetric", "volumetric_enabled = false"),
    ("Bloom", "bloom_enabled = false"),
    ("FXAA", "fxaa = false"),
    ("Fog veil", "fog_enabled = false"),
];

/// Switch `pass` off in `config` / `quality`; false when it is no pass a
/// replay can switch off. A style's pass is its style's name.
fn switch_off(
    pass: &str,
    config: &mut flint_render::PostProcessConfig,
    quality: &mut flint_render::QualitySettings,
) -> bool {
    let base = pass.split(" (").next().unwrap_or(pass);
    match base {
        "Shadows" => quality.shadows = false,
        "SSAO" => config.ssao_enabled = false,
        "Volumetric" => config.volumetric_enabled = false,
        "Bloom" => config.bloom_enabled = false,
        "FXAA" => config.fxaa = false,
        "Fog veil" => config.fog_enabled = false,
        _ => match style_of(base, config) {
            Some(name) => config.styles.set_enabled(&name, false),
            None => return false,
        },
    }
    true
}

/// The enabled style a pass named `pass` is (its name, any case, or its
/// name and a step: `kuwahara structure`).
fn style_of(pass: &str, config: &flint_render::PostProcessConfig) -> Option<String> {
    let lower = pass.to_lowercase();
    config
        .styles
        .iter()
        .filter(|(_, s)| s.enabled)
        .map(|(n, _)| n.to_string())
        .find(|n| lower == *n || lower.starts_with(&format!("{n} ")))
}

/// The passes of `description` that ran and a replay can switch off.
fn skippable(
    description: &FrameDescription,
    config: &flint_render::PostProcessConfig,
) -> Vec<Skippable> {
    let mut out: Vec<Skippable> = Vec::new();
    for pass in description.all_passes() {
        if !pass.ran || out.iter().any(|s| s.pass == pass.name) {
            continue;
        }
        let does = SWITCHES
            .iter()
            .find(|(name, _)| *name == pass.name)
            .map(|(_, does)| does.to_string())
            .or_else(|| style_of(&pass.name, config).map(|s| format!("the {s} style off")));
        if let Some(does) = does {
            out.push(Skippable {
                pass: pass.name.clone(),
                does,
            });
        }
    }
    out
}

/// Each of `assets` checked against its recorded hash: where it was, else
/// where the scene's search finds it now.
fn check_assets(assets: &[AssetDescription], scene: &str) -> Vec<AssetCheck> {
    let config = ModelLoadConfig::from_scene_path(scene);
    assets
        .iter()
        .map(|a| {
            let check = |state, now| AssetCheck {
                kind: a.kind.clone(),
                name: a.name.clone(),
                path: a.path.clone(),
                state,
                now,
            };
            let at_path = flint_render::frame_replay::hash_file(Path::new(&a.path));
            match (&a.sha256, at_path) {
                (Some(was), Some((now, _))) if *was == now => return check("same", None),
                (None, Some(_)) => return check("same", None),
                _ => {}
            }
            let found = flint_render::frame_replay::resolve_asset(&config, &a.kind, &a.name)
                .ok()
                .and_then(|p| {
                    let hash = flint_render::frame_replay::hash_file(&p)?.0;
                    Some((flint_core::project::shown(&p), hash))
                });
            match found {
                Some((path, hash)) if a.sha256.as_ref() == Some(&hash) => {
                    check(if path == a.path { "same" } else { "moved" }, Some(path))
                }
                Some((path, _)) => check("changed", Some(path)),
                None => check("missing", None),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::commit_note;

    #[test]
    fn the_commit_note_says_whether_the_replay_is_across_commits() {
        let (a, b) = ("beef94170000", "c0ffee000000");
        assert_eq!(commit_note(None, Some(a), Some(false)), None);
        let same = commit_note(Some(a), Some(a), Some(false)).unwrap();
        assert!(same.contains("this engine's commit, beef94170"), "{same}");
        let other = commit_note(Some(a), Some(b), Some(true)).unwrap();
        assert!(
            other.contains("beef94170 and replayed at c0ffee000"),
            "{other}"
        );
        assert!(other.contains("uncommitted"), "{other}");
        assert!(other.contains("across commits"), "{other}");
        let unknown = commit_note(Some(a), None, None).unwrap();
        assert!(unknown.contains(a), "{unknown}");
    }
}
