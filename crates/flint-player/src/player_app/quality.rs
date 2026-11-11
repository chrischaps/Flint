//! Runtime quality settings (FlintKart#55): one path from every source to
//! the renderer. The desktop's `--msaa`, `--render-scale`, `--quality` and
//! `FLINT_SHADOWS` / `FLINT_SHADOW_RES`, the page's `?msaa=`, `?scale=` and
//! `?quality=`, an Android dev profile and a script's `set_quality` all end
//! in [`PlayerApp::apply_quality`], between frames.
//!
//! The render scale stays the player's: the setting's `scale` is the
//! quality's `render_scale`, and `update_render_scale` still caps it by the
//! pixel ratio and steps it under load each frame. Everything else is the
//! renderer's (`SceneRenderer::set_quality`).

use super::PlayerApp;
use flint_render::QualitySettings;

impl PlayerApp {
    /// The settings startup asks for: the MSAA and render-scale fields, the
    /// shadow switches in the environment, then `quality_overrides`.
    fn startup_quality(&self) -> QualitySettings {
        let mut q = QualitySettings {
            render_scale: self.render_scale.scale,
            render_scale_auto: self.render_scale.dynamic,
            msaa: self.msaa_sample_count,
            ..QualitySettings::default()
        };
        // FLINT_SHADOWS=0 starts with the shadow pass off, so a perf run
        // can A/B its cost headlessly.
        if std::env::var("FLINT_SHADOWS").is_ok_and(|v| v == "0") {
            q.shadows = false;
        }
        // FLINT_SHADOW_RES=1024 sizes each cascade's map (FlintKart#54).
        if let Some(res) = std::env::var("FLINT_SHADOW_RES")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
        {
            q.shadow_resolution = res;
        }
        for e in q.apply_str(&self.quality_overrides) {
            tracing::warn!("--quality: {e}");
        }
        q.clamped()
    }

    /// Apply the startup settings and tell scripts what the GPU is. Call
    /// once the renderer exists and before any script's `on_init`.
    pub(super) fn init_quality(&mut self) {
        if let Some(context) = &self.render_context {
            let info = flint_render::gpu_info(&context.adapter);
            tracing::info!(
                "[quality] GPU {:?}: {} {} on {} ({})",
                info.name,
                info.vendor,
                info.device_type,
                info.backend,
                info.class
            );
            self.script.set_gpu_info(info);
        }
        let q = self.startup_quality();
        self.apply_quality(q);
        tracing::info!("[quality] {}", self.quality.to_query());
    }

    /// Apply `q` now. Call between frames: an MSAA change rebuilds the
    /// scene pipelines, a shadow-resolution change the shadow maps.
    pub(crate) fn apply_quality(&mut self, q: QualitySettings) {
        let q = q.clamped();
        if q.render_scale_auto != self.render_scale.dynamic || !q.render_scale_auto {
            // A fresh start under the new ceiling (and none when fixed).
            self.dynamic_scale = Default::default();
        }
        self.render_scale.scale = q.render_scale;
        self.render_scale.dynamic = q.render_scale_auto;
        let mut applied = q;
        if let (Some(context), Some(renderer)) = (&self.render_context, &mut self.scene_renderer) {
            // The scale is update_render_scale's to set (with the pixel
            // ratio cap and the dynamic step); keep the one drawn now so
            // the HDR chain is rebuilt once, there, not twice.
            let r = QualitySettings {
                render_scale: renderer.render_scale(),
                ..q
            };
            applied = QualitySettings {
                render_scale: q.render_scale,
                render_scale_auto: q.render_scale_auto,
                ..renderer.set_quality(&context.device, &r)
            };
        }
        self.quality = applied;
        self.script.set_quality_state(applied);
    }

    /// A script's `set_quality` from this frame, applied before the frame
    /// draws.
    pub(super) fn apply_script_quality(&mut self) {
        if let Some(q) = self.script_quality.take() {
            if q != self.quality {
                tracing::info!("[quality] script: {}", q.clamped().to_query());
                self.apply_quality(q);
            } else {
                // Asked for what's in force; just hand the state back.
                self.script.set_quality_state(self.quality);
            }
        }
    }

    /// After Android brings the surface back: the renderer and its settings
    /// outlive the surface, but the surface may come back another size, so
    /// apply the settings again over the resized chain.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub(super) fn reapply_quality(&mut self) {
        let q = self.quality;
        self.apply_quality(q);
    }

    /// The quality settings in force.
    pub fn quality(&self) -> &QualitySettings {
        &self.quality
    }
}
