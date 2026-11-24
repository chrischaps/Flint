//! A frame's capture (#1085, ADR 0126): arming it, and the taps each pass
//! makes as it ends. The read-back is `crate::frame_taps`; the writer
//! `crate::frame_capture_write`.
//!
//! A tap reads what a pass wrote by the ids the frame's description logged
//! for it (`FrameScopes::note`), so the images and the description name the
//! same targets. Every tap site is `self.tap_pass(..)` after the pass, which
//! returns at once unless the frame is captured.

use super::*;
use crate::frame_capture::FrameScopes;
use crate::frame_replay::{
    BoneMatrix, PaletteEntry, PaletteKind, ReplayRenderer, ReplayStateCapture, StyleState,
};
use crate::frame_taps::{CaptureTargets, FrameCapture, FrameTaps};

/// A frame's description and capture, set aside while the host draws
/// something else ([`SceneRenderer::hold_frame_capture`]).
#[must_use = "put it back with SceneRenderer::resume_frame_capture"]
pub struct HeldFrameCapture {
    log: Option<Box<crate::frame_capture::FrameLog>>,
    taps: Option<Box<FrameTaps>>,
}

impl SceneRenderer {
    /// Capture the next frame (#1085): describe it ([`describe_next_frame`]
    /// (Self::describe_next_frame)) and copy out each target as the pass that
    /// wrote it leaves it. `targets` are the host's output and depth buffer,
    /// which the renderer draws into without holding (each needs
    /// `COPY_SRC`). After the frame is submitted, take the copies with
    /// [`take_frame_capture`](Self::take_frame_capture) and the description
    /// with [`describe_frame`](Self::describe_frame).
    pub fn capture_next_frame(&mut self, targets: CaptureTargets) {
        self.describe_next_frame();
        self.frame_taps = Some(Box::new(FrameTaps::new(targets)));
    }

    /// Whether a frame is being captured.
    pub fn capturing_frame(&self) -> bool {
        self.frame_taps.is_some()
    }

    /// A pass of the host's own after the renderer's (#1086: the player's
    /// HUD, `UI`), while the frame is described: logged on the page after
    /// the renderer's passes, as writing `target`, and, while the frame is
    /// captured, `texture` copied as the pass left it. `timed` says whether
    /// the host opened a GPU timer scope of that name for it. Call after the
    /// host's pass is recorded and before the frame is taken. Nothing
    /// unless a frame is described.
    pub fn capture_host_pass(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        name: &str,
        target: &str,
        texture: &wgpu::Texture,
        timed: bool,
    ) {
        let Some(log) = self.frame_log.as_deref_mut() else {
            return;
        };
        log.on_page = true;
        let pass = log.ran(name, timed);
        pass.reads(&[target]).writes(&[target]);
        if let Some(taps) = self.frame_taps.as_deref_mut() {
            taps.copy(device, encoder, pass, target, None, texture, None);
        }
    }

    /// Set the frame's description and capture aside while the host draws
    /// something that isn't the frame being captured (#1086: the Living
    /// Props split's other side, `encode_again_to`), so it is neither
    /// logged nor read; [`resume_frame_capture`](Self::resume_frame_capture)
    /// puts it back.
    pub fn hold_frame_capture(&mut self) -> HeldFrameCapture {
        HeldFrameCapture {
            log: self.frame_log.take(),
            taps: self.frame_taps.take(),
        }
    }

    /// Put back what [`hold_frame_capture`](Self::hold_frame_capture) set
    /// aside.
    pub fn resume_frame_capture(&mut self, held: HeldFrameCapture) {
        if held.log.is_some() {
            self.frame_log = held.log;
        }
        if held.taps.is_some() {
            self.frame_taps = held.taps;
        }
    }

    /// The copies the frame since [`capture_next_frame`](Self::capture_next_frame)
    /// made, to map and read now that it is submitted; the capture ends
    /// (the next frame is captured only if asked again). `None` when
    /// nothing was asked.
    pub fn take_frame_capture(&mut self) -> Option<FrameCapture> {
        if let Some(r) = &self.postprocess_resources {
            r.styles.record_touches(false);
        }
        self.frame_taps.take().map(|t| t.finish())
    }

    /// The frame being captured starts again: a `prepare` begins it.
    pub(super) fn reset_frame_taps(&mut self) {
        if let Some(taps) = &mut self.frame_taps {
            taps.reset();
        }
    }

    /// Before a view's frame is recorded: the style chain notes the targets
    /// each style asks for while the frame is captured.
    pub(super) fn begin_view_taps(&self, scopes: &FrameScopes) {
        if let Some(r) = &self.postprocess_resources {
            r.styles.record_touches(scopes.taps.is_some());
        }
    }

    /// Copy every target the last logged pass named `label` wrote, as the
    /// pass left it. A shadow map is read a layer at a time, each into the
    /// layer's own pass. Nothing unless the frame is captured.
    pub(super) fn tap_pass(
        &self,
        device: &wgpu::Device,
        scopes: &mut FrameScopes,
        encoder: &mut wgpu::CommandEncoder,
        label: &str,
    ) {
        let (Some(taps), Some(log)) = (scopes.taps.as_deref_mut(), scopes.log.as_deref_mut())
        else {
            return;
        };
        let Some(pass) = log.named(label) else {
            return;
        };
        if !pass.pass.ran {
            return;
        }
        for id in pass.pass.writes.clone() {
            if id == "shadow_map" && !pass.passes.is_empty() {
                let Some(sp) = &self.shadow_pass else {
                    continue;
                };
                for layer in pass.passes.iter_mut().filter(|p| p.pass.ran) {
                    let Some(n) = render_passes::SHADOW_PASS_LABELS
                        .iter()
                        .position(|l| *l == layer.pass.name)
                    else {
                        continue;
                    };
                    taps.copy(
                        device,
                        encoder,
                        layer,
                        "shadow_map",
                        None,
                        &sp.shadow_texture,
                        Some(n as u32),
                    );
                }
                continue;
            }
            match self.tap_texture(&taps.view, &id) {
                Ok(texture) => taps.copy(device, encoder, pass, &id, None, &texture, None),
                Err(why) => taps.untapped(label, &id, why),
            }
        }
    }

    /// Copy the target `id` into the pass named `label` under `step` (a
    /// bloom step), while the frame is captured.
    pub(super) fn tap_step(
        &self,
        device: &wgpu::Device,
        scopes: &mut FrameScopes,
        encoder: &mut wgpu::CommandEncoder,
        label: &str,
        id: &str,
        step: &str,
    ) {
        let (Some(taps), Some(log)) = (scopes.taps.as_deref_mut(), scopes.log.as_deref_mut())
        else {
            return;
        };
        let Some(pass) = log.named(label) else {
            return;
        };
        match self.tap_texture(&taps.view, id) {
            Ok(texture) => taps.copy(device, encoder, pass, id, Some(step), &texture, None),
            Err(why) => taps.untapped(label, id, why),
        }
    }

    /// A style chain part (a style, or the shared edges) run in its scope
    /// and, while the frame is captured, read as it ends: `dst`, the target
    /// the plan has it write, then every target of the chain's table it
    /// asked for while it drew that no earlier pass gave (a style's own
    /// intermediates and history, a user style's targets), then every
    /// texture the style holds itself that it noted in `held` as it drew
    /// (#1161: ASCII's edge cells and their history).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn run_style_part(
        &self,
        device: &wgpu::Device,
        scopes: &mut FrameScopes,
        label: &'static str,
        encoder: &mut wgpu::CommandEncoder,
        run: &mut dyn FnMut(&mut wgpu::CommandEncoder),
        dst: Option<&str>,
        refuse_repeat: Option<&str>,
        held: Option<&crate::stylize::HeldTargets>,
    ) {
        if scopes.taps.is_none() {
            scopes.run_refusing(label, encoder, run, refuse_repeat);
            return;
        }
        let Some(resources) = &self.postprocess_resources else {
            scopes.run_refusing(label, encoder, run, refuse_repeat);
            return;
        };
        // What the part asks for, from here
        resources.styles.take_touched();
        if let Some(h) = held {
            h.begin();
        }
        scopes.run_refusing(label, encoder, run, refuse_repeat);
        let touched = resources.styles.take_touched();
        let held = held.map(|h| h.end()).unwrap_or_default();
        if let Some(dst) = dst {
            scopes.note(label, |p| {
                p.writes(&[dst]);
            });
        }
        let (Some(taps), Some(log)) = (scopes.taps.as_deref_mut(), scopes.log.as_deref_mut())
        else {
            return;
        };
        for (id, texture) in &held {
            if !log.targets.iter().any(|t| t.id == *id) {
                log.targets.push(crate::frame_capture::target(id, texture));
            }
        }
        let Some(pass) = log.named(label) else {
            return;
        };
        if let Some(dst) = dst {
            match self.tap_texture(&taps.view, dst) {
                Ok(texture) => taps.copy(device, encoder, pass, dst, None, &texture, None),
                Err(why) => taps.untapped(label, dst, why),
            }
        }
        for name in touched {
            if Some(name) == dst || taps.was_tapped(name) {
                continue;
            }
            if let Some(texture) = resources.styles.texture(name) {
                pass.writes(&[name]);
                taps.copy(device, encoder, pass, name, None, &texture, None);
            }
        }
        for (id, texture) in held {
            pass.writes(&[id]);
            taps.copy(device, encoder, pass, id, None, &texture, None);
        }
    }

    /// The texture a description's target id names in the view being drawn,
    /// or why there is none to read.
    fn tap_texture(&self, view: &CaptureTargets, id: &str) -> Result<wgpu::Texture, &'static str> {
        let host = |t: &Option<wgpu::Texture>| t.clone().ok_or("the host didn't hand it over");
        match id {
            "surface" => return host(&view.surface),
            "depth" => return host(&view.depth),
            "scene_depth" => {
                return self
                    .scaled_depth
                    .as_ref()
                    .map(|d| d.texture.clone())
                    .ok_or("no scaled depth");
            }
            "shadow_map" => {
                return self
                    .shadow_pass
                    .as_ref()
                    .map(|s| s.shadow_texture.clone())
                    .ok_or("no shadow pass");
            }
            "grass_instances" => return Err("a buffer, not a texture"),
            "msaa_color" | "msaa_depth" => {
                return Err("multisampled: its resolve is read");
            }
            "ocean_grab_color" => {
                return self
                    .water
                    .grab
                    .color
                    .as_ref()
                    .map(|(t, _)| t.clone())
                    .ok_or("no grab");
            }
            "ocean_grab_depth" => {
                return self
                    .water
                    .grab
                    .depth
                    .as_ref()
                    .map(|(t, _)| t.clone())
                    .ok_or("no grab");
            }
            // A replay's overdraw view (#1203)
            "overdraw_count" => {
                return self
                    .overdraw_textures()
                    .map(|(count, _)| count.clone())
                    .ok_or("no overdraw drawn");
            }
            "overdraw" => {
                return self
                    .overdraw_textures()
                    .map(|(_, heat)| heat.clone())
                    .ok_or("no overdraw drawn");
            }
            _ => {}
        }
        let r = self.postprocess_resources.as_ref().ok_or("no HDR chain")?;
        let texture = match id {
            "hdr_scene" => Some(&r.hdr_texture),
            "ssao" => Some(&r.ssao_texture),
            "ssao_blur" => Some(&r.ssao_blur_texture),
            "volumetric" => Some(&r.volumetric_texture),
            "volumetric_blur" => Some(&r.volumetric_blur_texture),
            "fxaa_in" => r.fxaa.as_ref().map(|f| &f.texture),
            "fog_veil_behind" => r.fog_veil.as_ref().map(|v| &v.behind),
            "fog_veil" => r.fog_veil.as_ref().map(|v| &v.veil),
            "selection_mask" => {
                return self
                    .selection_outline
                    .as_ref()
                    .and_then(|o| o.mask_texture())
                    .cloned()
                    .ok_or("no selection mask");
            }
            _ => None,
        };
        if let Some(t) = texture {
            return Ok(t.clone());
        }
        if let Some(mip) = id
            .strip_prefix("bloom_mip_")
            .and_then(|n| n.parse::<usize>().ok())
        {
            return r
                .bloom_mips
                .get(mip)
                .map(|m| m.texture.clone())
                .ok_or("no such bloom mip");
        }
        r.styles
            .texture(id)
            .ok_or("not a texture the renderer holds")
    }
}

impl SceneRenderer {
    /// Record the renderer's replay state for the frame being recorded
    /// (#1086): copy every bone, shadow and echo palette into staging in
    /// `encoder` (after the frame's last draw, so they hold what it drew
    /// with), and take the rest from the CPU. `world` names the entities.
    /// Map it once the frame is submitted; see the module docs.
    pub fn capture_replay_state(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        world: &FlintWorld,
    ) -> ReplayStateCapture {
        let name = |id: flint_core::EntityId| {
            world
                .get_name(id)
                .map(str::to_string)
                .unwrap_or_else(|| format!("#{}", id.0))
        };
        let skinning = &self.skinning;
        let mut capture = ReplayStateCapture::new(ReplayRenderer::default());
        let mut stage = |entry: PaletteEntry, source: &wgpu::Buffer| {
            capture.stage(device, encoder, entry, source);
        };
        // By name, so two runs list them alike
        let by_name = |ids: Vec<flint_core::EntityId>| {
            let mut named: Vec<_> = ids.into_iter().map(|id| (name(id), id)).collect();
            named.sort();
            named
        };
        for (entity, id) in by_name(skinning.bone_buffers.keys().copied().collect()) {
            let entry = PaletteEntry {
                entity,
                kind: PaletteKind::Bones,
                ..Default::default()
            };
            stage(entry, &skinning.bone_buffers[&id]);
        }
        for (entity, id) in by_name(skinning.shadow_bone_buffers.keys().copied().collect()) {
            let entry = PaletteEntry {
                entity,
                kind: PaletteKind::Shadow,
                ..Default::default()
            };
            stage(entry, &skinning.shadow_bone_buffers[&id]);
        }
        for (entity, id) in by_name(skinning.echoes.keys().copied().collect()) {
            for (i, echo) in skinning.echoes[&id].iter().enumerate() {
                let entry = PaletteEntry {
                    entity: entity.clone(),
                    kind: PaletteKind::Echo,
                    echo: Some(i),
                    opacity: Some(echo.opacity),
                    ..Default::default()
                };
                stage(entry, &echo.bones);
            }
        }

        let config = self.effective_post_process_config();
        let styles = config
            .styles
            .iter()
            .map(|(n, s)| {
                (
                    n.to_string(),
                    StyleState {
                        enabled: s.enabled,
                        params: s.params.clone(),
                        options: s.options.clone(),
                        order: s.order,
                        feeds_bloom: s.feeds_bloom,
                        blend: s.blend,
                        follows_tear: s.follows_tear,
                        off_everywhere: s.off_everywhere,
                        reveal: (!s.reveal.is_empty()).then(|| format!("{:?}", s.reveal)),
                    },
                )
            })
            .collect();
        let mut echo_lighten: Vec<String> =
            skinning.echo_lighten.iter().map(|id| name(*id)).collect();
        echo_lighten.sort();
        let renderer = ReplayRenderer {
            styles,
            post_process_debug: format!("{config:#?}"),
            // The shadow map's real size: a host may set it apart from the
            // quality (`flint render --shadow-resolution`)
            quality: self
                .quality
                .entries()
                .into_iter()
                .map(|(k, v)| match k {
                    "shadow_resolution" => (k.to_string(), self.shadow_resolution().to_string()),
                    _ => (k.to_string(), v.to_string()),
                })
                .collect(),
            msaa: self.pipelines.samples(),
            render_scale: self.render_scale,
            debug_mode: format!("{:?}", self.debug_state.mode),
            selected: self.selected_entity.map(name),
            grid: self.show_grid(),
            motion_hold: self.motion_hold.state(&name),
            skinning: Vec::new(),
            // Only while a draw binds an asset's shared palette (an entity
            // before its first own write); it is its owner's, the lowest id
            // that wrote the asset (#1177), so the same every run
            asset_bones: if skinning.bone_binds.values().any(|b| !b.from_entity) {
                skinning
                    .asset_bone_matrices
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            } else {
                Default::default()
            },
            part_tints: by_entity(&skinning.part_tints, &name),
            part_softness: by_entity(&skinning.part_softness, &name),
            echo_lighten,
            // Below, from the styles
            style_history: Vec::new(),
        };
        capture.renderer = renderer;
        // Each style's frame-to-frame history (#1136): the textures it read
        // as last frame's, which the frame left as they were
        if let Some(pp) = &self.postprocess_pipeline {
            let frame = self
                .style_frame_serial
                .load(std::sync::atomic::Ordering::Relaxed)
                .wrapping_sub(1);
            let (states, staged) = pp.styles.export_history(device, encoder, frame);
            capture.stage_history(states, staged);
        }
        capture
    }
}

impl SceneRenderer {
    /// Hand each style its history taken from a capture (#1136,
    /// [`crate::stylize::history`]; read with
    /// [`read_style_history`](crate::frame_replay::read_style_history)):
    /// each keeps its own until the next frame it draws, and draws that
    /// frame from it where the frame's key is the captured one. Call after
    /// [`reset_style_history`](Self::reset_style_history), which a taken
    /// history overrides. What the styles did with it is
    /// [`take_style_history_notes`](Self::take_style_history_notes)'s.
    pub fn import_style_history(
        &mut self,
        histories: Vec<crate::stylize::history::ImportedHistory>,
    ) {
        if let Some(pp) = &mut self.postprocess_pipeline {
            pp.styles.import_history(histories);
        }
    }

    /// What the styles said of the history handed to them since the last
    /// call, a line each: taken, or why it was drawn fresh.
    pub fn take_style_history_notes(&mut self) -> Vec<String> {
        self.postprocess_pipeline
            .as_mut()
            .map(|pp| pp.styles.take_history_notes())
            .unwrap_or_default()
    }
}

impl SceneRenderer {
    /// Put back what [`capture_replay_state`](Self::capture_replay_state)
    /// took (#1089, ADR 0126 decision 4), into a renderer whose scene is
    /// bound and loaded: every bone, shadow and echo palette from
    /// `skinning` (`skinning.bin`'s bytes, indexed by `state.skinning`), the
    /// asset palettes, part tints and softness, which echoes lighten, the
    /// motion hold's kept matrices and ticks, the selected entity and the
    /// floor grid. Entities are found in `world` by name; what names none is returned,
    /// a line each. Quality, MSAA, render scale and the config are the
    /// host's to set, as a load does.
    pub fn restore_replay_state(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        world: &FlintWorld,
        state: &ReplayRenderer,
        skinning: &[u8],
    ) -> Vec<String> {
        let mut unknown = std::collections::BTreeSet::new();
        let mut id_of = |name: &str| {
            let id = world.get_id(name);
            if id.is_none() {
                unknown.insert(name.to_string());
            }
            id
        };
        let palette = |e: &PaletteEntry| -> Option<Vec<BoneMatrix>> {
            let start = e.offset as usize;
            let end = start + e.count as usize * std::mem::size_of::<BoneMatrix>();
            skinning
                .get(start..end)
                .map(bytemuck::pod_collect_to_vec::<u8, BoneMatrix>)
        };
        let mut short = 0usize;
        for e in &state.skinning {
            let (Some(id), Some(m)) = (id_of(&e.entity), palette(e)) else {
                short += palette(e).is_none() as usize;
                continue;
            };
            match e.kind {
                PaletteKind::Bones => {
                    self.write_entity_bones(device, queue, id, 0, &m);
                    // The CPU copy the style scissor bounds with
                    let kept = &m[..m.len().min(crate::skinned_pipeline::MAX_BONES)];
                    self.skinning.bone_matrices.insert(id, kept.to_vec());
                }
                PaletteKind::Shadow => self.write_entity_shadow_bones(device, queue, id, &m),
                PaletteKind::Echo => {
                    let opacity = e.opacity.unwrap_or(1.0);
                    self.write_entity_echo_bones(
                        device,
                        queue,
                        id,
                        e.echo.unwrap_or(0),
                        opacity,
                        0,
                        &m,
                    );
                }
            }
        }
        for (asset, m) in &state.asset_bones {
            self.skinning
                .asset_bone_matrices
                .insert(asset.clone(), m.clone());
            if let Some(mesh) = self
                .mesh_cache
                .get_skinned(asset)
                .and_then(|meshes| meshes.first())
            {
                queue.write_buffer_counted(&mesh.bone_buffer, 0, bytemuck::cast_slice(m));
            }
        }
        for (name, tints) in &state.part_tints {
            if let Some(id) = id_of(name) {
                for (part, tint) in tints.iter().enumerate() {
                    self.set_entity_part_tint(id, part, *tint);
                }
            }
        }
        for (name, soft) in &state.part_softness {
            if let Some(id) = id_of(name) {
                for (part, s) in soft.iter().enumerate() {
                    self.set_entity_part_softness(id, part, *s);
                }
            }
        }
        for name in &state.echo_lighten {
            if let Some(id) = id_of(name) {
                self.set_entity_echo_lighten(id, true);
            }
        }
        self.motion_hold.restore(&state.motion_hold, &mut id_of);
        let selected = state.selected.as_deref().and_then(&mut id_of);
        self.set_selected_entity(selected);
        // The floor grid, drawn in the scene pass (#1211)
        self.set_show_grid(device, state.grid);
        let mut said: Vec<String> = unknown
            .into_iter()
            .map(|n| format!("the capture's renderer state names {n:?}, which the replay's world has no entity of"))
            .collect();
        if short > 0 {
            said.push(format!(
                "{short} bone palettes run past the end of skinning.bin"
            ));
        }
        said
    }
}

/// A per-entity table of the skinning state, by entity name.
fn by_entity<T: Clone>(
    map: &HashMap<flint_core::EntityId, Vec<T>>,
    name: &dyn Fn(flint_core::EntityId) -> String,
) -> std::collections::BTreeMap<String, Vec<T>> {
    map.iter().map(|(id, v)| (name(*id), v.clone())).collect()
}
