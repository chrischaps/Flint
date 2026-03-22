use std::path::PathBuf;
use flint_terrain::GrassConfig;
use flint_scene::SceneDocument;
use crate::DebugPanel;

// ---------------------------------------------------------------------------
// Helper widgets
// ---------------------------------------------------------------------------

/// Single labeled drag value. Returns true if the value changed.
fn drag_f32(ui: &mut egui::Ui, label: &str, value: &mut f32, speed: f64, range: std::ops::RangeInclusive<f64>) -> bool {
    let before = *value;
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(
            egui::DragValue::new(value)
                .speed(speed)
                .range(range)
                .max_decimals(3),
        );
    });
    *value != before
}

/// Labeled [f32; 3] drag row with R/G/B prefixes (for colors). Returns true if any element changed.
fn drag_vec3(ui: &mut egui::Ui, label: &str, value: &mut [f32; 3], speed: f64, range: std::ops::RangeInclusive<f64>) -> bool {
    let before = *value;
    let [r, g, b] = value;
    ui.horizontal(|ui| {
        ui.label(label);
        ui.label("R");
        ui.add(egui::DragValue::new(r).speed(speed).range(range.clone()).max_decimals(3));
        ui.label("G");
        ui.add(egui::DragValue::new(g).speed(speed).range(range.clone()).max_decimals(3));
        ui.label("B");
        ui.add(egui::DragValue::new(b).speed(speed).range(range.clone()).max_decimals(3));
    });
    before != [*r, *g, *b]
}

/// Labeled [f32; 3] drag row with X/Y/Z prefixes (for directions). Returns true if any element changed.
fn drag_xyz(ui: &mut egui::Ui, label: &str, value: &mut [f32; 3], speed: f64, range: std::ops::RangeInclusive<f64>) -> bool {
    let before = *value;
    let [x, y, z] = value;
    ui.horizontal(|ui| {
        ui.label(label);
        ui.label("X");
        ui.add(egui::DragValue::new(x).speed(speed).range(range.clone()).max_decimals(3));
        ui.label("Y");
        ui.add(egui::DragValue::new(y).speed(speed).range(range.clone()).max_decimals(3));
        ui.label("Z");
        ui.add(egui::DragValue::new(z).speed(speed).range(range.clone()).max_decimals(3));
    });
    before != [*x, *y, *z]
}

// ---------------------------------------------------------------------------
// Panel struct
// ---------------------------------------------------------------------------

pub struct GrassDebugPanel {
    config: GrassConfig,
    original: GrassConfig,
    scene_path: PathBuf,
    terrain_entity_name: String,
    open: bool,
    dirty: bool,
    density_changed: bool,
}

impl GrassDebugPanel {
    pub fn new(config: GrassConfig, scene_path: PathBuf, terrain_entity_name: String) -> Self {
        Self {
            original: config.clone(),
            config,
            scene_path,
            terrain_entity_name,
            open: false,
            dirty: false,
            density_changed: false,
        }
    }

    /// Read-only access to the working config for the player to push to the renderer.
    pub fn config(&self) -> &GrassConfig {
        &self.config
    }

    /// Whether the density field specifically changed (requires buffer reallocation).
    pub fn density_changed(&self) -> bool {
        self.density_changed
    }

    /// Clear the density_changed flag after the player has handled reallocation.
    pub fn clear_density_changed(&mut self) {
        self.density_changed = false
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Format the current config as `"grass.*" = value` TOML snippet.
    fn format_toml(&self) -> String {
        let c = &self.config;
        let [rb, gb, bb] = c.color_base;
        let [rt, gt, bt] = c.color_tip;
        let [rd, gd, bd] = c.color_dry;
        let [wx, wy, wz] = c.wind_direction;
        format!(
            "\"grass.enabled\" = {enabled}\n\
             \"grass.density\" = {density:.1}\n\
             \"grass.max_distance\" = {max_distance:.1}\n\
             \"grass.fade_start\" = {fade_start:.1}\n\
             \"grass.density_threshold\" = {density_threshold}\n\
             \"grass.blade_width\" = {blade_width}\n\
             \"grass.blade_height\" = {blade_height}\n\
             \"grass.height_variation\" = {height_variation}\n\
             \"grass.color_base\" = [{rb}, {gb}, {bb}]\n\
             \"grass.color_tip\" = [{rt}, {gt}, {bt}]\n\
             \"grass.color_dry\" = [{rd}, {gd}, {bd}]\n\
             \"grass.dry_amount\" = {dry_amount}\n\
             \"grass.wind_direction\" = [{wx}, {wy}, {wz}]\n\
             \"grass.wind_speed\" = {wind_speed}\n\
             \"grass.wind_strength\" = {wind_strength}\n\
             \"grass.bend_radius\" = {bend_radius}\n\
             \"grass.bend_strength\" = {bend_strength}\n",
            enabled = c.enabled,
            density = c.density,
            max_distance = c.max_distance,
            fade_start = c.fade_start,
            density_threshold = c.density_threshold,
            blade_width = c.blade_width,
            blade_height = c.blade_height,
            height_variation = c.height_variation,
            rb = rb, gb = gb, bb = bb,
            rt = rt, gt = gt, bt = bt,
            rd = rd, gd = gd, bd = bd,
            dry_amount = c.dry_amount,
            wx = wx, wy = wy, wz = wz,
            wind_speed = c.wind_speed,
            wind_strength = c.wind_strength,
            bend_radius = c.bend_radius,
            bend_strength = c.bend_strength,
        )
    }

    /// Patch all grass fields in the scene file and save.
    fn commit_to_file(&self) -> Result<(), String> {
        let entity = &self.terrain_entity_name;
        let c = &self.config;

        // Conversion helpers
        let f = |v: f32| toml::Value::Float(v as f64);
        let b = |v: bool| toml::Value::Boolean(v);
        let v3 = |arr: [f32; 3]| {
            toml::Value::Array(vec![
                toml::Value::Float(arr[0] as f64),
                toml::Value::Float(arr[1] as f64),
                toml::Value::Float(arr[2] as f64),
            ])
        };

        let mut doc = SceneDocument::from_file(&self.scene_path)?;

        doc.patch_field(entity, "terrain", "grass.enabled",           &b(c.enabled))?;
        doc.patch_field(entity, "terrain", "grass.density",           &f(c.density))?;
        doc.patch_field(entity, "terrain", "grass.max_distance",      &f(c.max_distance))?;
        doc.patch_field(entity, "terrain", "grass.fade_start",        &f(c.fade_start))?;
        doc.patch_field(entity, "terrain", "grass.density_threshold", &f(c.density_threshold))?;
        doc.patch_field(entity, "terrain", "grass.blade_width",       &f(c.blade_width))?;
        doc.patch_field(entity, "terrain", "grass.blade_height",      &f(c.blade_height))?;
        doc.patch_field(entity, "terrain", "grass.height_variation",  &f(c.height_variation))?;
        doc.patch_field(entity, "terrain", "grass.color_base",        &v3(c.color_base))?;
        doc.patch_field(entity, "terrain", "grass.color_tip",         &v3(c.color_tip))?;
        doc.patch_field(entity, "terrain", "grass.color_dry",         &v3(c.color_dry))?;
        doc.patch_field(entity, "terrain", "grass.dry_amount",        &f(c.dry_amount))?;
        doc.patch_field(entity, "terrain", "grass.wind_direction",    &v3(c.wind_direction))?;
        doc.patch_field(entity, "terrain", "grass.wind_speed",        &f(c.wind_speed))?;
        doc.patch_field(entity, "terrain", "grass.wind_strength",     &f(c.wind_strength))?;
        doc.patch_field(entity, "terrain", "grass.bend_radius",       &f(c.bend_radius))?;
        doc.patch_field(entity, "terrain", "grass.bend_strength",     &f(c.bend_strength))?;

        doc.save(&self.scene_path)
    }
}

// ---------------------------------------------------------------------------
// DebugPanel impl
// ---------------------------------------------------------------------------

impl DebugPanel for GrassDebugPanel {
    fn name(&self) -> &str { "Grass Debug" }

    fn ui(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;

        // --- Enable toggle ---
        let before_enabled = self.config.enabled;
        ui.checkbox(&mut self.config.enabled, "Enabled");
        if self.config.enabled != before_enabled {
            changed = true;
        }

        ui.separator();

        // --- Distribution ---
        egui::CollapsingHeader::new("Distribution")
            .default_open(true)
            .show(ui, |ui| {
                let before_density = self.config.density;
                if drag_f32(ui, "Density", &mut self.config.density, 0.1, 0.1..=50.0) {
                    changed = true;
                    if (self.config.density - before_density).abs() > f32::EPSILON {
                        self.density_changed = true;
                    }
                }
                if drag_f32(ui, "Max Distance", &mut self.config.max_distance, 1.0, 10.0..=500.0) {
                    changed = true;
                }
                if drag_f32(ui, "Fade Start", &mut self.config.fade_start, 1.0, 5.0..=500.0) {
                    changed = true;
                }
                if drag_f32(ui, "Density Threshold", &mut self.config.density_threshold, 0.005, 0.0..=1.0) {
                    changed = true;
                }
            });

        // --- Blade Geometry ---
        egui::CollapsingHeader::new("Blade Geometry")
            .default_open(true)
            .show(ui, |ui| {
                if drag_f32(ui, "Blade Width", &mut self.config.blade_width, 0.001, 0.01..=1.0) {
                    changed = true;
                }
                if drag_f32(ui, "Blade Height", &mut self.config.blade_height, 0.005, 0.01..=2.0) {
                    changed = true;
                }
                if drag_f32(ui, "Height Variation", &mut self.config.height_variation, 0.005, 0.0..=1.0) {
                    changed = true;
                }
            });

        // --- Colors ---
        egui::CollapsingHeader::new("Colors")
            .default_open(true)
            .show(ui, |ui| {
                if drag_vec3(ui, "Base Color", &mut self.config.color_base, 0.005, 0.0..=1.0) {
                    changed = true;
                }
                if drag_vec3(ui, "Tip Color", &mut self.config.color_tip, 0.005, 0.0..=1.0) {
                    changed = true;
                }
                if drag_vec3(ui, "Dry Color", &mut self.config.color_dry, 0.005, 0.0..=1.0) {
                    changed = true;
                }
                if drag_f32(ui, "Dry Amount", &mut self.config.dry_amount, 0.005, 0.0..=1.0) {
                    changed = true;
                }
            });

        // --- Wind ---
        egui::CollapsingHeader::new("Wind")
            .default_open(true)
            .show(ui, |ui| {
                if drag_xyz(ui, "Wind Direction", &mut self.config.wind_direction, 0.01, -1.0..=1.0) {
                    changed = true;
                }
                if drag_f32(ui, "Wind Speed", &mut self.config.wind_speed, 0.05, 0.0..=10.0) {
                    changed = true;
                }
                if drag_f32(ui, "Wind Strength", &mut self.config.wind_strength, 0.005, 0.0..=1.0) {
                    changed = true;
                }
            });

        // --- Bend ---
        egui::CollapsingHeader::new("Bend")
            .default_open(true)
            .show(ui, |ui| {
                if drag_f32(ui, "Bend Radius", &mut self.config.bend_radius, 0.1, 0.0..=20.0) {
                    changed = true;
                }
                if drag_f32(ui, "Bend Strength", &mut self.config.bend_strength, 0.01, 0.0..=2.0) {
                    changed = true;
                }
            });

        if changed {
            self.dirty = true;
        }

        ui.separator();

        // --- Bottom toolbar ---
        ui.horizontal(|ui| {
            if ui.button("Reset").clicked() {
                let density_was_different = self.config.density != self.original.density;
                self.config = self.original.clone();
                self.dirty = true;
                self.density_changed = density_was_different;
            }

            if ui.button("Copy TOML").clicked() {
                let snippet = self.format_toml();
                ui.output_mut(|o| o.copied_text = snippet);
            }

            if ui.button("Commit to File").clicked() {
                if let Err(e) = self.commit_to_file() {
                    tracing::error!("GrassDebugPanel: commit_to_file failed: {}", e);
                }
            }
        });
    }

    fn is_open(&self) -> bool { self.open }
    fn toggle(&mut self) { self.open = !self.open; }
    fn is_dirty(&self) -> bool { self.dirty }
    fn clear_dirty(&mut self) { self.dirty = false; }
    fn as_any(&self) -> &dyn std::any::Any { self }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
}
