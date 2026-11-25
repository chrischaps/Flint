//! The one-renderer still (#1145): a frame or a sweep's columns drawn on
//! one renderer, `--gpu-time`, `--ai-refine`, and the measures and ASCII
//! text a still and a stage both write.

use super::*;

/// What the dispatch hands the one-renderer still (#1145), worked out
/// already, as [`StagedRender`] is for the stage path: the scene as it loaded,
/// its binding and styles, every column's post-processing, and what was asked
/// of the frame.
pub(super) struct StillRender<'a> {
    pub(super) world: flint_ecs::FlintWorld,
    pub(super) scene_file: flint_scene::SceneFile,
    /// The scene as the renderer binds it (#739).
    pub(super) binding: &'a flint_render::SceneBinding,
    /// The registry this render draws with (#742), the host's copy's twin.
    pub(super) styles: flint_render::stylize::StyleRegistry,
    pub(super) assets: flint_core::project::AssetScope,
    pub(super) grid: Option<crate::commands::sweep::SweepGrid>,
    /// The sweep's cells, a row at a time; one, `(0, 0)`, without a sweep.
    pub(super) cells: Vec<(usize, usize)>,
    /// A cell's each, as `cells`.
    pub(super) configs: Vec<flint_render::PostProcessConfig>,
    /// What a sweep's cell is of its frame (#358).
    pub(super) view: crate::commands::sweep::CellView,
    /// The frame's size in pixels.
    pub(super) size: (u32, u32),
    /// The scene's `[post_process]`, under its own look, for `--ai-refine`.
    pub(super) scene_pp: Option<&'a flint_scene::PostProcessDef>,
    pub(super) look: Option<crate::commands::look::Look>,
    pub(super) reference: Option<crate::commands::look_measure::Reference>,
    pub(super) camera_path: Option<crate::commands::frame_seq::CameraPath>,
    /// `--mesh` (#222): a sequence a file a frame.
    pub(super) meshes: Vec<crate::commands::mesh_seq::MeshBinding>,
    /// `--ascii-text` / `--ascii-text-ansi`: the grid written beside the frame.
    pub(super) wants_text: bool,
    /// `--flip-stats` / `--flip-json`: a sequence's glyph flips counted.
    pub(super) wants_flips: bool,
    /// A reference grid scores frames by their glyphs: each grid is read.
    pub(super) reads_glyphs: bool,
}

/// A render with no clocks (#1145): the scene drawn as authored on one
/// renderer, a column's post-processing swapped in before each draw. A
/// `--frames` / `--mesh` sequence and an `--ai-refine` go on from here, on the
/// same renderer; under `--watch` the still stays, drawing again for each
/// save a user style's pass needs to see.
pub(super) fn render_still(
    args: &RenderArgs,
    host: &mut RenderHost,
    watch: Option<&mut crate::commands::render_watch::Watch>,
    still: StillRender,
) -> Result<()> {
    let StillRender {
        mut world,
        scene_file,
        binding,
        styles,
        assets,
        grid,
        cells,
        configs,
        view,
        size: (width, height),
        scene_pp,
        look,
        reference,
        camera_path,
        meshes,
        wants_text,
        wants_flips,
        reads_glyphs,
    } = still;
    // The scene's own block is what --ai-refine lays a candidate over
    #[cfg(not(feature = "ai"))]
    let _ = scene_pp;
    // A still draws the scene as authored, before any script (#371): said, as
    // the clocked path says it, since a look a script sets is otherwise missing
    // from the picture with no word why
    let scripted = named_with(&world, comp::SCRIPT);
    if !scripted.is_empty() {
        eprintln!("Warning: {}", scripts_not_run(&scripted));
    }

    let device = load_report::span(load_report::Stage::Device, String::new);
    let ctx = host.context(width, height, &args.debug.gpu)?;
    drop(device);

    // Configure camera — scene-level first, then CLI overrides take precedence
    let mut camera = Camera::new();
    camera.aspect = ctx.aspect_ratio();

    // Apply scene-level camera configuration
    let mut scene_set_position = false;
    if let Some(cam_def) = &scene_file.camera {
        if cam_def.projection == "orthographic" {
            camera.orthographic = true;
            if cam_def.ortho_height > 0.0 {
                camera.ortho_height = cam_def.ortho_height;
            }
        }
        if let Some(pos) = cam_def.position {
            camera.position = Vec3::new(pos[0], pos[1], pos[2]);
            scene_set_position = true;
        }
        if let Some(target) = cam_def.target {
            camera.target = Vec3::new(target[0], target[1], target[2]);
        }
        if let Some(fov) = cam_def.fov {
            camera.fov = fov;
        }
        if let Some(near) = cam_def.near {
            camera.near = near;
        }
        if let Some(far) = cam_def.far {
            camera.far = far;
        }
    }

    // Derive orbit parameters from position/target so update_orbit() is consistent
    if scene_set_position {
        let dir = camera.position - camera.target;
        camera.distance = dir.length();
        if camera.distance > 0.001 {
            let n = dir * (1.0 / camera.distance);
            camera.pitch = n.y.asin();
            camera.yaw = n.x.atan2(n.z);
        }
    }

    // CLI overrides take precedence
    if let Some(d) = args.camera.distance {
        camera.distance = d;
    }
    if let Some(y) = args.camera.yaw {
        camera.yaw = y.to_radians();
    }
    if let Some(p) = args.camera.pitch {
        camera.pitch = p.to_radians();
    }
    if let Some(t) = args.camera.target {
        camera.target = Vec3::new(t[0], t[1], t[2]);
    }
    if let Some(f) = args.camera.fov {
        camera.fov = f;
    }
    camera.update_orbit();

    // Create scene renderer
    let made = load_report::span(load_report::Stage::Renderer, String::new);
    let mut renderer = SceneRenderer::new_headless(
        &ctx.device,
        &ctx.queue,
        ctx.format,
        ctx.width,
        ctx.height,
        RendererConfig {
            show_grid: !args.camera.no_grid,
            sample_count: args.shading.msaa,
        },
    );
    drop(made);
    // Parts skipped or repeated by label (#1317)
    renderer.set_pass_knobs(args.measure.pass_knobs.knobs()?);

    // The scene bound, its levers set (#739), as every stage does
    bind_capture_scene(args, ctx, &mut renderer, &world, &scene_file, &binding);

    // Load models and textures from the scene
    let config = ModelLoadConfig::in_project(assets.project.clone(), &args.scene);
    let models = model_loader::load_models_from_world(
        &mut world,
        &mut renderer,
        &ctx.device,
        &ctx.queue,
        &config,
    );

    // Meshes swept along the scene's splines (#750); a still has no
    // physics, so their colliders are dropped
    flint_render::load_scene_splines(
        &args.scene,
        &mut world,
        &mut renderer,
        &ctx.device,
        &ctx.queue,
    );

    // The debug views, shadows, post-processing (the first column's, the
    // only one without --sweep), render scale and quality, as every stage
    set_up_capture(
        args,
        ctx,
        &mut renderer,
        CaptureSetup {
            scene_file: &scene_file,
            config: configs[0].clone(),
            text_capture: wants_text
                || wants_flips
                || args.measure.report.is_some()
                || reads_glyphs,
            first: true,
        },
    )?;
    // Post time for grain/mode animation and animated styles. Headless
    // renders leave this 0.0 so PNG snapshots stay deterministic;
    // --style-time (or the older --grain-time) pins an explicit frame.
    let time = still_time(args);
    // Poses held on twos (#405) tick with the post time from the first
    // frame, so a --frames sequence holds frame 0's pose through its tick
    renderer.set_motion_time(time);

    // Animation (#307), which runs IK after each step; without it, two-bone IK
    // (ADR 0070) poses authored chains before the single frame.
    let mut animation = pose_animation(args, &models, &config.project, &mut world, ANIM_DT, true);
    match &animation {
        Some(posed) => posed.upload(&mut renderer, &ctx.device, &ctx.queue),
        None => flint_animation::IkPass::new().run(&mut world),
    }

    if let Some(name) = &args.camera.select {
        let id = world
            .get_id(name)
            .with_context(|| format!("--select: no entity named '{name}'"))?;
        renderer.set_selected_entity(Some(id));
    }

    // Particles (ADR 0068): fixed-step, fixed-seed simulation so a snapshot
    // at a given --particle-time is reproducible run to run.
    let mut particle_sim = None;
    if let Some(seconds) = args.clock.particle_time {
        use flint_runtime::RuntimeSystem;
        let mut particles = flint_particles::ParticleSystem::new();
        let project = &config.project;
        flint_particles::load_particle_effects_from_world(project, &args.scene, &mut particles);
        particles
            .initialize(&mut world)
            .context("Failed to initialize particles")?;
        flint_render::load_scene_particle_textures(
            &mut renderer,
            &ctx.device,
            &ctx.queue,
            &particles.sync,
            project,
            Path::new(&args.scene).parent().unwrap_or(Path::new(".")),
        );
        particles.simulate_to(&world, seconds.max(0.0), 1.0 / 60.0);
        particles.pack(Some(camera.position_array()));
        say!(
            "Particles: {} alive across {} emitter(s) at t = {:.2}s",
            particles.sync.total_alive(),
            particles.sync.emitter_count(),
            seconds
        );
        particle_sim = Some(particles);
    }

    // The frame's inputs, and the world read into the draw lists (#739)
    renderer.prepare(
        &ctx.device,
        &ctx.queue,
        &world,
        &FrameInputs {
            particles: particle_sim.as_ref().map(|p| &p.sync),
            ..still_inputs(args, &camera, time)
        },
    );

    // GPU timing: the same frame many times, each pass's spread (#292)
    if let Some(frames) = args.measure.gpu_time {
        time_gpu(
            ctx,
            &mut renderer,
            &camera,
            frames,
            args.measure.gpu_time_json.as_deref(),
        )?;
    }

    let frames = match meshes.first() {
        Some(first) => Some(first.files.len() as u32),
        None => args.seq.frames,
    };
    if let Some(frames) = frames {
        return render_sequence(
            args,
            SequenceRig {
                ctx,
                renderer: &mut renderer,
                world: &mut world,
                camera,
                camera_path: camera_path.as_ref(),
                particles: particle_sim.as_mut(),
                animation: animation.as_mut(),
                meshes: &meshes,
                scene_name: &scene_file.scene.name,
            },
            frames,
        );
    }

    #[cfg(feature = "ai")]
    if args.refine.ai_refine {
        let look = look.expect("clap requires --look with --ai-refine");
        let reference = reference.expect("clap requires --reference with --ai-refine");
        return refine_look(args, look, reference, &mut |table| {
            let def = crate::commands::look::overlay(scene_pp, table)?;
            let config = post_process_config(args, Some(&def), renderer.styles())?;
            let scale = reads_glyphs.then(|| ink_scale(args, &config));
            let image = draw_frame(ctx, &mut renderer, &camera, Some(config), Some(&world))?;
            let ink = scale.and_then(|s| frame_ink(&renderer, ctx, &s));
            Ok(crate::commands::look_refine::Frame { image, ink })
        });
    }

    // The load is up; its last stage is the first draw (#1092)
    load_report::bring_up_done();
    // Draw and write the still or sheet; under --watch again on this same
    // renderer for each save of a user style's sources (#432). A redraw
    // sets the first column's config too: the last draw left the last's.
    let mut draw_and_write = |redraw: bool| -> Result<()> {
        // Render each column (one without --sweep), reading each back
        let mut frames = Vec::with_capacity(configs.len());
        let mut texts = Vec::with_capacity(configs.len());
        for (i, config) in configs.iter().enumerate() {
            // The frame's description (#1084) and capture (#1085): the
            // still's own frame
            let dump = crate::commands::render_frame_dump::Wanted::of(args)
                .filter(|_| i == 0)
                .map(|wanted| {
                    (
                        wanted,
                        crate::commands::render_frame_dump::arm(ctx, &mut renderer, wanted),
                    )
                });
            let pixels = draw_pixels(
                ctx,
                &mut renderer,
                &camera,
                (i > 0 || redraw).then(|| config.clone()),
                Some(&world),
            )?;
            first_capture_done();
            if let Some((wanted, armed)) = dump {
                let said = crate::commands::render_frame_dump::write(
                    armed,
                    ctx,
                    &mut renderer,
                    Some(&world),
                    wanted,
                    crate::commands::render_frame_dump::Origin {
                        scene: &args.scene,
                        game_time: None,
                        written_by: "flint render",
                        notes: Vec::new(),
                    },
                    Some(crate::commands::render_frame_dump::ReplayFacts {
                        inputs: flint_render::frame_replay::FrameInputsState::of(
                            &FrameInputs {
                                particles: particle_sim.as_ref().map(|p| &p.sync),
                                ..still_inputs(args, &camera, time)
                            },
                            &world,
                        ),
                        particles: particle_sim.as_ref().map(|p| &p.sync),
                    }),
                )?;
                for line in said {
                    say!("{line}");
                }
            }
            // The grid is read back once a frame: a second read finds nothing
            let measured = args.measure.report.is_some() || reads_glyphs;
            let text = (measured || wants_text)
                .then(|| renderer.style_text(&ctx.device, "ascii"))
                .flatten();
            if measured {
                texts.push(text.clone());
            }
            if wants_text {
                write_ascii_text(
                    args.out.ascii_text.as_deref(),
                    args.out.ascii_text_ansi.as_deref(),
                    text,
                )?;
            }
            let mut img = image::RgbaImage::from_raw(width, height, pixels)
                .context("Failed to create image from pixel data")?;
            // The style mask view says which hue is which style (#309): in the
            // terminal once, and on every frame's corner
            if renderer.debug_state().mode == DebugMode::StyleMask && !args.no_postprocess {
                let ids = renderer.style_ids();
                if i == 0 {
                    say!("{}", flint_render::style_mask::legend_line(ids));
                }
                crate::commands::style_sheet::draw_legend(
                    &mut img,
                    &flint_render::style_mask::legend(ids),
                );
            }
            // The edge view says which colour is which source and decision
            // (#349), and the thresholds it was drawn at
            if renderer.debug_state().mode == DebugMode::StyleEdges && !args.no_postprocess {
                let thresholds = flint_render::stylize::edge_view_thresholds(
                    renderer.effective_post_process_config(),
                );
                if i == 0 {
                    let names: Vec<&str> = flint_render::stylize::EDGE_VIEW_LEGEND
                        .iter()
                        .map(|(_, name)| *name)
                        .collect();
                    say!("ascii edges: {thresholds}");
                    say!(
                        "  pixels cyan/amber/magenta: {}; cells green/dashed/yellow/red/grey: {}",
                        names[..3].join(", "),
                        names[3..].join(", ")
                    );
                }
                let mut rows: Vec<(Option<[f32; 3]>, String)> =
                    flint_render::stylize::EDGE_VIEW_LEGEND
                        .iter()
                        .map(|(color, name)| (Some(*color), name.to_string()))
                        .collect();
                rows.extend(thresholds.split("; ").map(|t| (None, t.to_string())));
                // Nine rows: half the mask legend's size, so the card leaves the
                // scene readable
                let scale = (img.width() / 1280).clamp(1, 3);
                crate::commands::style_sheet::draw_card(&mut img, "ascii edges", &rows, scale);
            }
            if let Some(grid) = &grid {
                let (c, r) = cells[i];
                say!("  {}", grid.describe(c, r));
            }
            frames.push(img);
        }

        // A sweep whose neighbours drew the same showed nothing: say so (#216).
        // Judged on the frames as drawn, before a crop could hide a difference.
        if let Some(grid) = &grid {
            for warning in grid.identical_warnings(&frames) {
                eprintln!("Warning: {warning}");
            }
        }

        // The pictures the sheet is made of: each frame as its cell shows it
        // (cropped or shrunk under --sweep-crop / --sweep-scale, #358), and
        // for a crop the first frame whole, to show where they sit (#501)
        let locator = grid
            .as_ref()
            .and_then(|_| frames.first())
            .and_then(|f| view.locator(f));
        let pictures: Vec<image::RgbaImage> = match &grid {
            Some(_) => frames.into_iter().map(|f| view.cell(f)).collect(),
            None => frames,
        };

        // Measures (#357), from those same pixels. The ASCII section counts the
        // whole drawn frame's grid, which a crop shows only part of.
        let mut columns = Vec::new();
        if args.measure.report.is_some() || reference.is_some() {
            columns = measure_cells(
                args,
                &pictures,
                &texts,
                &configs,
                reference.as_ref(),
                grid.as_ref(),
                &cells,
            );
        }
        let nearest = crate::commands::look_measure::RenderReport::nearest_column(&columns);

        let mut pictures = pictures;
        let img = match &grid {
            None => pictures.swap_remove(0),
            Some(grid) => {
                let over = match &look {
                    Some(look) => format!("  over {}", look.name),
                    None => String::new(),
                };
                // Only a sheet that isn't of small frames says how big they were
                let scale = match view {
                    crate::commands::sweep::CellView::Small => String::new(),
                    _ => format!("  {}", view.describe(width, height)),
                };
                let reference_note = match &args.measure.reference {
                    Some(r) => format!("  d = distance to {r}"),
                    None => String::new(),
                };
                let subtitle = format!("sweep {}{over}{scale}{reference_note}", grid.name());
                let label = |s: &crate::commands::sweep::Sweep| {
                    (0..s.values.len()).map(|i| s.label(i)).collect()
                };
                match &grid.rows {
                    None => {
                        // With a reference, each label carries its distance and
                        // the nearest is starred (#357)
                        let labels = distance_labels(label(&grid.columns), &columns, nearest);
                        let mut text = crate::commands::style_sheet::SheetText::new(
                            scene_file.scene.name.clone(),
                            subtitle,
                            labels,
                        );
                        text.locator = locator;
                        crate::commands::style_sheet::compose(&pictures, &text)
                    }
                    Some(rows) => crate::commands::style_sheet::compose_grid(
                        &pictures,
                        &crate::commands::style_sheet::GridText {
                            title: scene_file.scene.name.clone(),
                            subtitle,
                            row_key: rows.name(),
                            column_labels: label(&grid.columns),
                            row_labels: label(rows),
                            time: String::new(),
                            locator,
                        },
                    ),
                }
            }
        };
        if let Some(path) = &args.measure.report {
            write_measures(
                args,
                path,
                look.as_ref(),
                grid.as_ref(),
                (width, height),
                columns,
                nearest,
            )?;
        }
        img.save(&args.output)
            .context(format!("Failed to save image to {}", args.output))?;

        say!(
            "Rendered {}x{} image to {}",
            img.width(),
            img.height(),
            args.output
        );

        Ok(())
    };
    draw_and_write(false)?;
    if let Some(watch) = watch {
        // The renderer's user styles are shared with this copy, so a WGSL
        // save it reloads is the pass's to see (#742)
        let mut watched = styles.clone();
        watch.redraws(&assets, &mut watched, || draw_and_write(true))?;
    }
    Ok(())
}

/// Frames `--gpu-time` leaves out at the start of `timed`: the first tenth,
/// at most 30, while pipelines, caches and the GPU's clocks settle (#292).
fn gpu_warmup(timed: usize) -> usize {
    (timed / 10).min(30)
}

/// `--gpu-time N`: the frame `renderer` holds drawn N times with GPU timing,
/// each frame's times kept raw and each pass's spread over them printed
/// (#292), with the style chain's targets; `--gpu-time-json` writes it too.
pub(super) fn time_gpu(
    ctx: &HeadlessContext,
    renderer: &mut SceneRenderer,
    camera: &Camera,
    frames: u32,
    json: Option<&str>,
) -> Result<()> {
    let (width, height) = (ctx.width, ctx.height);
    if !renderer.set_gpu_timing(&ctx.device, &ctx.queue, true) {
        anyhow::bail!("--gpu-time: this adapter has no TIMESTAMP_QUERY");
    }
    if let Some(timer) = renderer.gpu_timer_mut() {
        timer.record(true);
    }
    for _ in 0..frames.max(1) {
        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("GPU Time Frame"),
            });
        renderer.encode_to(
            &ctx.device,
            &ctx.queue,
            &mut encoder,
            ctx.depth_target(),
            camera,
            &ctx.color_view,
        );
        renderer.encode_gpu_timings(&ctx.device, &mut encoder);
        ctx.queue.submit(std::iter::once(encoder.finish()));
        renderer.frame_submitted();
        let _ = flint_render::wait_for_gpu(&ctx.device);
    }
    let spread = match renderer.gpu_timer_mut() {
        Some(timer) => {
            timer.flush(&ctx.device);
            let recorded = timer.recorded();
            let spread = flint_render::gpu_timer::spread(recorded, gpu_warmup(recorded.len()));
            timer.record(false);
            spread
        }
        None => Default::default(),
    };
    let (sw, sh) = renderer.scene_size();
    let adapter = &ctx.adapter_info;
    say!(
        "GPU time over {} frames at {}x{} (scene {}x{}) on {} ({:?}):",
        frames,
        width,
        height,
        sw,
        sh,
        adapter.name,
        adapter.backend
    );
    say!(
        "  {} frames timed, the first {} dropped as warm-up",
        spread.frames + spread.dropped,
        spread.dropped
    );
    for line in spread_lines(&spread) {
        say!("{line}");
    }
    if let Some(path) = json {
        let doc = spread_json(&spread, frames, (width, height), (sw, sh), adapter);
        std::fs::write(path, serde_json::to_string_pretty(&doc)?)
            .context(format!("Failed to write {path}"))?;
        say!("Wrote GPU timing to {path}");
    }
    // What each of the style chain's textures costs (#412): the
    // built-ins' and every user style's targets
    let targets = renderer.style_target_entries();
    if !targets.is_empty() {
        let total: u64 = targets.iter().map(|t| t.bytes).sum();
        say!(
            "Style targets: {} ({:.2} MB)",
            targets.len(),
            total as f64 / (1024.0 * 1024.0)
        );
        let w = targets.iter().map(|t| t.name.len()).max().unwrap_or(0);
        for t in &targets {
            say!(
                "  {:<w$} {:>5}x{:<5} {:<15} {:>7.2} MB",
                t.name,
                t.width,
                t.height,
                format!("{:?}", t.format),
                t.bytes as f64 / (1024.0 * 1024.0)
            );
        }
    }
    Ok(())
}

/// `--gpu-time`'s table: a header, a row a pass, then the frame total's.
/// The label column is as wide as the longest label (a style's, such as
/// "Display bloom").
fn spread_lines(spread: &flint_render::GpuSpread) -> Vec<String> {
    let rows: Vec<(String, &flint_render::PassSpread)> = spread
        .passes
        .iter()
        .map(|p| (p.name(), p))
        .chain(spread.frame.iter().map(|f| ("frame".to_string(), f)))
        .collect();
    let w = rows.iter().map(|(n, _)| n.len()).max().unwrap_or(0).max(12);
    let mut lines = vec![format!(
        "  {:<w$} {:>7} {:>7} {:>7} {:>7}  ms",
        "pass", "min", "p50", "p90", "max"
    )];
    for (name, p) in rows {
        lines.push(format!(
            "  {name:<w$} {:>7.3} {:>7.3} {:>7.3} {:>7.3}",
            p.min, p.p50, p.p90, p.max
        ));
    }
    lines
}

/// `--gpu-time-json`: the spread, each pass with its kept frames' raw times.
fn spread_json(
    spread: &flint_render::GpuSpread,
    frames: u32,
    (width, height): (u32, u32),
    (scene_width, scene_height): (u32, u32),
    adapter: &wgpu::AdapterInfo,
) -> serde_json::Value {
    // The passes in the shape `flint-player --perf-report` writes them too,
    // so `scripts/perf-compare.py` reads both (#1093)
    let mut doc = spread.to_json(true);
    let more = serde_json::json!({
        "format": "flint-gpu-time",
        "version": 1,
        "adapter": {
            "name": adapter.name,
            "backend": format!("{:?}", adapter.backend),
            "driver": adapter.driver,
            "driver_info": adapter.driver_info,
        },
        "build_profile": flint_player::provenance::BUILD_PROFILE,
        "engine_commit": flint_player::provenance::BUILD_COMMIT,
        "width": width,
        "height": height,
        "scene_width": scene_width,
        "scene_height": scene_height,
        "frames_drawn": frames.max(1),
        "frames_timed": spread.frames + spread.dropped,
    });
    if let (Some(doc), serde_json::Value::Object(more)) = (doc.as_object_mut(), more) {
        doc.extend(more);
    }
    doc
}

/// The reference a render measures against (#357): the `--reference`
/// picture and, with `--reference-text`, its glyph grid (#495), read
/// through the glyph set `config` draws with. A reference picture of an
/// ASCII frame with no grid is said once: its letters are scored as pixels.
pub(super) fn measured_reference(
    args: &RenderArgs,
    image: Option<image::RgbImage>,
    config: &flint_render::PostProcessConfig,
) -> Result<Option<crate::commands::look_measure::Reference>> {
    use crate::commands::look_measure as lm;
    let Some(image) = image else {
        return Ok(None);
    };
    let ink = match &args.measure.reference_text {
        Some(path) => Some(lm::load_reference_text(path, &ink_scale(args, config))?),
        None => {
            if config.styles.is_enabled("ascii") {
                say!(
                    "Note: the reference is scored by its pixels; give --reference-text \
                     (its --ascii-text) to score glyphs as glyphs (#495)"
                );
            }
            None
        }
    };
    Ok(Some(lm::Reference { image, ink }))
}

/// How a glyph's ink is read under `config`: its ASCII font's atlas.
pub(super) fn ink_scale(
    args: &RenderArgs,
    config: &flint_render::PostProcessConfig,
) -> crate::commands::look_measure::InkScale {
    crate::commands::look_measure::InkScale::for_config(config, Path::new(&args.scene))
}

/// The glyph grid the last frame drew, as ink; `None` when ASCII didn't draw.
#[cfg(feature = "ai")]
pub(super) fn frame_ink(
    renderer: &SceneRenderer,
    ctx: &HeadlessContext,
    scale: &crate::commands::look_measure::InkScale,
) -> Option<crate::commands::look_measure::InkGrid> {
    renderer
        .style_text(&ctx.device, "ascii")
        .map(|t| crate::commands::look_measure::InkGrid::from_text(&t, scale))
}

/// `--ai-refine` (#357): run the loop, then write the refined look, its log
/// and the sheet of rounds.
#[cfg(feature = "ai")]
pub(super) fn refine_look(
    args: &RenderArgs,
    look: crate::commands::look::Look,
    reference: crate::commands::look_measure::Reference,
    draw: &mut dyn FnMut(&toml::Table) -> Result<crate::commands::look_refine::Frame>,
) -> Result<()> {
    use crate::commands::look_refine as lr;
    let (agent, meter) = lr::build_agent(
        &args.refine.ai_provider,
        args.refine.ai_model.as_deref(),
        args.refine.max_cost,
    )?;
    let name = format!("{}_refined", look.name);
    let out = args
        .refine
        .refine_out
        .clone()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| lr::default_out(&args.output, &name));
    let reference_path = args.measure.reference.clone().unwrap_or_default();
    let log = lr::run(
        lr::Options {
            look: look.clone(),
            reference_path,
            reference,
            rounds: args.refine.refine_rounds.max(1),
            candidates: args.refine.refine_candidates.max(1),
        },
        agent.as_ref(),
        &meter,
        draw,
    )?;
    let text = lr::refined_look_toml(&log, &look, &name)?;
    if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).ok();
    }
    std::fs::write(&out, text).context(format!("Failed to write {}", out.display()))?;
    let log_path = out.with_extension("json");
    std::fs::write(&log_path, serde_json::to_string_pretty(&log)?)
        .context(format!("Failed to write {}", log_path.display()))?;
    let (labels, cells): (Vec<String>, Vec<image::RgbaImage>) = log.frames.iter().cloned().unzip();
    let sheet = crate::commands::style_sheet::compose(
        &cells,
        &crate::commands::style_sheet::SheetText::new(
            format!("refine {} toward {}", look.name, log.reference),
            format!(
                "{}: distance {:.2} -> {:.2}, {} round(s), {} call(s), ${:.4}",
                log.agent, log.start.score, log.end.score, log.rounds_run, log.calls, log.cost_usd
            ),
            labels,
        ),
    );
    sheet
        .save(&args.output)
        .context(format!("Failed to save image to {}", args.output))?;
    say!(
        "Refined {}: distance {:.2} -> {:.2} ({}), {} call(s), ${:.4}",
        look.name,
        log.start.score,
        log.end.score,
        log.stopped,
        log.calls,
        log.cost_usd
    );
    say!(
        "  by {}: start {}; end {}",
        log.metric.name(),
        log.start.terms_line(),
        log.end.terms_line()
    );
    say!(
        "Wrote {} (load it with --look {})",
        out.display(),
        out.display()
    );
    say!(
        "Wrote {} and the rounds' sheet {}",
        log_path.display(),
        args.output
    );
    Ok(())
}

/// Write the ASCII grid (#317) to --ascii-text / --ascii-text-ansi.
pub(super) fn write_ascii_text(
    text_path: Option<&str>,
    ansi_path: Option<&str>,
    text: Option<flint_render::stylize::GlyphText>,
) -> Result<()> {
    let Some(text) = text else {
        anyhow::bail!(
            "--ascii-text: the ASCII style didn't draw this frame; turn it on with --style ascii, \
             a look, or the scene's [post_process.styles.ascii] / ascii_style entities"
        );
    };
    if let Some(path) = text_path {
        std::fs::write(path, text.to_text()).context(format!("Failed to write {path}"))?;
        say!("Wrote {}x{} ASCII grid to {path}", text.cols, text.rows);
    }
    if let Some(path) = ansi_path {
        std::fs::write(path, text.to_ansi()).context(format!("Failed to write {path}"))?;
        say!(
            "Wrote {}x{} ANSI ASCII grid to {path}",
            text.cols,
            text.rows
        );
    }
    Ok(())
}

/// Measures (#357) of each cell as the sheet shows it, a line each in the
/// terminal. The ASCII section counts the whole drawn frame's grid, which a
/// crop shows only part of, and so does the glyph distance (#495).
pub(super) fn measure_cells(
    args: &RenderArgs,
    pictures: &[image::RgbaImage],
    texts: &[Option<flint_render::stylize::GlyphText>],
    configs: &[flint_render::PostProcessConfig],
    reference: Option<&crate::commands::look_measure::Reference>,
    grid: Option<&crate::commands::sweep::SweepGrid>,
    cells: &[(usize, usize)],
) -> Vec<crate::commands::look_measure::ColumnReport> {
    use crate::commands::look_measure as lm;
    let mut columns = Vec::with_capacity(pictures.len());
    for (i, img) in pictures.iter().enumerate() {
        let image = lm::measure(img);
        let text = texts.get(i).cloned().flatten();
        let ascii = text
            .as_ref()
            .map(|t| lm::measure_ascii(t, lm::ascii_ramp(&configs[i], Path::new(&args.scene))));
        let distance = reference.map(|r| {
            let ink = r
                .ink
                .as_ref()
                .and(text.as_ref())
                .map(|t| lm::InkGrid::from_text(t, &ink_scale(args, &configs[i])));
            r.distance(img, ink.as_ref())
        });
        let value = grid.map(|g| {
            let (c, r) = cells[i];
            g.describe(c, r)
        });
        say!(
            "  [{}] {}{}",
            value.as_deref().unwrap_or("frame"),
            lm::summary(&image, ascii.as_ref()),
            distance
                .as_ref()
                .map(|d| format!(
                    "  distance {:.2} by {} ({})",
                    d.score,
                    d.metric.name(),
                    d.terms_line()
                ))
                .unwrap_or_default()
        );
        columns.push(lm::ColumnReport {
            index: i,
            value,
            image,
            ascii,
            distance,
        });
    }
    columns
}

/// A sweep's labels with each column's distance to the reference, the nearest
/// starred (#357); as they were without a reference.
pub(super) fn distance_labels(
    labels: Vec<String>,
    columns: &[crate::commands::look_measure::ColumnReport],
    nearest: Option<usize>,
) -> Vec<String> {
    labels
        .into_iter()
        .enumerate()
        .map(
            |(i, l)| match columns.get(i).and_then(|c| c.distance.as_ref()) {
                Some(d) => {
                    let star = if nearest == Some(i) { " *" } else { "" };
                    format!("{l}  d {:.1}{star}", d.score)
                }
                None => l,
            },
        )
        .collect()
}

/// `--report FILE` (#357): the measures as JSON.
pub(super) fn write_measures(
    args: &RenderArgs,
    path: &str,
    look: Option<&crate::commands::look::Look>,
    grid: Option<&crate::commands::sweep::SweepGrid>,
    (width, height): (u32, u32),
    columns: Vec<crate::commands::look_measure::ColumnReport>,
    nearest: Option<usize>,
) -> Result<()> {
    let report = crate::commands::look_measure::RenderReport {
        scene: args.scene.clone(),
        look: look.map(|l| l.name.clone()),
        sweep: grid.map(|g| g.name()),
        reference: args.measure.reference.clone(),
        width,
        height,
        thresholds: crate::commands::look_measure::Thresholds::USED,
        columns,
        nearest,
    };
    std::fs::write(path, serde_json::to_string_pretty(&report)?)
        .context(format!("Failed to write {path}"))?;
    say!("Wrote measures to {path}");
    Ok(())
}
