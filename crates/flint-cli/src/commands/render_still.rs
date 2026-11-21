//! The one-renderer still (#1145): a frame or a sweep's columns drawn on
//! one renderer, `--gpu-time`, `--ai-refine`, and the measures and ASCII
//! text a still and a stage both write.

use super::*;

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

/// `--ai-refine` (#357): run the loop, then write the refined look, its log
/// and the sheet of rounds.
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
            "--ascii-text: the ASCII style didn't draw this frame; turn it on with --ascii, \
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
