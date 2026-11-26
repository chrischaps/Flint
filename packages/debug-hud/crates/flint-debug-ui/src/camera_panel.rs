//! Camera tuning panel (F3 family).
//!
//! Keys on a `camera_tuning` component (engine schema): field `fov_deg` is
//! the perspective vertical field of view in degrees. The player applies it
//! to the live render camera at scene load and whenever the panel edits it,
//! so games can tune FOV by feel — wide enough to see the foreground, narrow
//! enough to avoid fisheye — then Commit to File to persist the choice.
//! The panel is created only when the component exists.

use crate::DebugPanel;
use flint_ecs::ComponentView;
use flint_scene::SceneDocument;
use std::path::PathBuf;

/// The engine component this panel tunes (`schemas/components/camera_tuning.toml`).
pub const CAMERA_TUNING: &str = "camera_tuning";

/// `camera_tuning.fov_deg` when it isn't written (the schema's default).
const DEFAULT_FOV_DEG: f32 = 65.0;

#[derive(Debug, Clone, PartialEq)]
pub struct CameraPanelConfig {
    pub fov_deg: f32,
}

impl CameraPanelConfig {
    /// Seed the slider from a `camera_tuning` component through the typed
    /// view (ADR 0115, #1068): a `fov_deg` of the wrong type is 65 and is
    /// reported once.
    pub fn from_component(view: &ComponentView<'_>) -> Self {
        Self {
            fov_deg: view.f32("fov_deg", DEFAULT_FOV_DEG),
        }
    }

    pub fn to_fields(&self) -> Vec<(&'static str, toml::Value)> {
        vec![("fov_deg", toml::Value::Float(self.fov_deg as f64))]
    }
}

pub struct CameraDebugPanel {
    config: CameraPanelConfig,
    original: CameraPanelConfig,
    scene_path: PathBuf,
    entity_name: String,
    open: bool,
    dirty: bool,
}

impl CameraDebugPanel {
    pub fn new(config: CameraPanelConfig, scene_path: PathBuf, entity_name: String) -> Self {
        Self {
            original: config.clone(),
            config,
            scene_path,
            entity_name,
            open: false,
            dirty: false,
        }
    }

    pub fn config(&self) -> &CameraPanelConfig {
        &self.config
    }

    pub fn entity_name(&self) -> &str {
        &self.entity_name
    }

    /// Keep the slider tracking the live camera (e.g. a script override)
    /// while not being edited.
    pub fn sync_fov(&mut self, fov_deg: f32) {
        if !self.dirty {
            self.config.fov_deg = fov_deg;
        }
    }

    fn commit_to_file(&self) -> Result<(), String> {
        let mut doc = SceneDocument::from_file(&self.scene_path)?;
        for (field, value) in self.config.to_fields() {
            doc.patch_field(&self.entity_name, CAMERA_TUNING, field, &value)?;
        }
        doc.save(&self.scene_path)
    }
}

fn label_for(fov_deg: f32) -> &'static str {
    match fov_deg {
        f if f < 50.0 => "narrow",
        f if f < 72.0 => "natural",
        f if f < 90.0 => "wide",
        _ => "fisheye",
    }
}

impl DebugPanel for CameraDebugPanel {
    fn name(&self) -> &str {
        "Camera"
    }

    /// An edit drives the live camera and the component (so Commit to File
    /// keeps it); otherwise the slider follows the camera, so a script's
    /// FOV doesn't leave it stale.
    fn sync(&mut self, ctx: &mut crate::PanelCtx) {
        if !self.dirty {
            self.sync_fov(ctx.camera.fov);
            return;
        }
        ctx.camera.fov = self.config.fov_deg;
        if let Some(id) = ctx.world.get_id(&self.entity_name) {
            if let Some(mut comps) = ctx.world.get_components_mut(id) {
                for (field, value) in self.config.to_fields() {
                    comps.set_field(CAMERA_TUNING, field, value);
                }
            }
        }
        self.dirty = false;
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        let before = self.config.fov_deg;
        ui.horizontal(|ui| {
            ui.label("Vertical FOV");
            ui.add(
                egui::Slider::new(&mut self.config.fov_deg, 40.0..=110.0)
                    .fixed_decimals(1)
                    .suffix("°"),
            );
            ui.label(label_for(self.config.fov_deg));
        });
        if (self.config.fov_deg - before).abs() > f32::EPSILON {
            self.dirty = true;
        }

        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("Reset").clicked() {
                self.config = self.original.clone();
                self.dirty = true;
            }
            if ui.button("Commit to File").clicked() {
                if let Err(e) = self.commit_to_file() {
                    tracing::error!("CameraDebugPanel: commit_to_file failed: {}", e);
                }
            }
        });
    }

    fn is_open(&self) -> bool {
        self.open
    }
    fn toggle(&mut self) {
        self.open = !self.open;
    }
    fn is_dirty(&self) -> bool {
        self.dirty
    }
    fn clear_dirty(&mut self) {
        self.dirty = false;
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}
