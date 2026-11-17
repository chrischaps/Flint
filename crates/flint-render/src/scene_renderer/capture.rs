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
use crate::frame_taps::{CaptureTargets, FrameCapture, FrameTaps};

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
    /// intermediates and history, a user style's targets).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn run_style_part(
        &self,
        device: &wgpu::Device,
        scopes: &mut FrameScopes,
        label: &'static str,
        encoder: &mut wgpu::CommandEncoder,
        run: &mut dyn FnMut(&mut wgpu::CommandEncoder),
        dst: Option<&str>,
    ) {
        if scopes.taps.is_none() {
            scopes.run(label, encoder, run);
            return;
        }
        let Some(resources) = &self.postprocess_resources else {
            scopes.run(label, encoder, run);
            return;
        };
        // What the part asks for, from here
        resources.styles.take_touched();
        scopes.run(label, encoder, run);
        let touched = resources.styles.take_touched();
        if let Some(dst) = dst {
            scopes.note(label, |p| {
                p.writes(&[dst]);
            });
        }
        let (Some(taps), Some(log)) = (scopes.taps.as_deref_mut(), scopes.log.as_deref_mut())
        else {
            return;
        };
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
