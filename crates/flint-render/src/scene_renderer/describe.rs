//! A frame's description (#1084, ADR 0126): arming the log, the draws and
//! targets each pass logs, and turning the log into a
//! [`FrameDescription`](crate::frame_capture::FrameDescription).
//!
//! The log is written where the frame is recorded (`encode_to`, the shadow
//! pass, the post chain, comic panels), through
//! [`FrameScopes`](crate::frame_capture::FrameScopes); this file holds what
//! those sites call, so each stays a line. Nothing here runs unless a host
//! asked with [`SceneRenderer::describe_next_frame`].

use super::*;
use crate::frame_capture::{
    self as fc, DrawDescription, FrameDescription, FrameLog, LoggedDraw, LoggedPass,
    PassDescription, TargetDescription,
};

/// What a host knows of the frame that the renderer doesn't: the adapter
/// it runs on and, for a window, its present mode.
#[derive(Default, Clone, Copy)]
pub struct FrameHost<'a> {
    pub adapter: Option<&'a wgpu::AdapterInfo>,
    pub present_mode: Option<wgpu::PresentMode>,
}

impl SceneRenderer {
    /// Describe the next frame (#1084): from now until
    /// [`describe_frame`](Self::describe_frame), each `prepare` starts the
    /// log afresh and each `encode_to` writes its passes, draws and targets
    /// into it. Without this the renderer keeps no log and a frame costs
    /// what it did.
    pub fn describe_next_frame(&mut self) {
        self.frame_log = Some(Box::default());
    }

    /// Whether a frame is being described.
    pub fn describing_frame(&self) -> bool {
        self.frame_log.is_some()
    }

    /// The frame drawn since [`describe_next_frame`](Self::describe_next_frame),
    /// described, and the log dropped (the next frame is described only if
    /// asked again). `world` names each draw's entity (without it draws
    /// carry ids alone); `host` adds the adapter and present mode. GPU times
    /// are laid on afterwards from that frame's raw read-back
    /// ([`FrameDescription::lay_gpu_times`]), which arrives a frame or more
    /// later. `None` when nothing was asked.
    pub fn describe_frame(
        &mut self,
        world: Option<&FlintWorld>,
        host: &FrameHost<'_>,
    ) -> Option<FrameDescription> {
        let log = *self.frame_log.take()?;
        let mut names = Namer {
            world,
            meshes: self.mesh_cache.names_by_buffer(),
            materials: HashMap::new(),
        };
        let FrameLog {
            views,
            page,
            panels,
            notes,
            ..
        } = log;
        let styles = views.last().map(|v| v.styles.clone()).unwrap_or_default();
        let mut views = views.into_iter();
        let (passes, panels) = if panels.is_empty() {
            let first = views.next().map(|v| v.passes).unwrap_or_default();
            let mut passes: Vec<PassDescription> =
                first.into_iter().map(|p| names.pass(p)).collect();
            passes.extend(page.into_iter().map(|p| names.pass(p)));
            // A split view's other side (#238) is a view of its own
            let again = views
                .enumerate()
                .map(|(i, v)| fc::PanelDescription {
                    name: format!("again {}", i + 1),
                    size: v.size,
                    passes: v.passes.into_iter().map(|p| names.pass(p)).collect(),
                })
                .collect();
            (passes, again)
        } else {
            let panels = views
                .zip(panels)
                .map(|(v, name)| fc::PanelDescription {
                    name,
                    size: v.size,
                    passes: v.passes.into_iter().map(|p| names.pass(p)).collect(),
                })
                .collect();
            (page.into_iter().map(|p| names.pass(p)).collect(), panels)
        };

        let mut stats = self.collect_stats();
        stats.resolution = self.surface_size.into();
        let targets = self.frame_targets();
        let style_target_bytes = self.style_target_entries().iter().map(|t| t.bytes).sum();
        let config = self.effective_post_process_config();
        let post_process = flint_schema::post_effects::params()
            .filter_map(|p| {
                let value = config.param(p.key)?;
                let json = match value {
                    flint_schema::post_effects::PostValue::Bool(b) => serde_json::json!(b),
                    flint_schema::post_effects::PostValue::F32(v) => serde_json::json!(v),
                    flint_schema::post_effects::PostValue::U32(v) => serde_json::json!(v),
                    flint_schema::post_effects::PostValue::Rgb(c) => serde_json::json!(c),
                };
                Some((p.key.to_string(), json))
            })
            .collect();
        let quality = self
            .quality
            .entries()
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let (kept, left, default_lights) = self.lights.described();
        let light = |(kind, name): (&str, String), casts_shadow: bool| fc::LightEntry {
            kind: kind.to_string(),
            entity_id: world.and_then(|w| w.get_id(&name)).map(|id| id.0),
            entity: Some(name),
            casts_shadow,
        };
        let mut described = FrameDescription {
            frame: fc::FrameInfo {
                index: None,
                game_time: None,
                dt: None,
                size: self.surface_size.into(),
                scene_size: self.scene_size().into(),
                render_scale: self.render_scale,
                msaa: self.pipelines.samples(),
                surface_format: format!("{:?}", self.surface_format),
                adapter: host.adapter.map(crate::gpu_select::adapter_line),
                backend: host.adapter.map(|a| format!("{:?}", a.backend)),
                present_mode: host.present_mode.map(|m| format!("{m:?}")),
                debug_mode: format!("{:?}", self.debug_state.mode),
                quality,
                post_process,
                styles,
                gpu_frame_ms: None,
                capturing: false,
            },
            stats,
            lights: fc::LightsDescription {
                kept: kept
                    .into_iter()
                    .map(|(kind, name, shadow)| light((kind, name), shadow))
                    .collect(),
                left_out: left.into_iter().map(|l| light(l, false)).collect(),
                default_lights,
            },
            memory: fc::MemoryDescription {
                target_bytes: targets.iter().map(|t| t.bytes).sum(),
                style_target_bytes,
            },
            targets,
            passes,
            panels,
            notes,
            ..Default::default()
        };
        name_images(&mut described);
        Some(described)
    }

    /// Every texture this frame's view draws into or reads, each once.
    fn frame_targets(&self) -> Vec<TargetDescription> {
        let mut out = Vec::new();
        let (w, h) = self.surface_size;
        out.push(TargetDescription {
            id: "surface".into(),
            format: format!("{:?}", self.surface_format),
            size: [w, h],
            layers: 1,
            samples: 1,
            bytes: w as u64 * h as u64 * 4,
        });
        // The caller's depth buffer: the headless and window contexts both
        // make it Depth32Float
        out.push(TargetDescription {
            id: "depth".into(),
            format: "Depth32Float".into(),
            size: [w, h],
            layers: 1,
            samples: 1,
            bytes: w as u64 * h as u64 * 4,
        });
        if let Some(d) = &self.scaled_depth {
            out.push(fc::target("scene_depth", &d.texture));
        }
        if let Some(r) = &self.postprocess_resources {
            out.push(fc::target("hdr_scene", &r.hdr_texture));
            out.push(fc::target("ssao", &r.ssao_texture));
            out.push(fc::target("ssao_blur", &r.ssao_blur_texture));
            out.push(fc::target("volumetric", &r.volumetric_texture));
            out.push(fc::target("volumetric_blur", &r.volumetric_blur_texture));
            for (i, mip) in r.bloom_mips.iter().enumerate() {
                out.push(fc::target(&format!("bloom_mip_{i}"), &mip.texture));
            }
            if let Some(f) = &r.fxaa {
                out.push(fc::target("fxaa_in", &f.texture));
            }
            if let Some(v) = &r.fog_veil {
                out.push(fc::target("fog_veil_behind", &v.behind));
                out.push(fc::target("fog_veil", &v.veil));
            }
            for t in r.styles.entries() {
                out.push(TargetDescription {
                    id: t.name.to_string(),
                    format: format!("{:?}", t.format),
                    size: [t.width, t.height],
                    layers: 1,
                    samples: 1,
                    bytes: t.bytes,
                });
            }
        }
        let (color, depth) = self.msaa.textures();
        if let Some(t) = color {
            out.push(fc::target("msaa_color", t));
        }
        if let Some(t) = depth {
            out.push(fc::target("msaa_depth", t));
        }
        if let Some(sp) = &self.shadow_pass {
            out.push(fc::target("shadow_map", &sp.shadow_texture));
        }
        if let Some((t, _)) = &self.water.grab.color {
            out.push(fc::target("ocean_grab_color", t));
        }
        if let Some((t, _)) = &self.water.grab.depth {
            out.push(fc::target("ocean_grab_depth", t));
        }
        out
    }

    // ── What the passes log ──

    /// The depth target the scene draws against: its own below render
    /// scale 1 (#122), else the caller's.
    pub(super) fn scene_depth_id(&self) -> &'static str {
        if self.scaled_depth.is_some() {
            "scene_depth"
        } else {
            "depth"
        }
    }

    /// The scene pass's draws, in the order it draws them, each through the
    /// main pass's own selector (`main_pass_draws`); the culled beside them
    /// with their reason. `eye` is where this camera's culls were made from.
    pub(super) fn log_scene_draws(&self, pass: &mut LoggedPass, eye: [f32; 3]) {
        for layer in Layer::ALL {
            for item in self.draws.layer(layer) {
                if let Some(why) = self.cull_reason(item, eye) {
                    pass.culled.push(self.logged_draw(item, Some(why), ""));
                } else if self.main_pass_draws(item) {
                    let pipeline = self.main_pipeline_name(item);
                    pass.draws.push(self.logged_draw(item, None, pipeline));
                } else if let Draw::Terrain(_) = item.draw {
                    pass.culled
                        .push(self.logged_draw(item, Some("frustum"), ""));
                }
            }
            // What the main pass draws between the list's layers
            match layer {
                Layer::Terrain if self.grass.drawing && self.pipelines.grass.is_some() => {
                    let grass = self.extra_draw("Grass", "grass", self.grass.instance_count);
                    if self.draws.grass_cut() {
                        let mut grass = grass;
                        grass.draw.culled = Some("past the draw limit".into());
                        pass.culled.push(grass);
                    } else {
                        pass.draws.push(grass);
                    }
                }
                Layer::Billboard if self.pipelines.sprite2d.is_some() => {
                    let drawn = self.draws.sprites_drawn();
                    for (i, batch) in self.sprite2d_batches.iter().enumerate() {
                        let mut d =
                            self.extra_draw("Sprite batch", "sprite2d", batch.instance_count);
                        d.draw.triangles = batch.instance_count * 2;
                        if i < drawn {
                            pass.draws.push(d);
                        } else {
                            d.draw.culled = Some("past the draw limit".into());
                            pass.culled.push(d);
                        }
                    }
                }
                Layer::Terrain
                | Layer::OpaqueStatic
                | Layer::OpaqueSkinned
                | Layer::Billboard
                | Layer::BlendedStatic
                | Layer::BlendedSkinned
                | Layer::Echo
                | Layer::Particles => {}
            }
        }
        let mut also = vec!["Sky"];
        if self.debug_state.mode == DebugMode::WireframeOnly {
            also.push("Wireframe");
        }
        if self.water_active() {
            also.push("Water");
        }
        if self.grid_draw.is_some() {
            also.push("Grid");
        }
        if self.debug_state.mode == DebugMode::WireframeOverlay {
            also.push("Wireframe overlay");
        }
        if self.debug_state.show_normals {
            also.push("Normal arrows");
        }
        if self.debug_state.show_skeleton {
            also.push("Skeletons");
        }
        if !self.debug_overlay_draws.is_empty() {
            also.push("Gizmos");
        }
        pass.pass.also.extend(also.into_iter().map(String::from));
    }

    /// Why `item` is set aside from here: `None` when this camera draws it.
    fn cull_reason(&self, item: &DrawItem, eye: [f32; 3]) -> Option<&'static str> {
        if item.drawn() {
            return None;
        }
        if let Some(why) = item.replay_cull() {
            return Some(why);
        }
        if !item.lod_culled() {
            return Some("frustum");
        }
        let lod = match &item.draw {
            Draw::Static(d) => d.lod,
            Draw::Skinned(_)
            | Draw::Echo(_)
            | Draw::Billboard(_)
            | Draw::Terrain(_)
            | Draw::Particles(_) => None,
        };
        let Some(lod) = lod else {
            return Some("lod");
        };
        let d = distance_to_box(eye, lod.ref_min, lod.ref_max);
        let far = lod.band.draw_distance * self.lod_settings.draw_distance_scale;
        Some(if d >= far { "draw distance" } else { "lod" })
    }

    /// One item as a described draw.
    pub(super) fn logged_draw(
        &self,
        item: &DrawItem,
        culled: Option<&str>,
        pipeline: &str,
    ) -> LoggedDraw {
        let key = |b: &Arc<wgpu::Buffer>| Some(Arc::as_ptr(b) as usize);
        let material = |m: &Arc<MaterialBuffer>| Some(Arc::as_ptr(m) as usize);
        let (mesh_key, material_key, triangles, instances, double_sided, mesh) = match &item.draw {
            Draw::Static(d) => (
                key(&d.vertex_buffer),
                material(&d.material_buffer),
                d.index_count / 3,
                1,
                d.double_sided,
                None,
            ),
            Draw::Skinned(d) => (
                key(&d.vertex_buffer),
                material(&d.material_buffer),
                d.index_count / 3,
                1,
                d.double_sided,
                None,
            ),
            Draw::Echo(d) => (
                key(&d.vertex_buffer),
                material(&d.material_buffer),
                d.index_count / 3,
                1,
                false,
                None,
            ),
            Draw::Billboard(_) => (None, None, 2, 1, false, Some("billboard quad".to_string())),
            Draw::Terrain(n) => {
                let t = self
                    .terrain
                    .draws
                    .get(*n as usize)
                    .map_or(0, |d| d.index_count / 3);
                (None, None, t, 1, false, Some(format!("terrain chunk {n}")))
            }
            Draw::Particles(n) => {
                let count = self
                    .particle_draws
                    .get(*n as usize)
                    .map_or(0, |d| d.instance_count);
                (
                    None,
                    None,
                    count * 2,
                    count,
                    false,
                    Some(format!("particle draw {n}")),
                )
            }
        };
        LoggedDraw {
            entity: item.entity,
            mesh_key,
            material_key,
            draw: DrawDescription {
                kind: item.draw.kind().to_string(),
                layer: item.layer().label().to_string(),
                blend: blend_name(item.blend).to_string(),
                vertex: match item.vertex {
                    VertexStage::Plain => "plain",
                    VertexStage::Wooze(_) => "wooze",
                }
                .to_string(),
                pipeline: pipeline.to_string(),
                mesh,
                triangles,
                instances,
                casts_shadow: item.casts_shadow,
                double_sided,
                culled: culled.map(String::from),
                ..Default::default()
            },
        }
    }

    /// A draw that isn't a list item (grass, a 2D sprite batch).
    fn extra_draw(&self, kind: &str, pipeline: &str, instances: u32) -> LoggedDraw {
        LoggedDraw {
            entity: None,
            mesh_key: None,
            material_key: None,
            draw: DrawDescription {
                kind: kind.to_string(),
                layer: kind.to_string(),
                blend: "opaque".into(),
                vertex: "plain".into(),
                pipeline: pipeline.to_string(),
                instances,
                ..Default::default()
            },
        }
    }

    /// The pipeline [`draw_main`](Self::draw_main) draws `item` through, by
    /// the same choices it makes (kind, blend, vertex stage, sides).
    fn main_pipeline_name(&self, item: &DrawItem) -> &'static str {
        let woozy_static = item.wooze().is_some() && self.pipelines.wooze_standard.is_some();
        let woozy_skinned = item.wooze().is_some() && self.pipelines.wooze_skinned.is_some();
        let opaque = item.blend == Blend::Opaque;
        let mode = item.blend.mesh_mode();
        let blended = |woozy: bool, skinned: bool| match (woozy, skinned, mode) {
            (true, _, crate::pipeline::BlendMode::Alpha) => "wooze alpha (back, then front)",
            (true, _, crate::pipeline::BlendMode::Additive) => "wooze additive",
            (true, _, crate::pipeline::BlendMode::Multiply) => "wooze multiply",
            (false, false, crate::pipeline::BlendMode::Alpha) => "pbr alpha (back, then front)",
            (false, false, crate::pipeline::BlendMode::Additive) => "pbr additive",
            (false, false, crate::pipeline::BlendMode::Multiply) => "pbr multiply",
            (false, true, crate::pipeline::BlendMode::Alpha) => "skinned alpha (back, then front)",
            (false, true, crate::pipeline::BlendMode::Additive) => "skinned additive",
            (false, true, crate::pipeline::BlendMode::Multiply) => "skinned multiply",
        };
        match &item.draw {
            Draw::Terrain(_) => "terrain",
            Draw::Static(d) if opaque => match (d.is_wireframe, woozy_static, d.double_sided) {
                (true, _, _) => "line",
                (false, true, true) => "wooze opaque double-sided",
                (false, true, false) => "wooze opaque",
                (false, false, true) => "pbr double-sided",
                (false, false, false) => "pbr",
            },
            Draw::Static(_) => blended(woozy_static, false),
            Draw::Skinned(d) if opaque => match (woozy_skinned, d.double_sided) {
                (true, true) => "wooze skinned double-sided",
                (true, false) => "wooze skinned",
                (false, true) => "skinned double-sided",
                (false, false) => "skinned",
            },
            Draw::Skinned(d) if d.soft_edge_only && !woozy_skinned => "skinned soft edge",
            Draw::Skinned(_) => blended(woozy_skinned, true),
            Draw::Billboard(_) => "billboard",
            Draw::Echo(_) if item.blend == Blend::Lighten => "echo lighten",
            Draw::Echo(_) => "skinned alpha (echo)",
            Draw::Particles(n) => match self.particle_draws.get(*n as usize).map(|d| d.blend) {
                Some(flint_particles::ParticleBlendMode::Alpha) => "particles alpha",
                Some(flint_particles::ParticleBlendMode::Premultiplied) => {
                    "particles premultiplied"
                }
                Some(flint_particles::ParticleBlendMode::Additive) => "particles additive",
                Some(flint_particles::ParticleBlendMode::Multiply) => "particles multiply",
                None => "particles",
            },
        }
    }

    /// Why a post effect switched by `key` (`ssao_enabled`, `fxaa`...) is
    /// off this frame: post-processing off, a quality cap over the authored
    /// config, or the config's own key.
    pub(super) fn post_off_reason(&self, key: &str) -> String {
        let drawn = &self.postprocess_config;
        if !drawn.enabled {
            return "post-processing is off".into();
        }
        let on = |c: &PostProcessConfig| {
            matches!(
                c.param(key),
                Some(flint_schema::post_effects::PostValue::Bool(true))
            )
        };
        if on(&self.authored_postprocess) && !on(drawn) {
            let cap = match key {
                "ssao_enabled" => "ssao",
                "bloom_enabled" => "bloom",
                "volumetric_enabled" => "volumetric",
                "fog_enabled" => "fog",
                other => other,
            };
            return format!("capped by quality ({cap} = false)");
        }
        format!("off ([post_process] {key} = false)")
    }

    /// Why the shadow pass draws nothing this frame.
    pub(super) fn shadows_off_reason(&self) -> String {
        if !self.shadows_enabled() {
            if !self.quality.shadows {
                return "capped by quality (shadows = false)".into();
            }
            return "shadows are off".into();
        }
        if !self.lights.scene_shadows() {
            return "the scene says [environment] shadows = false".into();
        }
        "no sun gives light and no spot or point light casts".into()
    }
}

/// Names a logged frame's entities, meshes and materials as it becomes a
/// description.
struct Namer<'w> {
    world: Option<&'w FlintWorld>,
    meshes: HashMap<usize, (String, usize, String)>,
    /// Each material buffer seen, numbered by first appearance.
    materials: HashMap<usize, u32>,
}

impl Namer<'_> {
    fn pass(&mut self, logged: LoggedPass) -> PassDescription {
        let LoggedPass {
            mut pass,
            draws,
            culled,
            passes,
        } = logged;
        pass.draws = draws.into_iter().map(|d| self.draw(d)).collect();
        pass.culled = culled.into_iter().map(|d| self.draw(d)).collect();
        pass.passes = passes.into_iter().map(|p| self.pass(p)).collect();
        pass
    }

    fn draw(&mut self, logged: LoggedDraw) -> DrawDescription {
        let LoggedDraw {
            entity,
            mesh_key,
            material_key,
            mut draw,
        } = logged;
        if let Some(id) = entity {
            draw.entity_id = Some(id.0);
            draw.entity = self.world.and_then(|w| w.get_name(id)).map(str::to_string);
        }
        if let Some(key) = mesh_key {
            match self.meshes.get(&key) {
                Some((name, i, material)) => {
                    draw.mesh = Some(format!("{name}#{i}"));
                    draw.material = Some(material.clone());
                }
                // Not a cached model's mesh: a procedural box
                None if draw.kind == "Static" || draw.kind == "Wire box" => {
                    draw.mesh = Some("box".into());
                }
                None => {}
            }
        }
        if let Some(key) = material_key {
            let next = self.materials.len() as u32;
            draw.material_id = Some(*self.materials.entry(key).or_insert(next));
        }
        draw
    }
}

fn blend_name(blend: Blend) -> &'static str {
    match blend {
        Blend::Opaque => "opaque",
        Blend::Alpha => "alpha",
        Blend::Premultiplied => "premultiplied",
        Blend::Additive => "additive",
        Blend::Multiply => "multiply",
        Blend::Lighten => "lighten",
    }
}

/// Give each image a captured frame read (#1085) its file, ADR 0126's
/// `targets/NN-<target>.<ext>`: `NN` the pass's place in
/// [`FrameDescription::all_passes`], a shadow layer named by its pass
/// (`shadow_cascade_0`), a step after the target (`bloom_mip_1-down`).
/// Marks the frame `capturing` when it has any.
fn name_images(described: &mut FrameDescription) {
    let mut place = 0usize;
    let mut taken = std::collections::HashSet::new();
    let mut any = false;
    described.each_pass_mut(&mut |pass| {
        for image in &mut pass.images {
            any = true;
            let mut stem = if image.layer.is_some() {
                crate::frame_taps::slug(&pass.name)
            } else {
                crate::frame_taps::slug(&image.target)
            };
            if let Some(step) = &image.step {
                stem = format!("{stem}-{}", crate::frame_taps::slug(step));
            }
            let ext = match image.stored.as_str() {
                "png" | "png16" => "png",
                "raw" => "bin",
                _ => "exr",
            };
            let mut file = format!("targets/{place:02}-{stem}.{ext}");
            let mut again = 2;
            while !taken.insert(file.clone()) {
                file = format!("targets/{place:02}-{stem}-{again}.{ext}");
                again += 1;
            }
            image.file = file;
        }
        place += 1;
    });
    described.frame.capturing = any;
}
