//! A `--frames` or `--mesh` sequence (#345, #222, #1145): one renderer
//! drawing frame after frame, written as PNGs, a GIF or a sheet.

use super::*;

/// What a `--frames` sequence draws with.
pub(super) struct SequenceRig<'a> {
    pub(super) ctx: &'a HeadlessContext,
    pub(super) renderer: &'a mut SceneRenderer,
    pub(super) world: &'a mut flint_ecs::FlintWorld,
    pub(super) camera: Camera,
    pub(super) camera_path: Option<&'a crate::commands::frame_seq::CameraPath>,
    pub(super) particles: Option<&'a mut flint_particles::ParticleSystem>,
    /// `--anim-time` (#307): steps on a frame at a time from there
    pub(super) animation: Option<&'a mut HeadlessAnimation>,
    /// `--mesh` (#222): each frame shows the next file on each binding
    pub(super) meshes: &'a [crate::commands::mesh_seq::MeshBinding],
    pub(super) scene_name: &'a str,
}

/// Where a `--mesh` sequence's frames go: a PNG a frame (`seq.png` numbered as
/// --frames numbers them, or `frames/f-%05d.png`), one GIF, or with `--sheet`
/// one contact sheet.
pub(super) fn check_mesh_output(args: &RenderArgs, files: usize) -> Result<()> {
    let out = &args.output;
    let moving = render_plan::is_gif(out) || render_plan::is_sequence(out);
    if args.seq.sheet && moving {
        anyhow::bail!("--sheet writes one image: give --output a plain .png, not {out}");
    }
    if render_plan::is_gif(out) && files < 2 {
        anyhow::bail!("--mesh matched one file, and a GIF of one frame is a still: -o x.png");
    }
    if args.seq.no_images && (moving || args.seq.sheet) {
        anyhow::bail!("--no-images writes no frames, so {out} would be empty: drop one");
    }
    Ok(())
}

/// Point `binding`'s entity at the asset its frames are uploaded under. Its
/// `model` is marked `runtime`, so the scene's model loader leaves it to the
/// sequence; an entity with no model gets one.
pub(super) fn bind_mesh(
    world: &mut flint_ecs::FlintWorld,
    binding: &crate::commands::mesh_seq::MeshBinding,
) -> Result<()> {
    use flint_core::components::MODEL;
    let Some(id) = world.get_id(&binding.entity) else {
        let mut names: Vec<String> = world
            .all_entities()
            .iter()
            .filter(|e| {
                world
                    .get_components(e.id)
                    .is_some_and(|c| c.get(MODEL).is_some())
            })
            .map(|e| e.name.clone())
            .collect();
        names.sort();
        let known = if names.is_empty() {
            String::new()
        } else {
            format!(" (entities with a model: {})", names.join(", "))
        };
        anyhow::bail!(
            "--mesh: no entity named '{}' in the scene{known}",
            binding.entity
        );
    };
    let mut model = world
        .get_components(id)
        .and_then(|c| c.get(MODEL))
        .and_then(|m| m.as_table().cloned())
        .unwrap_or_default();
    model.insert("asset".into(), binding.asset().into());
    model.insert("runtime".into(), true.into());
    world
        .set_component(id, MODEL, toml::Value::Table(model))
        .map_err(|e| anyhow::anyhow!("--mesh {}: {e}", binding.entity))?;
    Ok(())
}

/// Upload frame `index` of every binding in place of the frame before it,
/// whose namespaced textures go first so that no draw binds the old ones.
fn swap_meshes(
    ctx: &HeadlessContext,
    renderer: &mut SceneRenderer,
    meshes: &[crate::commands::mesh_seq::MeshBinding],
    index: usize,
    textures: &mut Vec<Vec<String>>,
) -> Result<()> {
    textures.resize(meshes.len(), Vec::new());
    for (binding, old) in meshes.iter().zip(textures.iter_mut()) {
        let file = &binding.files[index];
        let import = flint_import::import_gltf(file)
            .with_context(|| format!("--mesh {}: importing {}", binding.entity, file.display()))?;
        let import = crate::commands::mesh_seq::flatten(import);
        let asset = binding.asset();
        for texture in old.drain(..) {
            renderer.remove_texture(&texture);
        }
        renderer.remove_mesh(&asset);
        renderer.load_model(&ctx.device, &ctx.queue, &asset, &import);
        *old = import
            .materials
            .iter()
            .flat_map(|m| {
                [
                    &m.base_color_texture,
                    &m.emissive_texture,
                    &m.normal_texture,
                    &m.metallic_roughness_texture,
                ]
            })
            .flatten()
            // A texture used as colour and as data is uploaded twice, the
            // data copy suffixed (namespace_textures); a missing name is a no-op
            .flat_map(|t| [format!("{asset}::{t}"), format!("{asset}::{t}#linear")])
            .collect();
    }
    Ok(())
}

/// Where a sequence writes its frames. `--frames` alone always numbers PNGs
/// beside --output, as it did; a `--mesh` sequence also takes `%05d`, a GIF
/// and `--sheet`.
#[derive(Clone, Copy, PartialEq)]
enum MeshOutput {
    /// `seq.png` -> `seq_0000.png`...
    Numbered,
    /// `frames/f-%05d.png`
    Pattern,
    Gif,
    Sheet,
}

impl MeshOutput {
    fn of(args: &RenderArgs, mesh: bool) -> Self {
        match () {
            _ if !mesh => Self::Numbered,
            _ if args.seq.sheet => Self::Sheet,
            _ if render_plan::is_gif(&args.output) => Self::Gif,
            _ if render_plan::is_sequence(&args.output) => Self::Pattern,
            _ => Self::Numbered,
        }
    }

    fn path(self, output: &str, index: u32, frames: u32) -> std::path::PathBuf {
        match self {
            Self::Pattern => render_plan::sequence_path(output, index as usize).into(),
            _ => crate::commands::frame_seq::numbered(output, index, frames),
        }
    }
}

/// A frame's name on a `--mesh` sheet: its file's, or each binding's joined.
fn mesh_label(meshes: &[crate::commands::mesh_seq::MeshBinding], index: usize) -> String {
    meshes
        .iter()
        .map(|b| crate::commands::mesh_seq::label(&b.files[index]))
        .collect::<Vec<_>>()
        .join(" / ")
}

/// `--frames N` (#345): N frames on one renderer, `--frame-dt` apart, the
/// camera moved between them, each written as a numbered PNG (and grid
/// text), with the glyph flips counted when asked.
pub(super) fn render_sequence(args: &RenderArgs, rig: SequenceRig, frames: u32) -> Result<()> {
    use crate::commands::frame_seq::{numbered, FlipCounter};
    let SequenceRig {
        ctx,
        renderer,
        world,
        mut camera,
        camera_path,
        mut particles,
        mut animation,
        meshes,
        scene_name,
    } = rig;
    // A mesh sequence's frames are --fps apart, as its GIF plays them
    let dt = args.seq.frame_dt.unwrap_or(if meshes.is_empty() {
        1.0 / 60.0
    } else {
        1.0 / args.clock.fps
    });
    let wants_flips = args.seq.flip_stats || args.seq.flip_json.is_some();
    let wants_text = args.out.ascii_text.is_some() || args.out.ascii_text_ansi.is_some();
    let base_time = still_time(args);
    let yaw0 = camera.yaw;
    let (width, height) = (ctx.width, ctx.height);
    let mut mesh_textures = Vec::new();
    // Where each frame goes: a numbered PNG, a `%05d` PNG, one GIF, or a sheet
    let out = MeshOutput::of(args, !meshes.is_empty());
    let mut sheet_cells = Vec::new();
    if !args.seq.no_images {
        if let Some(dir) = Path::new(&args.output).parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir)
                    .context(format!("Failed to create {}", dir.display()))?;
            }
        }
    }
    let mut gif = match out {
        MeshOutput::Gif => Some(crate::commands::gif_writer::Spool::new(
            Path::new(&args.output),
            width,
            height,
            frames as usize,
        )?),
        _ => None,
    };
    let number = |p: &Option<String>, i: u32| {
        p.as_ref()
            .map(|p| numbered(p, i, frames).to_string_lossy().into_owned())
    };
    let mut flips = FlipCounter::default();
    let motion = match (args.seq.orbit, camera_path) {
        (Some(rate), _) => format!(", orbiting {rate} deg/s"),
        (None, Some(path)) => format!(", on a {}-key camera path", path.key.len()),
        _ => String::new(),
    };
    say!("Sequence: {frames} frames, {dt:.6} s apart{motion}");
    for i in 0..frames {
        let t = i as f32 * dt;
        if let Some(rate) = args.seq.orbit {
            camera.yaw = yaw0 + (rate * t).to_radians();
            camera.update_orbit();
        } else if let Some(path) = camera_path {
            let (position, target) = path.at(t);
            camera.position = position;
            camera.target = target;
        }
        // The post time and held poses (#405) on the frame's clock
        let time = base_time + t as f64;
        renderer.set_motion_time(time);
        if let Some(posed) = animation.as_deref_mut() {
            if i > 0 {
                posed.step(world, dt as f64);
                posed.upload(renderer, &ctx.device, &ctx.queue);
            }
        }
        if let Some(p) = particles.as_deref_mut() {
            if i > 0 {
                p.simulate_to(world, dt, dt.min(1.0 / 60.0));
            }
            p.pack(Some(camera.position_array()));
        }
        if !meshes.is_empty() {
            swap_meshes(ctx, renderer, meshes, i as usize, &mut mesh_textures)?;
        }
        // The frame's inputs, and the world read again: the draw lists hold
        // the poses and meshes they were built with (#739)
        renderer.prepare(
            &ctx.device,
            &ctx.queue,
            world,
            &FrameInputs {
                // The grass's wind from the sequence's start, as it always ran
                grass_time: Some(t),
                // History smoothing scales by the frame's game time (#342)
                dt,
                particles: particles.as_deref().map(|p| &p.sync),
                ..still_inputs(args, &camera, time)
            },
        );
        renderer.render_frame_to(
            &ctx.device,
            &ctx.queue,
            ctx.depth_target(),
            &camera,
            &ctx.color_view,
            Some(world),
        );
        if wants_text || wants_flips {
            let text = renderer.style_text(&ctx.device, "ascii");
            if wants_flips {
                let Some(text) = &text else {
                    anyhow::bail!(
                        "--flip-stats: the ASCII style didn't draw frame {i}; turn it on with \
                         --style ascii, a look, or the scene's [post_process.styles.ascii]"
                    );
                };
                let f = flips.push(text);
                if args.seq.flip_stats {
                    match (f.flips, f.reversals) {
                        (Some(n), Some(r)) => {
                            say!("  frame {i:>4}: {n:>6} flips {r:>6} reversals")
                        }
                        (Some(n), None) => say!("  frame {i:>4}: {n:>6} flips"),
                        _ => {}
                    }
                }
            }
            if wants_text {
                write_ascii_text(
                    number(&args.out.ascii_text, i).as_deref(),
                    number(&args.out.ascii_text_ansi, i).as_deref(),
                    text,
                )?;
            }
        }
        // --last-frame (#478) reads back only the frame it writes
        let kept = !args.seq.no_images && (!args.seq.last_frame || i + 1 == frames);
        if kept {
            let pixels =
                pollster::block_on(ctx.read_pixels()).context("Failed to read rendered pixels")?;
            let img = image::RgbaImage::from_raw(width, height, pixels)
                .context("Failed to create image from pixel data")?;
            match out {
                _ if args.seq.last_frame => {
                    img.save(&args.output)
                        .context(format!("Failed to save image to {}", args.output))?;
                }
                MeshOutput::Numbered | MeshOutput::Pattern => {
                    let path = out.path(&args.output, i, frames);
                    img.save(&path)
                        .context(format!("Failed to save image to {}", path.display()))?;
                }
                MeshOutput::Gif => {
                    let rgb = image::DynamicImage::ImageRgba8(img).into_rgb8();
                    gif.as_mut().expect("made above").push(&rgb)?;
                }
                MeshOutput::Sheet => sheet_cells.push(img),
            }
        }
    }
    if args.seq.last_frame {
        say!(
            "Rendered frame {} of {frames} ({width}x{height}) to {}",
            frames - 1,
            args.output
        );
    } else if !args.seq.no_images {
        match out {
            MeshOutput::Numbered | MeshOutput::Pattern => say!(
                "Rendered {frames} {width}x{height} frames to {} .. {}",
                out.path(&args.output, 0, frames).display(),
                out.path(&args.output, frames - 1, frames).display()
            ),
            MeshOutput::Gif => {
                let fps = 1.0 / dt;
                if fps > render_plan::GIF_MAX_FPS {
                    eprintln!(
                        "warning: --frame-dt {dt} is {fps:.0} fps; the GIF plays at \
                         {} (its delays count hundredths of a second)",
                        render_plan::GIF_MAX_FPS
                    );
                }
                gif.take()
                    .expect("made above")
                    .finish(fps.min(render_plan::GIF_MAX_FPS))?;
                say!(
                    "Rendered {frames} {width}x{height} frames at {fps:.1} fps to {}",
                    args.output
                );
            }
            MeshOutput::Sheet => {
                let labels = (0..frames as usize)
                    .map(|i| mesh_label(meshes, i))
                    .collect();
                let bound: Vec<String> = meshes
                    .iter()
                    .map(|b| format!("{} <- {} files", b.entity, b.files.len()))
                    .collect();
                let text = crate::commands::style_sheet::SheetText::new(
                    scene_name.to_string(),
                    format!("--mesh {}", bound.join(", ")),
                    labels,
                );
                let sheet = crate::commands::style_sheet::compose(&sheet_cells, &text);
                sheet
                    .save(&args.output)
                    .context(format!("Failed to save image to {}", args.output))?;
                say!(
                    "Rendered {frames} {width}x{height} frames as one {}x{} sheet to {}",
                    sheet.width(),
                    sheet.height(),
                    args.output
                );
            }
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
    Ok(())
}
