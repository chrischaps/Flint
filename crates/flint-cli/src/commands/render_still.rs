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
    /// The packages the project's `[packages]` lets in (#1623: a left-out
    /// package's scene binder builds nothing).
    pub(super) packages: Option<flint_core::package::InUse>,
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
        packages,
    } = still;
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
    bind_capture_scene(
        args,
        ctx,
        &mut renderer,
        &world,
        &scene_file,
        binding,
        packages.as_ref(),
    );

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
    let mut animation = pose_animation(args, &models, &config.project, &mut world, ANIM_DT, true)?;
    match &animation {
        Some(posed) => posed.upload(&mut renderer, &ctx.device, &ctx.queue),
        None => pose_still(&mut world),
    }

    if let Some(name) = &args.camera.select {
        let id = world
            .get_id(name)
            .with_context(|| format!("--select: no entity named '{name}'"))?;
        renderer.set_selected_entity(Some(id));
    }

    // Particles (ADR 0068): fixed-step, fixed-seed simulation so a snapshot
    // at a given --particle-time is reproducible run to run.
    // The particles package's system (#1622): `render` refused
    // --particle-time already in a flint without it
    let mut particle_sim: Option<Box<dyn flint_render::ParticleSource>> = None;
    if let (Some(seconds), Some(point)) = (args.clock.particle_time, particles()) {
        let mut particles = (point.new)();
        let project = &config.project;
        particles
            .load(project, &args.scene, &world)
            .context("Failed to initialize particles")?;
        flint_render::load_scene_particle_textures(
            &mut renderer,
            &ctx.device,
            &ctx.queue,
            &particles.texture_names(),
            project,
            Path::new(&args.scene).parent().unwrap_or(Path::new(".")),
        );
        particles.simulate_to(&world, seconds.max(0.0), 1.0 / 60.0);
        particles.pack(Some(camera.position_array()));
        say!(
            "Particles: {} alive across {} emitter(s) at t = {:.2}s",
            particles.total_alive(),
            particles.emitter_count(),
            seconds
        );
        particle_sim = Some(particles);
    }

    // The frame's inputs, and the world read into the draw lists (#739)
    let particle_draws = particle_sim.as_ref().map(|p| p.draw_data());
    renderer.prepare(
        &ctx.device,
        &ctx.queue,
        &world,
        &FrameInputs {
            particles: particle_draws.as_deref(),
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
                particles: particle_sim.as_deref_mut(),
                animation: animation.as_deref_mut(),
                meshes: &meshes,
                scene_name: &scene_file.scene.name,
            },
            frames,
        );
    }

    // The scene's own block is what --ai-refine lays a candidate over
    if let Some(refine) = refines(args) {
        let look = look.expect("clap requires --look with --ai-refine");
        let reference = reference.expect("clap requires --reference with --ai-refine");
        return refine_look(args, refine, look, reference, &mut |table| {
            let def = crate::commands::look::overlay(scene_pp, table)?;
            let config = post_process_config(args, Some(&def), renderer.styles())?;
            let scale = reads_glyphs.then(|| ink_scale(args, &config));
            let image = draw_frame(ctx, &mut renderer, &camera, Some(config), Some(&world))?;
            let ink = scale.and_then(|s| frame_ink(&renderer, ctx, &s));
            Ok(Frame { image, ink })
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
                                particles: particle_draws.as_deref(),
                                ..still_inputs(args, &camera, time)
                            },
                            &world,
                        ),
                        particles: particle_sim.as_deref(),
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
pub(super) fn frame_ink(
    renderer: &SceneRenderer,
    ctx: &HeadlessContext,
    scale: &crate::commands::look_measure::InkScale,
) -> Option<crate::commands::look_measure::InkGrid> {
    renderer
        .style_text(&ctx.device, "ascii")
        .map(|t| crate::commands::look_measure::InkGrid::from_text(&t, scale))
}

/// One candidate block drawn by a render path: its picture, and its glyph
/// grid when the ASCII style drew (read only when the reference has one).
pub(super) struct Frame {
    pub(super) image: image::RgbaImage,
    pub(super) ink: Option<crate::commands::look_measure::InkGrid>,
}

/// `--ai-refine` (#357): the package's refine ([`refines`]) over this
/// render, which lends it a [`RefineHost`] that draws with `draw` and
/// measures against `reference` (#1572: the loop is the ai package's).
pub(super) fn refine_look(
    args: &RenderArgs,
    refine: &dyn flint_package::cli::look::LookRefine,
    look: crate::commands::look::Look,
    reference: crate::commands::look_measure::Reference,
    draw: &mut dyn FnMut(&toml::Table) -> Result<Frame>,
) -> Result<()> {
    let mut host = LookHost {
        args,
        look,
        reference,
        draw,
    };
    refine.run(args.packages.matches(), &mut host)
}

/// What a render lends a package's refine: the look, the reference, a draw
/// of the scene under a candidate block, and what `--sweep` and the
/// measures know.
struct LookHost<'a> {
    args: &'a RenderArgs,
    look: crate::commands::look::Look,
    reference: crate::commands::look_measure::Reference,
    draw: &'a mut dyn FnMut(&toml::Table) -> Result<Frame>,
}

impl flint_package::cli::look::RefineHost for LookHost<'_> {
    fn look(&self) -> &crate::commands::look::Look {
        &self.look
    }

    fn reference_path(&self) -> &str {
        self.args.measure.reference.as_deref().unwrap_or_default()
    }

    fn reference(&self) -> &image::RgbImage {
        &self.reference.image
    }

    fn reference_ink(&self) -> Option<String> {
        self.reference.ink.as_ref().map(|g| g.summary())
    }

    fn output(&self) -> &str {
        &self.args.output
    }

    fn draw(&mut self, post_process: &toml::Table) -> Result<flint_package::cli::look::Drawn> {
        let Frame { image, ink } = (self.draw)(post_process)?;
        let distance = self.reference.distance(&image, ink.as_ref());
        Ok(flint_package::cli::look::Drawn {
            image,
            distance,
            ink: ink.map(|g| g.summary()),
        })
    }

    fn measure(&self, image: &image::RgbaImage) -> crate::commands::look_measure::ImageMeasures {
        crate::commands::look_measure::measure(image)
    }

    fn summary(&self, measures: &crate::commands::look_measure::ImageMeasures) -> String {
        crate::commands::look_measure::summary(measures, None)
    }

    fn apply_edit(&self, table: &mut toml::Table, key: &str, value: &str) -> Result<(), String> {
        look_edit(table, key, value)
    }

    fn tune(&self, sweep: &str) -> Option<flint_package::cli::look::TuneSweep> {
        let sweep = crate::commands::sweep::Sweep::parse(sweep).ok()?;
        Some(flint_package::cli::look::TuneSweep {
            key: sweep.name(),
            values: sweep.values,
        })
    }

    fn current(&self, table: &toml::Table, key: &str) -> Option<toml::Value> {
        // A key as `Sweep::name` spells it, read by its family's spelling:
        // `post.<field>`, `<style>.<key>`, or `look` and a scene's keys,
        // which no block holds
        use crate::commands::sweep::Family;
        match Family::of(key) {
            Family::Post(field) => table.get(field).cloned(),
            Family::Style(style, key) => table.get("styles")?.get(style)?.get(key).cloned(),
            _ => None,
        }
    }

    fn value(&self, text: &str) -> toml::Value {
        crate::commands::sweep::parse_value(text)
    }

    fn sheet(
        &self,
        cells: &[image::RgbaImage],
        title: String,
        subtitle: String,
        labels: Vec<String>,
    ) -> image::RgbaImage {
        crate::commands::style_sheet::compose(
            cells,
            &crate::commands::style_sheet::SheetText::new(title, subtitle, labels),
        )
    }
}

/// Check `key=value` the way `--sweep` checks a column (key known, value in
/// range) and put it into `table`, a look's `[post_process]`.
fn look_edit(table: &mut toml::Table, key: &str, value: &str) -> Result<(), String> {
    use crate::commands::sweep::{parse_value, Sweep, SweepKey};
    // The sweep parser knows every key and range; one edit is a sweep of a
    // single value, written twice because a sweep takes two.
    let sweep = Sweep::parse(&format!("{key}={value}|{value}"))?;
    let parsed = parse_value(value);
    match sweep.key {
        SweepKey::Look => return Err("`look` can't be an edit; edit its keys".into()),
        SweepKey::Wooze { .. } | SweepKey::Echoes { .. } | SweepKey::Entity(_) => {
            return Err(
                "wooze knobs, [living] echoes and an entity's fields belong to the scene, \
                 not a look"
                    .into(),
            )
        }
        SweepKey::Post { field } => {
            table.insert(field, parsed);
        }
        SweepKey::Style { style, key: name } => {
            let styles = table
                .entry("styles")
                .or_insert_with(|| toml::Value::Table(Default::default()));
            let Some(styles) = styles.as_table_mut() else {
                return Err("[post_process] styles is not a table".into());
            };
            let t = styles
                .entry(style)
                .or_insert_with(|| toml::Value::Table(Default::default()));
            let Some(t) = t.as_table_mut() else {
                return Err(format!("[post_process.styles] {key} is not a table"));
            };
            t.insert(name, parsed);
        }
    }
    crate::commands::look::overlay(None, table).map_err(|e| format!("{e:#}"))?;
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

/// The render's half of `--ai-refine` (#1572): what [`LookHost`] lends the
/// ai package's loop, held with the loop itself running over it.
#[cfg(test)]
mod refine_tests {
    use super::*;
    use crate::commands::look::{self, Look};
    use crate::commands::look_measure::{InkGrid, Metric, Reference};
    use flint_asset_gen::cli::look_refine::{
        refined_look_toml, run, Ask, Candidate, Edit, LookAgent, Options, Tried, TuneWalker,
    };
    use flint_package::cli::look::RefineHost;
    use flint_procgen_ai::cost::CostMeter;
    use image::{RgbImage, RgbaImage};

    fn noir() -> Look {
        look::find("ascii_noir", "").unwrap()
    }

    fn args() -> RenderArgs {
        use clap::Parser as _;
        let cli = crate::Cli::parse_from(["flint", "render", "x.scene.toml", "-o", "out/s.png"]);
        let crate::Commands::Render(args) = cli.command else {
            unreachable!()
        };
        *args
    }

    fn host<'a>(
        args: &'a RenderArgs,
        look: Look,
        reference: Reference,
        draw: &'a mut dyn FnMut(&toml::Table) -> Result<Frame>,
    ) -> LookHost<'a> {
        LookHost {
            args,
            look,
            reference,
            draw,
        }
    }

    fn grey(image: RgbImage) -> Reference {
        Reference { image, ink: None }
    }

    #[test]
    fn an_edit_lands_in_the_block_and_is_checked() {
        let mut t = noir().post_process;
        look_edit(&mut t, "ascii.brightness", "0.3").unwrap();
        assert_eq!(t["styles"]["ascii"]["brightness"].as_float(), Some(0.3));
        look_edit(&mut t, "post.fog_color", "0.1,0.2,0.3").unwrap();
        assert!(t["fog_color"].is_array());
        assert!(look_edit(&mut t, "ascii.cell", "900").is_err());
        assert!(look_edit(&mut t, "post.exposur", "1")
            .unwrap_err()
            .contains("exposure"));
        assert!(look_edit(&mut t, "wooze.glide", "0.5").is_err());
    }

    #[test]
    fn the_walker_skips_current_and_tried_values() {
        let args = args();
        let mut draw = |_: &toml::Table| -> Result<Frame> { unreachable!() };
        let h = host(&args, noir(), grey(RgbImage::new(1, 1)), &mut draw);
        let look = noir();
        let m = h.summary(&h.measure(&RgbaImage::new(1, 1)));
        let flat = RgbImage::new(1, 1);
        let d = crate::commands::look_measure::distance_report(
            flint_import::measure::distance_rgb(&flat, &flat),
            None,
        );
        let ask = |history: &[Tried]| {
            TuneWalker
                .propose(&Ask {
                    round: 1,
                    look: &look,
                    current: &look.post_process,
                    current_png: &[],
                    current_distance: &d,
                    current_measures: &m,
                    current_ink: None,
                    reference_png: &[],
                    reference_measures: &m,
                    reference_ink: None,
                    history,
                    want: 3,
                    host: &h,
                })
                .unwrap()
        };
        let first = ask(&[]);
        let labels: Vec<String> = first.iter().map(Candidate::label).collect();
        // contrast 1.6 and brightness 0.15 are the look's own values
        assert_eq!(
            labels,
            ["ascii.contrast=1.2", "ascii.cell=7", "ascii.brightness=0"]
        );
        let history: Vec<Tried> = first
            .into_iter()
            .map(|candidate| Tried {
                round: 1,
                candidate,
                distance: None,
                refused: None,
                image: None,
                kept: false,
            })
            .collect();
        let next: Vec<String> = ask(&history).iter().map(Candidate::label).collect();
        // cell 9 is the look's own 9.0
        assert_eq!(
            next,
            [
                "ascii.contrast=2.2",
                "ascii.cell=12",
                "ascii.brightness=0.3"
            ]
        );
    }

    /// #495: a halo-like key that moves only pixels wins on pixels, and
    /// loses to a key that moves the letters once the reference has a grid.
    /// The fake renderer's grey is the exposure; its glyphs' ink is twice
    /// the ASCII brightness. The reference is grey 150 in ink 0.9: each key
    /// matches one half of it.
    #[test]
    fn with_a_reference_grid_the_letters_win_over_the_pixels() {
        let look = Look::parse(
            "t",
            "[look]\ndescription = \"test\"\n\
             tune = [\"post.exposure=0.5,1.5\", \"ascii.brightness=0,0.45\"]\n\
             [post_process]\nexposure = 0.5\n\
             [post_process.styles.ascii]\nbrightness = 0.0",
            "test",
        )
        .unwrap();
        let filled = |ink: f32| InkGrid {
            ink: vec![ink; 8 * 4],
            ..InkGrid::blank(8, 4)
        };
        let args = args();
        let kept = |ink: Option<InkGrid>| {
            let mut draw = |t: &toml::Table| -> Result<Frame> {
                let e = t.get("exposure").and_then(|v| v.as_float()).unwrap_or(1.0);
                let b = t["styles"]["ascii"]["brightness"].as_float().unwrap_or(0.0);
                let g = (e * 100.0).clamp(0.0, 255.0) as u8;
                Ok(Frame {
                    image: RgbaImage::from_pixel(16, 16, image::Rgba([g, g, g, 255])),
                    ink: Some(filled((b * 2.0) as f32)),
                })
            };
            let reference = Reference {
                image: RgbImage::from_pixel(16, 16, image::Rgb([150, 150, 150])),
                ink,
            };
            let mut h = host(&args, look.clone(), reference, &mut draw);
            let log = run(
                Options {
                    rounds: 1,
                    candidates: 2,
                },
                &TuneWalker,
                &CostMeter::new("mock"),
                &mut h,
            )
            .unwrap();
            assert_eq!(log.tried.len(), 2, "both keys tried");
            (
                log.metric,
                log.kept.iter().map(Edit::to_string).collect::<Vec<_>>(),
            )
        };
        assert_eq!(
            kept(None),
            (Metric::Pixels, vec!["post.exposure=1.5".to_string()])
        );
        assert_eq!(
            kept(Some(filled(0.9))),
            (Metric::Glyphs, vec!["ascii.brightness=0.45".to_string()])
        );
    }

    /// A fake renderer: the picture's grey level is the look's exposure,
    /// so the loop's job is to walk exposure toward the reference's grey.
    #[test]
    fn the_loop_keeps_what_moves_the_distance_down_and_stops() {
        let look = Look::parse(
            "t",
            "[look]\ndescription = \"test\"\ntune = [\"post.exposure=0.5,1,1.5,2\"]\n\
             [post_process]\nexposure = 0.5",
            "test",
        )
        .unwrap();
        let mut draw = |t: &toml::Table| -> Result<Frame> {
            let e = t.get("exposure").and_then(|v| v.as_float()).unwrap_or(1.0);
            let g = (e * 100.0).clamp(0.0, 255.0) as u8;
            Ok(Frame {
                image: RgbaImage::from_pixel(16, 16, image::Rgba([g, g, g, 255])),
                ink: None,
            })
        };
        let args = args();
        let reference = grey(RgbImage::from_pixel(16, 16, image::Rgb([150, 150, 150])));
        let mut h = host(&args, look.clone(), reference, &mut draw);
        let meter = CostMeter::new("mock");
        let log = run(
            Options {
                rounds: 5,
                candidates: 1,
            },
            &TuneWalker,
            &meter,
            &mut h,
        )
        .unwrap();
        // 0.5 (50) -> 1 (100) -> 1.5 (150, the reference) -> 2 is further,
        // then nothing is left to try: stop
        assert_eq!(
            log.kept.iter().map(Edit::to_string).collect::<Vec<_>>(),
            ["post.exposure=1", "post.exposure=1.5"]
        );
        assert!(log.end.score < 1e-9, "{:?}", log.end);
        assert!(log.stopped.contains("round 4"), "{}", log.stopped);
        assert_eq!(log.frames.len(), 4, "reference, start and two kept rounds");
        let text = refined_look_toml(&log, &look, "t_refined").unwrap();
        let back = Look::parse("t_refined", &text, "test").unwrap();
        assert_eq!(back.post_process["exposure"].as_float(), Some(1.5));
        assert!(back
            .notes
            .contains("kept post.exposure=1, post.exposure=1.5"));
    }
}
