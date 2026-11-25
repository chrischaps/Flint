//! The clocked path (#216, #487, #1145): a [`Stage`] a sweep cell, captured
//! at every moment asked for, and the sheets and strips made of them.

use super::*;

/// What `run_in` hands the stage path, worked out already: the schemas, the
/// scene as it loaded it (the first cell's world), the sweep and every cell's
/// post-processing, and what the one-renderer path would have drawn with.
pub(super) struct StagedRender<'a> {
    pub(super) registry: &'a SchemaRegistry,
    /// The scene as each stage's renderer binds it (#739).
    pub(super) binding: &'a flint_render::SceneBinding,
    pub(super) first: (flint_ecs::FlintWorld, flint_scene::SceneFile),
    pub(super) grid: Option<&'a crate::commands::sweep::SweepGrid>,
    /// A cell's each, a row at a time, as [`crate::commands::sweep::SweepGrid::cells`].
    pub(super) configs: Vec<flint_render::PostProcessConfig>,
    pub(super) report_json: bool,
    /// What a sweep's cell is of its frame (#358).
    pub(super) view: crate::commands::sweep::CellView,
    /// The scene's `[post_process]`, under its own look, for `--ai-refine`.
    pub(super) scene_pp: Option<&'a flint_scene::PostProcessDef>,
    pub(super) look: Option<crate::commands::look::Look>,
    pub(super) reference: Option<crate::commands::look_measure::Reference>,
    pub(super) camera_path: Option<&'a crate::commands::frame_seq::CameraPath>,
}

/// Whether game time runs through the captures: a `--time`, or `--frames`
/// `--frame-dt` apart (#596).
pub(super) fn on_clock(args: &RenderArgs) -> bool {
    args.clock.time.is_some() || args.seq.frames.is_some()
}

/// A render with clocks (#216): a [`Stage`] a sweep cell, each captured at
/// every moment asked for, the cells composed into one sheet a moment (a row,
/// or a grid for two sweeps), and a span written as a GIF or a PNG a frame.
pub(super) fn run_stages(
    args: &RenderArgs,
    host: &mut RenderHost,
    staged: StagedRender,
) -> Result<()> {
    use std::hash::{Hash, Hasher};
    let StagedRender {
        registry,
        binding,
        first,
        grid,
        configs,
        report_json,
        view,
        scene_pp,
        look,
        reference,
        camera_path,
    } = staged;

    // A span or a list as one contact sheet (#574): neither a GIF nor stills
    let strip = args.strip.strip;
    let animated = !strip && matches!(args.clock.time, Some(TimeSpec::Range(..)));
    // A still a moment (#614), numbered in the order the moments were given.
    let moments = !strip && matches!(args.clock.time, Some(TimeSpec::List(_)));
    let report_file = args.measure.report.as_deref().filter(|_| !report_json);
    let measured = reference.is_some() || report_file.is_some();
    // What a clocked render can't mean (#596), each refused with why
    if measured && (animated || moments || strip) {
        anyhow::bail!(
            "--report FILE / --reference measure one moment, and this --time captures \
             several: give --time T (--report json reports every capture's props)"
        );
    }
    if args.refine.ai_refine && (animated || moments) {
        anyhow::bail!("--ai-refine refines a look at one moment: give --time T");
    }
    if args.refine.ai_refine && report_json {
        anyhow::bail!("--ai-refine writes its own log beside the refined look: drop --report json");
    }
    let wants_text = args.out.ascii_text.is_some() || args.out.ascii_text_ansi.is_some();
    let wants_flips = args.seq.flip_stats || args.seq.flip_json.is_some();
    let reads_glyphs = reference.as_ref().is_some_and(|r| r.ink.is_some());

    let cells = grid.map_or(vec![(0, 0)], |g| g.cells());
    let count = cells.len();

    // The scene, once for each cell: a world is changed by loading it
    let mut scenes = Vec::with_capacity(count);
    scenes.push(first);
    for _ in 1..count {
        scenes.push(load_settled(&args.scene, registry)?);
    }
    // An echoes column is its scene with the column's [living] echoes (#531),
    // written before anything reads the table
    if let Some(grid) = grid.filter(|g| g.is_living()) {
        for ((_, scene_file), &(column, row)) in scenes.iter_mut().zip(&cells) {
            grid.lay_living(column, row, &mut scene_file.tables);
        }
    }
    let scene_name = scenes[0].1.scene.name.clone();

    let (width, height) = match grid {
        // Cells drawn small, or at the full size to be cropped or shrunk
        Some(_) => view.render_size(args.width, args.height),
        // A refine's rounds are sheet cells
        // A refine's rounds are sheet cells, and so are a strip's tiles
        None if args.refine.ai_refine || strip => {
            let (w, h) = crate::commands::sweep::CELL_SIZE;
            (args.width.unwrap_or(w), args.height.unwrap_or(h))
        }
        None => {
            let (w, h) = render_plan::default_size(false, animated);
            (args.width.unwrap_or(w), args.height.unwrap_or(h))
        }
    };
    let frame_step = args
        .clock
        .frame_step
        .unwrap_or(render_plan::DEFAULT_FRAME_STEP);

    let device = load_report::span(load_report::Stage::Device, String::new);
    let ctx = host.context(width, height, &args.debug.gpu)?;
    drop(device);

    let timeline = match &args.clock.expression_timeline {
        Some(path) => Some(ExpressionTimeline::load(path)?),
        None => None,
    };
    if let Some(timeline) = &timeline {
        say!("Expression timeline: {} states", timeline.len());
    }

    let mut stages = Vec::with_capacity(count);
    for (index, (((world, scene_file), config), &(column, row))) in
        scenes.into_iter().zip(configs).zip(&cells).enumerate()
    {
        stages.push(Stage::load(
            args,
            ctx,
            StageScene {
                world,
                scene_file,
                binding,
                index,
                cell: (column, row),
                grid,
                config,
                look: stage_look(args, grid, (column, row), scene_pp),
                timeline: timeline.clone(),
                resolve_procgen: false,
            },
        )?);
    }

    // The load is up; its last stage is the first capture (#1092)
    load_report::bring_up_done();
    let mut load = None;

    // --ai-refine (#357) at a moment: each candidate is the one stage redrawn
    // at that moment under the candidate's look, its clocks never rewound
    #[cfg(feature = "ai")]
    if args.refine.ai_refine {
        let look = look.expect("clap requires --look with --ai-refine");
        let reference = reference.expect("clap requires --reference with --ai-refine");
        let t = match args.clock.time {
            Some(TimeSpec::At(t)) => t,
            _ => 0.0,
        };
        let stage = &mut stages[0];
        return refine_look(args, look, reference, &mut |table| {
            let def = crate::commands::look::overlay(scene_pp, table)?;
            let config = post_process_config(args, Some(&def), stage.renderer.styles())?;
            let scale = reads_glyphs.then(|| ink_scale(args, &config));
            let image = stage.redraw(args, ctx, t, frame_step, config)?;
            let ink = scale.and_then(|s| frame_ink(&stage.renderer, ctx, &s));
            Ok(crate::commands::look_refine::Frame { image, ink })
        });
    }

    // --frames (#345) on the clock: a frame every --frame-dt of game time
    let frame_dt = args.seq.frame_dt.map_or(1.0 / 60.0, f64::from);
    let times = match args.seq.frames {
        Some(frames) => (0..frames).map(|i| f64::from(i) * frame_dt).collect(),
        None if strip => render_plan::strip_times(args.clock.time.as_ref(), args.strip.every)?,
        None => render_plan::capture_times(args.clock.time.as_ref(), args.clock.fps),
    };
    if let Some(frames) = args.seq.frames {
        let motion = match (args.seq.orbit, camera_path) {
            (Some(rate), _) => format!(", orbiting {rate} deg/s"),
            (None, Some(path)) => format!(", on a {}-key camera path", path.key.len()),
            _ => String::new(),
        };
        say!("Sequence: {frames} frames, {frame_dt:.6} s apart{motion}");
    }
    let yaw0: Vec<f32> = stages.iter().map(|stage| stage.camera.yaw).collect();

    let mut warnings = stage_warnings(args, grid, &stages[0]);
    for warning in &warnings {
        eprintln!("Warning: {warning}");
    }

    let output = Path::new(&args.output);
    // Where each frame goes as it's captured, so a span costs memory for one
    // frame however long it runs (#588).
    let sequence = animated && render_plan::is_sequence(&args.output);
    let mut gif = None;
    let mut still = None;
    // A strip's tiles, composed once the last is captured
    let mut tiles = Vec::new();
    let mut written_bytes = 0u64;
    let (mut frame_width, mut frame_height) = (width, height);
    let mut captures = Vec::new();
    let mut flips = crate::commands::frame_seq::FlipCounter::default();
    // Each cell's frames so far, hashed: neighbours that drew the same at every
    // moment showed nothing
    let mut drawn: Vec<std::collections::hash_map::DefaultHasher> =
        (0..count).map(|_| Default::default()).collect();
    // A moment's measures; there is one moment when there are any (#357)
    let mut columns = Vec::new();
    // A grid's text, a frame's each when there are several (#317)
    let text_path = |path: &Option<String>, index: usize| {
        path.as_ref().map(|p| match times.len() {
            1 => p.clone(),
            n => crate::commands::frame_seq::numbered(p, index as u32, n as u32)
                .to_string_lossy()
                .into_owned(),
        })
    };
    for (index, &t) in times.iter().enumerate() {
        let mut pictures = Vec::with_capacity(count);
        let mut texts = Vec::with_capacity(count);
        for (i, stage) in stages.iter_mut().enumerate() {
            stage.advance(ctx, t, frame_step);
            // A strip's tile is --time t alone: a first frame, whatever came before
            if strip {
                stage.forget_history();
            }
            // The camera's move, over where the scripts left it
            if args.seq.frames.is_some() {
                if let Some(rate) = args.seq.orbit {
                    stage.camera.yaw = yaw0[i] + (rate * t as f32).to_radians();
                    stage.camera.update_orbit();
                } else if let Some(path) = camera_path {
                    let (position, target) = path.at(t as f32);
                    stage.camera.position = position;
                    stage.camera.target = target;
                }
            }
            // The frame's description (#1084) and capture (#1085): the
            // first cell's last capture
            let dump = crate::commands::render_frame_dump::Wanted::of(args)
                .filter(|_| i == 0 && index + 1 == times.len())
                .map(|wanted| {
                    (
                        wanted,
                        crate::commands::render_frame_dump::arm(ctx, &mut stage.renderer, wanted),
                    )
                });
            pictures.push(stage.capture(args, ctx, t, frame_step)?);
            load = load.or_else(first_capture_done);
            if let Some((wanted, armed)) = dump {
                let said = crate::commands::render_frame_dump::write(
                    armed,
                    ctx,
                    &mut stage.renderer,
                    Some(stage.game.world()),
                    wanted,
                    crate::commands::render_frame_dump::Origin {
                        scene: &args.scene,
                        game_time: Some(t),
                        written_by: "flint render",
                        notes: Vec::new(),
                    },
                    stage.replay_inputs.take().map(|inputs| {
                        crate::commands::render_frame_dump::ReplayFacts {
                            inputs,
                            particles: Some(&stage.game.particles().sync),
                        }
                    }),
                )?;
                for line in said {
                    say!("{line}");
                }
            }
            if wants_text || wants_flips || report_file.is_some() || reads_glyphs {
                texts.push(stage.renderer.style_text(&ctx.device, "ascii"));
            }
            if report_json {
                let (column, row) = cells[i];
                captures.push(CaptureReport {
                    time: t,
                    column: grid.map(|g| g.columns.values[column].clone()),
                    row: grid
                        .and_then(|g| g.rows.as_ref())
                        .map(|rows| rows.values[row].clone()),
                    distance: None,
                    expression: stage.scripted.then_some(()).and_then(|()| {
                        let state = stage.game.expression().get("state")?.clone();
                        state.into_string().ok()
                    }),
                    props: render_report::of_game::props(&stage.game),
                });
            }
        }
        for (hasher, picture) in drawn.iter_mut().zip(&pictures) {
            picture.as_raw().hash(hasher);
        }

        // The ASCII grid as text, and its flips frame to frame. A sweep is
        // refused text before anything loads, so this is the one stage's
        if wants_flips {
            let Some(text) = &texts[0] else {
                anyhow::bail!(
                    "--flip-stats: the ASCII style didn't draw frame {index}; turn it on with \
                     --style ascii, a look, or the scene's [post_process.styles.ascii]"
                );
            };
            let f = flips.push(text);
            if args.seq.flip_stats {
                match (f.flips, f.reversals) {
                    (Some(n), Some(r)) => say!("  frame {index:>4}: {n:>6} flips {r:>6} reversals"),
                    (Some(n), None) => say!("  frame {index:>4}: {n:>6} flips"),
                    _ => {}
                }
            }
        }
        if wants_text {
            write_ascii_text(
                text_path(&args.out.ascii_text, index).as_deref(),
                text_path(&args.out.ascii_text_ansi, index).as_deref(),
                texts[0].clone(),
            )?;
        }

        // Each cell as the sheet shows it (cropped or shrunk, #358), and measured
        // from those pixels as the one-renderer path measures
        // A crop's first frame stays whole for the sheet's locator (#501)
        let mut locator = None;
        if grid.is_some() {
            locator = pictures.first().and_then(|p| view.locator(p));
            pictures = pictures.into_iter().map(|p| view.cell(p)).collect();
        }
        if measured {
            let used: Vec<_> = stages
                .iter()
                .map(|stage| stage.renderer.post_process_config().clone())
                .collect();
            columns = measure_cells(
                args,
                &pictures,
                &texts,
                &used,
                reference.as_ref(),
                grid,
                &cells,
            );
            let first = captures.len().saturating_sub(count);
            for (capture, column) in captures[first..].iter_mut().zip(&columns) {
                capture.distance = column.distance.as_ref().map(|d| d.score);
            }
        }
        let nearest = crate::commands::look_measure::RenderReport::nearest_column(&columns);

        let frame = match grid {
            Some(grid) => stage_sheet(
                grid,
                StageSheet {
                    view,
                    width,
                    height,
                    scene_name: &scene_name,
                    t,
                    reference: args.measure.reference.as_deref(),
                    columns: &columns,
                    nearest,
                    locator,
                },
                &stages,
                &pictures,
            ),
            None => pictures.remove(0),
        };
        (frame_width, frame_height) = frame.dimensions();
        if let Some(frames) = args.seq.frames {
            if args.seq.last_frame {
                // #478: only where the sequence ends, to --output itself
                if index as u32 + 1 == frames {
                    if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
                        std::fs::create_dir_all(parent)
                            .with_context(|| format!("failed to create {}", parent.display()))?;
                    }
                    frame
                        .save(output)
                        .with_context(|| format!("Failed to save image to {}", args.output))?;
                }
            } else if !args.seq.no_images {
                let path = crate::commands::frame_seq::numbered(&args.output, index as u32, frames);
                if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                    std::fs::create_dir_all(parent)
                        .with_context(|| format!("failed to create {}", parent.display()))?;
                }
                frame
                    .save(&path)
                    .with_context(|| format!("Failed to save image to {}", path.display()))?;
            }
        } else if sequence {
            let path = render_plan::sequence_path(&args.output, index);
            if let Some(parent) = Path::new(&path)
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
            {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create {}", parent.display()))?;
            }
            frame
                .save(&path)
                .with_context(|| format!("Failed to save frame {index} to {path}"))?;
            written_bytes += std::fs::metadata(&path).map_or(0, |m| m.len());
        } else if strip {
            tiles.push(frame);
        } else if moments {
            let path = render_plan::numbered_still(&args.output, index, times.len());
            frame
                .save(&path)
                .context(format!("Failed to save image to {}", path.display()))?;
        } else if animated {
            let frame = image::DynamicImage::ImageRgba8(frame).into_rgb8();
            let spool = match &mut gif {
                Some(spool) => spool,
                None => gif.insert(gif_writer::Spool::new(
                    output,
                    frame.width(),
                    frame.height(),
                    times.len(),
                )?),
            };
            spool.push(&frame)?;
        } else {
            still = Some(frame);
        }
    }

    // A span's clocks run on to its end (the end itself is never a frame, so that
    // the GIF loops), so what the scripts log covers all of it.
    if let Some(TimeSpec::Range(_, end)) = args.clock.time {
        for stage in &mut stages {
            stage.advance(ctx, end, frame_step);
        }
    }

    // What the scripts asked for that isn't in the picture, said once as it
    // happened, and kept for the report.
    if stages[0].scripted {
        warnings.extend(stages[0].game.notes());
    }

    if let Some(grid) = grid {
        let drawn: Vec<u64> = drawn.iter().map(Hasher::finish).collect();
        for warning in grid.identical_warnings(&drawn) {
            eprintln!("Warning: {warning}");
            warnings.push(warning);
        }
    }

    // Write the file
    if let Some(frames) = args.seq.frames {
        if args.seq.last_frame {
            say!(
                "Rendered frame {} of {frames} ({frame_width}x{frame_height}) to {}",
                frames - 1,
                args.output
            );
        } else if !args.seq.no_images {
            say!(
                "Rendered {frames} {frame_width}x{frame_height} frames to {} .. {}",
                crate::commands::frame_seq::numbered(&args.output, 0, frames).display(),
                crate::commands::frame_seq::numbered(&args.output, frames - 1, frames).display()
            );
        }
    } else if sequence {
        say!(
            "Rendered {} frames of {frame_width}x{frame_height} at {} fps to {} ({:.1} MB)",
            times.len(),
            args.clock.fps,
            args.output,
            written_bytes as f64 / 1e6,
        );
    } else if strip {
        let sheet = strip_sheet(args, &scene_name, &times, &tiles, frame_step);
        if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        sheet
            .save(output)
            .with_context(|| format!("Failed to save the strip to {}", args.output))?;
        say!(
            "Rendered {} tiles of {frame_width}x{frame_height} ({} .. {}) as one {}x{} strip to {}",
            tiles.len(),
            render_plan::strip_label(times[0]),
            render_plan::strip_label(times[times.len() - 1]),
            sheet.width(),
            sheet.height(),
            args.output
        );
    } else if animated {
        let spool = gif.context("a span captured no frames")?;
        let (frames, spilled) = (spool.frames(), spool.spilled());
        spool.finish(args.clock.fps)?;
        say!(
            "Rendered {frames} frames of {frame_width}x{frame_height} at {} fps to {} \
             ({:.1} MB; {:.1} MB waited on disk)",
            args.clock.fps,
            args.output,
            std::fs::metadata(output).map_or(0.0, |m| m.len() as f64 / 1e6),
            spilled as f64 / 1e6,
        );
    } else if moments {
        let count = times.len();
        say!(
            "Rendered {count} moments ({}) as {frame_width}x{frame_height} to {} .. {}",
            times
                .iter()
                .map(|t| format!("{t}"))
                .collect::<Vec<_>>()
                .join(", "),
            render_plan::numbered_still(&args.output, 0, count).display(),
            render_plan::numbered_still(&args.output, count - 1, count).display()
        );
    } else {
        let still = still.context("the render captured no frame")?;
        still
            .save(&args.output)
            .context(format!("Failed to save image to {}", args.output))?;
        match grid {
            Some(grid) => {
                let (across, down) = grid.shape();
                let shape = match grid.rows {
                    Some(_) => format!("{across}x{down} cells"),
                    None => format!("{across} columns"),
                };
                say!(
                    "Rendered {shape} of {width}x{height} as {frame_width}x{frame_height} to {}",
                    args.output
                )
            }
            None => say!("Rendered {width}x{height} image to {}", args.output),
        }
    }

    if wants_flips {
        let stats = flips.stats();
        say!("Flips: {}", stats.summary());
        if let Some(path) = &args.seq.flip_json {
            std::fs::write(path, serde_json::to_string_pretty(&stats)?)
                .context(format!("Failed to write {path}"))?;
            say!("Wrote flip counts to {path}");
        }
    }

    if let Some(path) = report_file {
        let nearest = crate::commands::look_measure::RenderReport::nearest_column(&columns);
        write_measures(
            args,
            path,
            look.as_ref(),
            grid,
            (width, height),
            columns,
            nearest,
        )?;
    }

    // GPU timing (#596): the first cell's last capture, drawn again and again
    if let Some(frames) = args.measure.gpu_time {
        let stage = &mut stages[0];
        say!(
            "GPU time is of the first cell's capture at t = {:.3} s",
            stage.captured.unwrap_or(0.0)
        );
        time_gpu(
            ctx,
            &mut stage.renderer,
            &stage.camera,
            frames,
            args.measure.gpu_time_json.as_deref(),
        )?;
    }

    // Every script error of the run, each sweep cell's game folded in
    // (#693): once each on stderr, in the report, and a failure under
    // --strict-scripts
    let mut script_errors = flint_script::ScriptErrors::new();
    for stage in &stages {
        script_errors.merge(stage.game.script_errors());
    }
    let scripted = args.clock.scripts || args.clock.expression_timeline.is_some();
    if scripted {
        for line in script_errors.summary() {
            eprintln!("{line}");
        }
    }

    if report_json {
        let zones = render_report::of_game::wooze_zones(&stages[0].game);
        let report = render_report::RenderReport {
            scene: scene_name,
            output: args.output.clone(),
            width,
            height,
            frame_step,
            wooze_field: zones.is_some(),
            wooze_zones: zones.unwrap_or(0),
            sweep: grid.map(crate::commands::sweep::SweepGrid::name),
            frame_dump: args.measure.frame_dump.clone(),
            frame_capture: args.measure.frame_capture.clone(),
            captures,
            still_props: still_props(&stages[0]),
            warnings,
            script_errors: script_errors
                .all()
                .iter()
                .map(render_report::ScriptErrorReport::from)
                .collect(),
            style_drops: flint_render::stylize::variant_table::dropped(),
            load,
        };
        let json = serde_json::to_string(&report).context("serialising the render report")?;
        crate::output::document_text(&json);
    }
    if args.clock.strict_scripts {
        if let Some(why) = script_errors.strict_failure() {
            anyhow::bail!(why);
        }
    }
    Ok(())
}

/// A `--strip`'s sheet (#574): the tiles in rows of `--columns`, under the
/// scene's name and what was asked, each labelled with its time; or with
/// `--label off` the tiles alone.
fn strip_sheet(
    args: &RenderArgs,
    scene_name: &str,
    times: &[f64],
    tiles: &[image::RgbaImage],
    frame_step: f64,
) -> image::RgbaImage {
    let columns = render_plan::strip_columns(tiles.len(), args.strip.columns);
    if args.strip.label == crate::commands::render_args::StripLabel::Off {
        return crate::commands::style_sheet::compose_plain(tiles, columns);
    }
    let asked = match &args.clock.time {
        Some(TimeSpec::Range(from, to)) => format!(
            "--time {from}..{to}  a tile every {} s",
            args.strip.every.unwrap_or(render_plan::STRIP_EVERY)
        ),
        Some(TimeSpec::List(moments)) => format!(
            "--time {}",
            moments
                .iter()
                .map(|t| t.to_string())
                .collect::<Vec<_>>()
                .join(",")
        ),
        _ => String::new(),
    };
    let (w, h) = tiles.first().map_or((0, 0), image::RgbaImage::dimensions);
    let mut subtitle = format!("{asked}  {} tiles of {w}x{h}", tiles.len());
    if args.clock.scripts {
        subtitle.push_str("  scripts on");
    }
    if let Some(look) = &args.look {
        subtitle.push_str(&format!("  look {look}"));
    }
    if args.clock.frame_step.is_some() {
        subtitle.push_str(&format!("  frame step {frame_step} s"));
    }
    let mut text = crate::commands::style_sheet::SheetText::new(
        scene_name.to_string(),
        subtitle,
        times.iter().map(|&t| render_plan::strip_label(t)).collect(),
    );
    text.per_row = columns;
    crate::commands::style_sheet::compose(tiles, &text)
}

/// What a stage sheet says beside its cells.
struct StageSheet<'a> {
    view: crate::commands::sweep::CellView,
    width: u32,
    height: u32,
    scene_name: &'a str,
    t: f64,
    reference: Option<&'a str>,
    columns: &'a [crate::commands::look_measure::ColumnReport],
    nearest: Option<usize>,
    /// Where a crop's cells sit in the frame (#501).
    locator: Option<crate::commands::style_sheet::Locator>,
}

/// One moment's sheet of a clocked sweep: a row of columns, or a grid for two
/// sweeps (#596), with the moment top right.
fn stage_sheet(
    grid: &crate::commands::sweep::SweepGrid,
    sheet: StageSheet,
    stages: &[Stage],
    pictures: &[image::RgbaImage],
) -> image::RgbaImage {
    // Only a sheet that isn't of small frames says how big they were, and only
    // one with a reference what its labels' d is
    let scale = match sheet.view {
        crate::commands::sweep::CellView::Small => String::new(),
        view => format!("  {}", view.describe(sheet.width, sheet.height)),
    };
    let reference = sheet
        .reference
        .map_or_else(String::new, |r| format!("  d = distance to {r}"));
    let label =
        |s: &crate::commands::sweep::Sweep| (0..s.values.len()).map(|i| s.label(i)).collect();
    match &grid.rows {
        None => {
            let mut text = sheet_text(&grid.columns, sheet.scene_name, sheet.t, stages);
            text.subtitle.push_str(&scale);
            text.subtitle.push_str(&reference);
            text.labels = distance_labels(text.labels, sheet.columns, sheet.nearest);
            text.locator = sheet.locator;
            crate::commands::style_sheet::compose(pictures, &text)
        }
        Some(rows) => crate::commands::style_sheet::compose_grid(
            pictures,
            &crate::commands::style_sheet::GridText {
                title: sheet.scene_name.to_string(),
                subtitle: format!(
                    "sweep {}{}{scale}{reference}",
                    grid.name(),
                    living_note(stages)
                ),
                row_key: rows.name(),
                column_labels: label(&grid.columns),
                row_labels: label(rows),
                time: format!("T {:.2}S", sheet.t),
                locator: sheet.locator,
            },
        ),
    }
}

/// Try every cell of a sweep that turns a wooze knob on a field of its own,
/// before anything is loaded. A preset may be a file beside the scene (#249),
/// which has to read. A build without the living package runs no field,
/// so it refuses the sweep, naming the package (#1333).
#[cfg(not(feature = "living"))]
pub(super) fn check_wooze_sweep(
    _: &RenderArgs,
    grid: &crate::commands::sweep::SweepGrid,
) -> Result<()> {
    Err(crate::commands::absent::missing(
        format!("flint render --sweep {}", grid.name()),
        "living",
    ))
}

#[cfg(feature = "living")]
pub(super) fn check_wooze_sweep(
    args: &RenderArgs,
    grid: &crate::commands::sweep::SweepGrid,
) -> Result<()> {
    for (column, row) in grid.cells() {
        let (_, Some(table)) = cell_knobs(args, Some(grid), (column, row))? else {
            continue;
        };
        let mut field = flint_wooze::WoozeField::new(flint_wooze::WoozeSample::NEUTRAL);
        field
            .set_global(&table, 0.0, 0.0)
            .map_err(|reason| anyhow!("--sweep {}: {reason}", grid.name()))?;
    }
    Ok(())
}

/// Whether a render is of a moment of the game (#216): a time, frames of it, or
/// a sweep of the field. It hides the editor's grid and draws shallow wooze.
pub(super) fn is_moment(
    args: &RenderArgs,
    grid: Option<&crate::commands::sweep::SweepGrid>,
) -> bool {
    on_clock(args) || grid.is_some_and(|g| g.wooze_knob().is_some())
}

/// The names of `world`'s entities that carry `component`.
pub(super) fn named_with(world: &flint_ecs::FlintWorld, component: &str) -> Vec<String> {
    world
        .all_entities()
        .into_iter()
        .filter(|entity| {
            world
                .get_components(entity.id)
                .is_some_and(|comps| comps.get(component).is_some())
        })
        .map(|entity| entity.name)
        .collect()
}

/// What a render that didn't run `scripted`'s scripts is missing (#371): a
/// scene whose look a script sets draws plain without them.
pub(super) fn scripts_not_run(scripted: &[String]) -> String {
    format!(
        "scripts don't run without --scripts, so what {} would do to this scene isn't in it \
         (--script-time T runs them to game time T)",
        scripted.join(", ")
    )
}

/// What a reader of this capture has to be told about what it isn't showing.
fn stage_warnings(
    args: &RenderArgs,
    grid: Option<&crate::commands::sweep::SweepGrid>,
    stage: &Stage,
) -> Vec<String> {
    let mut warnings = Vec::new();
    if !is_moment(args, grid) {
        return warnings;
    }
    // Scripts are a clock of their own, so with them running this says nothing
    if render_report::of_game::count(&stage.game) == 0 && !stage.scripted {
        warnings.push("this scene has no living props, so only its other clocks move".to_string());
    }
    let with = |component: &str| named_with(stage.game.world(), component);
    let scripts = with(comp::SCRIPT);
    if !scripts.is_empty() && !stage.scripted {
        warnings.push(scripts_not_run(&scripts));
    }
    let animated = with(comp::ANIMATOR);
    if !animated.is_empty() && stage.game.animation().is_none() {
        warnings.push(format!(
            "animation clips don't run without --anim-time, so {} won't match the player",
            animated.join(", ")
        ));
    }
    warnings
}

/// The entities that asked to be living props and drew as still models.
fn still_props(stage: &Stage) -> Vec<String> {
    let living = render_report::of_game::entities(&stage.game);
    let world = stage.game.world();
    world
        .all_entities()
        .into_iter()
        .filter(|entity| {
            !living.contains(&entity.id)
                && world
                    .get_components(entity.id)
                    .is_some_and(|comps| comps.get(crate::check::LIVING_PROP).is_some())
        })
        .map(|entity| entity.name)
        .collect()
}

/// The preset file a `--sweep wooze.preset=...` value names, if it's a file the
/// scene can see rather than a built-in (#249).
fn preset_file_of(
    args: &RenderArgs,
    sweep: &crate::commands::sweep::Sweep,
    index: usize,
) -> Result<Option<flint_procgen::WoozePreset>> {
    if sweep.wooze_knob() != Some("preset") {
        return Ok(None);
    }
    let name = &sweep.values[index];
    match flint_procgen::read_preset(Path::new(&args.scene), name) {
        None => Ok(None),
        Some(Ok(preset)) => Ok(Some(preset)),
        Some(Err(reason)) => Err(anyhow!("--sweep: preset '{name}': {reason}")),
    }
}

/// What a sweep cell lays over the scene's `[wooze]` table: a preset file one
/// of its axes names (its knobs first), then every other wooze value it takes,
/// so a `wooze.glide` row turns the glide a preset column set (#596). `None`
/// for a cell no axis of which is a wooze knob.
pub(super) fn cell_knobs(
    args: &RenderArgs,
    grid: Option<&crate::commands::sweep::SweepGrid>,
    (column, row): (usize, usize),
) -> Result<(Option<flint_procgen::WoozePreset>, Option<toml::Table>)> {
    let Some(grid) = grid else {
        return Ok((None, None));
    };
    let axes = grid.axes(column, row);
    let mut preset = None;
    for &(sweep, i) in &axes {
        if let Some(file) = preset_file_of(args, sweep, i)? {
            preset = Some(file);
        }
    }
    let mut table = preset.as_ref().map(|p| preset_wooze_table(&p.wooze));
    for &(sweep, i) in &axes {
        if preset.is_some() && sweep.wooze_knob() == Some("preset") {
            continue;
        }
        if let Some(knobs) = sweep.wooze_table(i) {
            table.get_or_insert_with(toml::Table::new).extend(knobs);
        }
    }
    Ok((preset, table))
}

/// A preset's `[wooze]` knobs as the table the field takes.
fn preset_wooze_table(knobs: &flint_procgen::WoozeKnobs) -> toml::Table {
    let mut table = toml::Table::new();
    for (key, value) in [
        ("amplitude", knobs.amplitude),
        ("step", knobs.step),
        ("glide", knobs.glide),
        ("jitter", knobs.jitter),
    ] {
        if let Some(value) = value {
            table.insert(key.to_string(), toml::Value::Float(f64::from(value)));
        }
    }
    table
}

/// `   N living props` for a sheet's subtitle, or nothing in a scene without.
fn living_note(stages: &[Stage]) -> String {
    let props = render_report::of_game::count(&stages[0].game);
    if props > 0 {
        format!("   {props} living props")
    } else {
        String::new()
    }
}

/// What a sweep's sheet says, as of the moment each stage last captured: the
/// moment top right, and under a wooze column's label what the props read.
fn sheet_text(
    sweep: &crate::commands::sweep::Sweep,
    scene_name: &str,
    t: f64,
    stages: &[Stage],
) -> crate::commands::style_sheet::SheetText {
    let mut text = crate::commands::style_sheet::SheetText::new(
        scene_name,
        format!("sweep {}{}", sweep.name(), living_note(stages)),
        (0..sweep.values.len()).map(|i| sweep.label(i)).collect(),
    );
    text.time = format!("T {t:.2}S");
    if let Some(knob) = sweep.wooze_knob() {
        text.notes = sweep
            .values
            .iter()
            .zip(stages)
            .map(|(value, stage)| {
                render_report::of_game::read_note(&stage.game, knob, value.parse().ok())
            })
            .collect();
    }
    text
}
