//! SPIKE (#1082, throwaway, never for main): capture one frame of a
//! `flint render --time` Stage to a folder, measure it, then replay it on a
//! fresh Stage loaded from the captured world and compare the bytes.
//!
//! `FLINT_SPIKE_CAPTURE=<dir>` turns it on; `FLINT_SPIKE_AT=<t>` picks the
//! capture's moment (default: every capture); `FLINT_SPIKE_FRESH=1` draws the
//! captured frame with its style history reset.

use super::{Stage, StageScene};
use anyhow::{Context, Result};
use flint_render::{FrameInputs, HeadlessContext};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The frame's inputs, owned (FrameInputs borrows).
pub struct Inputs {
    pub time: f64,
    pub dt: f32,
    pub look_time: Option<f64>,
    pub grass_time: Option<f32>,
    pub grass_wooze: Option<flint_render::ShallowWoozeDraw>,
    pub shallow_wooze: Vec<(flint_core::EntityId, flint_render::ShallowWoozeDraw)>,
    pub boil: flint_wooze::BoilFrame,
    pub pixels_per_point: f32,
    pub view_2d: flint_render::View2d,
    pub particles: bool,
    pub ocean_tint: Option<flint_render::OceanTint>,
}

pub fn due(t: f64) -> bool {
    if std::env::var_os("FLINT_SPIKE_CAPTURE").is_none() {
        return false;
    }
    match std::env::var("FLINT_SPIKE_AT")
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
    {
        Some(at) => (t - at).abs() < 1e-6,
        None => true,
    }
}

fn ms(a: Instant) -> f64 {
    a.elapsed().as_secs_f64() * 1000.0
}

fn frame_inputs<'a>(
    si: &'a Inputs,
    particles: Option<&'a flint_particles::ParticleSync>,
) -> FrameInputs<'a> {
    FrameInputs {
        time: si.time,
        dt: si.dt,
        look_time: si.look_time,
        grass_time: si.grass_time,
        grass_wooze: si.grass_wooze,
        shallow_wooze: &si.shallow_wooze,
        boil: si.boil,
        pixels_per_point: si.pixels_per_point,
        view_2d: si.view_2d,
        particles: if si.particles { particles } else { None },
        ocean_tint: si.ocean_tint,
        ..Default::default()
    }
}

pub fn run(
    stage: &mut Stage,
    args: &super::RenderArgs,
    ctx: &HeadlessContext,
    t: f64,
    si: Inputs,
    original: &[u8],
) -> Result<()> {
    let dir = PathBuf::from(std::env::var("FLINT_SPIKE_CAPTURE").unwrap());
    let images = dir.join("images");
    std::fs::create_dir_all(&images)?;
    let mut report = serde_json::Map::new();
    let mut put = |k: &str, v: serde_json::Value| {
        report.insert(k.to_string(), v);
    };
    put("scene", args.scene.clone().into());
    put("t", t.into());
    put("size", format!("{}x{}", ctx.width, ctx.height).into());
    put("adapter", format!("{:?}", ctx.adapter_info).into());

    // 1. Every target read back, in one submission after the frame's
    let extra: [(&str, &wgpu::Texture); 1] = [("final", &ctx.color_texture)];
    let (taps, timing) = stage
        .renderer
        .spike_tap_all(&ctx.device, &ctx.queue, &extra);
    // A second time, warm (buffers made again: what a second capture costs)
    let (_, warm) = stage
        .renderer
        .spike_tap_all(&ctx.device, &ctx.queue, &extra);
    eprintln!("SPIKE taps cold: {timing:?}");
    eprintln!("SPIKE taps warm: {warm:?}");
    put("tap_cold", format!("{timing:?}").into());
    put("tap_warm", format!("{warm:?}").into());

    // 2. Written: PNG where 8-bit, raw bytes otherwise (what goes off-thread)
    let t_write = Instant::now();
    let mut written = 0u64;
    let mut list = Vec::new();
    for tap in &taps {
        let name = tap.name.replace('.', "_");
        let (path, bytes) = match tap.format {
            wgpu::TextureFormat::Rgba8UnormSrgb | wgpu::TextureFormat::Rgba8Unorm => {
                let p = images.join(format!("{name}.png"));
                image::save_buffer(
                    &p,
                    &tap.bytes,
                    tap.width,
                    tap.height,
                    image::ColorType::Rgba8,
                )?;
                (p.clone(), std::fs::metadata(&p)?.len())
            }
            wgpu::TextureFormat::Bgra8UnormSrgb | wgpu::TextureFormat::Bgra8Unorm => {
                let mut rgba = tap.bytes.clone();
                for px in rgba.chunks_exact_mut(4) {
                    px.swap(0, 2);
                }
                let p = images.join(format!("{name}.png"));
                image::save_buffer(&p, &rgba, tap.width, tap.height, image::ColorType::Rgba8)?;
                (p.clone(), std::fs::metadata(&p)?.len())
            }
            wgpu::TextureFormat::R8Unorm => {
                let p = images.join(format!("{name}.png"));
                image::save_buffer(&p, &tap.bytes, tap.width, tap.height, image::ColorType::L8)?;
                (p.clone(), std::fs::metadata(&p)?.len())
            }
            _ => {
                let p = images.join(format!("{name}.bin"));
                std::fs::write(&p, &tap.bytes)?;
                (p.clone(), tap.bytes.len() as u64)
            }
        };
        written += bytes;
        list.push(serde_json::json!({
            "name": tap.name, "format": format!("{:?}", tap.format),
            "w": tap.width, "h": tap.height, "raw": tap.bytes.len(),
            "file": path.file_name().unwrap().to_string_lossy(), "bytes": bytes,
        }));
    }
    let write_ms = ms(t_write);
    eprintln!(
        "SPIKE wrote {} images, {:.1} MB, in {write_ms:.1} ms",
        taps.len(),
        written as f64 / 1e6
    );
    put("images", list.into());
    put("images_bytes", written.into());
    put("images_write_ms", write_ms.into());

    // 3. The world as it stood: entities from the world, scene tables from
    // the file, no prefabs (the world has them expanded)
    let (registry, _) = super::super::common_args::load_schemas(Some(&args.scene), &args.schemas)?;
    let t_world = Instant::now();
    let (_, original_file) = flint_scene::load_scene(&args.scene, &registry)?;
    let mut snapshot = original_file.clone();
    snapshot.prefabs.clear();
    snapshot.entities.clear();
    flint_scene::update_scene_file(stage.game.world(), &mut snapshot);
    let text = toml::to_string_pretty(&snapshot)?;
    let world_ms = ms(t_world);
    std::fs::write(dir.join("world.scene.toml"), &text)?;
    eprintln!(
        "SPIKE world: {} entities, {:.1} KB of TOML, {world_ms:.1} ms (incl. re-reading the scene file)",
        stage.game.world().entity_count(),
        text.len() as f64 / 1e3
    );
    put("world_entities", stage.game.world().entity_count().into());
    put("world_toml_bytes", text.len().into());

    // 4. The rest of the frame's state as text, to size it
    let config = format!("{:#?}", stage.renderer.effective_post_process_config());
    std::fs::write(dir.join("post_process_config.txt"), &config)?;
    put("drawn_config_debug_bytes", config.len().into());
    std::fs::write(dir.join("camera.txt"), {
        let c = &stage.camera;
        format!(
            "position {:?}
target {:?}
up {:?}
fov {} near {} far {} aspect {} ortho {} {}
",
            c.position,
            c.target,
            c.up,
            c.fov,
            c.near,
            c.far,
            c.aspect,
            c.orthographic,
            c.ortho_height
        )
    })?;
    put("shallow_wooze_draws", si.shallow_wooze.len().into());
    put("boil", format!("{:?}", si.boil).into());
    let particles_alive = stage.game.particles().sync.total_alive();
    put("particles_alive", particles_alive.into());
    put(
        "particle_instance_bytes",
        (particles_alive * std::mem::size_of::<flint_render::ParticleInstance>()).into(),
    );

    // 5. Replay: the captured world loaded fresh beside the scene (so its
    // assets resolve as the scene's do), drawn once with the captured
    // inputs, the clock-driven uploads (living props' blends, bones)
    // taken from the capturing game as stand-ins for captured blobs
    let beside = Path::new(&args.scene).with_file_name(".spike-replay.scene.toml");
    let t_replay = Instant::now();
    let (world, scene_file) = flint_scene::load_scene_text(&text, &beside, &registry)?;
    // FLINT_SPIKE_KEEP_IDS (default on): the replay world takes the captured
    // world's ids (a manifest would record name -> id), since bones, blends
    // and shallow wooze are keyed by id
    let keep_ids = std::env::var_os("FLINT_SPIKE_NEW_IDS").is_none();
    // FLINT_SPIKE_BASE=scene: the replay starts from the scene file as
    // authored (the host's load derives model parts and living meshes as
    // ever) and the captured components go over it by name
    let from_scene = std::env::var("FLINT_SPIKE_BASE").as_deref() == Ok("scene");
    let (world, scene_file) = if from_scene {
        flint_scene::load_scene(&args.scene, &registry)?
    } else {
        (world, scene_file)
    };
    let world = if keep_ids && !from_scene {
        let captured = stage.game.world();
        let mut w = flint_ecs::FlintWorld::new();
        let mut infos = captured.all_entities();
        infos.sort_by_key(|i| i.id.raw());
        for info in &infos {
            w.spawn_with_id(info.id, info.name.clone())?;
        }
        for info in &infos {
            if let Some(p) = captured.get_parent(info.id) {
                w.set_parent(info.id, p)?;
            }
            let Some(lid) = world.get_id(&info.name) else {
                continue;
            };
            if let Some(c) = world.get_components(lid) {
                let (data, arch) = (c.data.clone(), c.archetype.clone());
                if let Some(mut m) = w.get_components_mut(info.id) {
                    m.set_archetype(arch);
                    for (k, v) in data {
                        m.set(k, v);
                    }
                }
            }
        }
        w
    } else {
        world
    };
    let binding =
        flint_render::SceneBinding::for_scene(Path::new(&args.scene)).with_archetypes(&registry);
    let config = stage.renderer.post_process_config().clone();
    // The scene path names the folder assets resolve from: the same folder
    let replay_args = args;
    let mut replay = Stage::load(
        &replay_args,
        ctx,
        StageScene {
            world,
            scene_file,
            binding: &binding,
            index: 0,
            cell: (0, 0),
            grid: None,
            config: config.clone(),
            look: None,
            timeline: None,
        },
    )
    .context("replay stage")?;
    // Stage::load ran the game's first step (scripts' on_init, particle
    // defaults): a replay draws the captured world as it was, so put every
    // captured component back
    let restore = std::env::var_os("FLINT_SPIKE_NO_RESTORE").is_none();
    if restore {
        let (captured_world, _) = flint_scene::load_scene_text(&text, &beside, &registry)?;
        let w = replay.game.world_mut();
        for info in captured_world.all_entities() {
            let (Some(id), Some(c)) =
                (w.get_id(&info.name), captured_world.get_components(info.id))
            else {
                continue;
            };
            let data = c.data.clone();
            if let Some(mut m) = w.get_components_mut(id) {
                for (k, v) in data {
                    m.set(k, v);
                }
            }
        }
    }
    // How the replay's world differs from the captured one
    {
        let a = stage.game.world();
        let b = replay.game.world();
        let mut notes = Vec::new();
        for info in a.all_entities() {
            match b.get_id(&info.name) {
                None => notes.push(format!("missing in replay: {}", info.name)),
                Some(id) => {
                    if id != info.id {
                        notes.push(format!("id moved: {} {:?} -> {:?}", info.name, info.id, id));
                    }
                    let ca = a.get_components(info.id).map(|c| c.data.clone());
                    let cb = b.get_components(id).map(|c| c.data.clone());
                    if let (Some(ca), Some(cb)) = (ca, cb) {
                        for (k, v) in &ca {
                            match cb.get(k) {
                                Some(w) if w == v => {}
                                Some(w) => notes.push(format!("{}.{k}: {v} -> {w}", info.name)),
                                None => notes.push(format!("{}.{k}: dropped", info.name)),
                            }
                        }
                        for k in cb.keys() {
                            if !ca.contains_key(k) {
                                notes.push(format!("{}.{k}: added", info.name));
                            }
                        }
                    }
                }
            }
        }
        for info in b.all_entities() {
            if a.get_id(&info.name).is_none() {
                notes.push(format!("extra in replay: {}", info.name));
            }
        }
        eprintln!("SPIKE world diff: {} notes", notes.len());
        for n in notes.iter().take(40) {
            eprintln!("  {n}");
        }
        std::fs::write(
            dir.join("world_diff.txt"),
            notes.join(
                "
",
            ),
        )?;
    }
    replay.camera = stage.camera.clone();
    replay.renderer.set_post_process_config(config);
    replay.renderer.ensure_style_resources(&ctx.device);
    replay.renderer.ensure_fxaa_resources(&ctx.device);
    replay.renderer.set_motion_time(t);
    let uploads = std::env::var_os("FLINT_SPIKE_NO_UPLOADS").is_none();
    if uploads {
        stage
            .game
            .upload_living(&mut replay.renderer, &ctx.device, &ctx.queue);
        if let Some(posed) = stage.game.animation() {
            posed.upload(&mut replay.renderer, &ctx.device, &ctx.queue);
        }
    }
    // The skinning state read back from the capturing renderer's GPU
    // buffers, written into the replay's (FLINT_SPIKE_NO_GPU_BONES skips it)
    if std::env::var_os("FLINT_SPIKE_NO_GPU_BONES").is_none() {
        let skin = stage.renderer.spike_skinning(&ctx.device, &ctx.queue);
        eprintln!(
            "SPIKE skinning: {} bone buffers, {} shadow, {} tinted, {} with echoes; {:.1} KB, read in {:.1} ms",
            skin.bones.len(),
            skin.shadow_bones.len(),
            skin.part_tints.len(),
            skin.echo_entities,
            skin.bytes as f64 / 1e3,
            skin.read_ms
        );
        replay
            .renderer
            .spike_put_skinning(&ctx.device, &ctx.queue, &skin);
    }
    let inputs = frame_inputs(&si, Some(&stage.game.particles().sync));
    replay
        .renderer
        .prepare(&ctx.device, &ctx.queue, replay.game.world(), &inputs);
    replay.renderer.render_frame_to(
        &ctx.device,
        &ctx.queue,
        ctx.depth_target(),
        &replay.camera,
        &ctx.color_view,
        Some(replay.game.world()),
    );
    let pixels = pollster::block_on(ctx.read_pixels()).context("replay read")?;
    let replay_ms = ms(t_replay);

    let mut differing = 0usize;
    let mut max = 0u8;
    for (a, b) in original.chunks_exact(4).zip(pixels.chunks_exact(4)) {
        if a != b {
            differing += 1;
            for i in 0..4 {
                max = max.max(a[i].abs_diff(b[i]));
            }
        }
    }
    let pixels_total = original.len() / 4;
    eprintln!(
        "SPIKE replay: {differing} of {pixels_total} pixels differ (max channel {max}), load+draw {replay_ms:.0} ms{}",
        if uploads { "" } else { " [no uploads]" }
    );
    image::save_buffer(
        dir.join("captured.png"),
        original,
        ctx.width,
        ctx.height,
        image::ColorType::Rgba8,
    )?;
    image::save_buffer(
        dir.join("replayed.png"),
        &pixels,
        ctx.width,
        ctx.height,
        image::ColorType::Rgba8,
    )?;
    if differing > 0 {
        let diff: Vec<u8> = original
            .chunks_exact(4)
            .zip(pixels.chunks_exact(4))
            .flat_map(|(a, b)| {
                if a == b {
                    [a[0] / 4, a[1] / 4, a[2] / 4, 255]
                } else {
                    [255, 0, 255, 255]
                }
            })
            .collect();
        image::save_buffer(
            dir.join("diff.png"),
            &diff,
            ctx.width,
            ctx.height,
            image::ColorType::Rgba8,
        )?;
    }
    put("replay_differing_pixels", differing.into());
    put("replay_max_channel_diff", max.into());
    put("replay_ms", replay_ms.into());
    put(
        "fresh_history",
        std::env::var_os("FLINT_SPIKE_FRESH").is_some().into(),
    );
    put("uploads", uploads.into());
    std::fs::write(
        dir.join("spike.json"),
        serde_json::to_string_pretty(&serde_json::Value::Object(report))?,
    )?;
    Ok(())
}
