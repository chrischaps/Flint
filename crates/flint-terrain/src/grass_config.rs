//! Grass configuration — parsed from `[grass]` TOML section

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DensitySource {
    Splat,
    Map,
}

impl Default for DensitySource {
    fn default() -> Self { Self::Splat }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrassConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_density")]
    pub density: f32,
    #[serde(default = "default_max_distance")]
    pub max_distance: f32,
    #[serde(default = "default_fade_start")]
    pub fade_start: f32,
    #[serde(default = "default_blade_width")]
    pub blade_width: f32,
    #[serde(default = "default_blade_height")]
    pub blade_height: f32,
    #[serde(default = "default_height_variation")]
    pub height_variation: f32,
    #[serde(default = "default_color_base")]
    pub color_base: [f32; 3],
    #[serde(default = "default_color_tip")]
    pub color_tip: [f32; 3],
    #[serde(default = "default_color_dry")]
    pub color_dry: [f32; 3],
    #[serde(default = "default_dry_amount")]
    pub dry_amount: f32,
    #[serde(default = "default_wind_direction")]
    pub wind_direction: [f32; 3],
    #[serde(default = "default_wind_speed")]
    pub wind_speed: f32,
    #[serde(default = "default_wind_strength")]
    pub wind_strength: f32,
    #[serde(default = "default_bend_radius")]
    pub bend_radius: f32,
    #[serde(default = "default_bend_strength")]
    pub bend_strength: f32,
    #[serde(default)]
    pub density_source: DensitySource,
    #[serde(default)]
    pub density_layer: u32,
    #[serde(default = "default_density_threshold")]
    pub density_threshold: f32,
}

impl Default for GrassConfig {
    fn default() -> Self {
        Self {
            enabled: false, density: default_density(), max_distance: default_max_distance(),
            fade_start: default_fade_start(), blade_width: default_blade_width(),
            blade_height: default_blade_height(), height_variation: default_height_variation(),
            color_base: default_color_base(), color_tip: default_color_tip(),
            color_dry: default_color_dry(), dry_amount: default_dry_amount(),
            wind_direction: default_wind_direction(), wind_speed: default_wind_speed(),
            wind_strength: default_wind_strength(), bend_radius: default_bend_radius(),
            bend_strength: default_bend_strength(), density_source: DensitySource::default(),
            density_layer: 0, density_threshold: default_density_threshold(),
        }
    }
}

impl GrassConfig {
    pub fn max_instances(&self, terrain_width: f32, terrain_depth: f32) -> u32 {
        let area = terrain_width * terrain_depth;
        (self.density * area * 0.5).ceil() as u32
    }
}

fn default_density() -> f32 { 8.0 }
fn default_max_distance() -> f32 { 80.0 }
fn default_fade_start() -> f32 { 60.0 }
fn default_blade_width() -> f32 { 0.08 }
fn default_blade_height() -> f32 { 0.4 }
fn default_height_variation() -> f32 { 0.3 }
fn default_color_base() -> [f32; 3] { [0.15, 0.45, 0.1] }
fn default_color_tip() -> [f32; 3] { [0.3, 0.7, 0.15] }
fn default_color_dry() -> [f32; 3] { [0.55, 0.5, 0.2] }
fn default_dry_amount() -> f32 { 0.15 }
fn default_wind_direction() -> [f32; 3] { [1.0, 0.0, 0.3] }
fn default_wind_speed() -> f32 { 1.0 }
fn default_wind_strength() -> f32 { 0.15 }
fn default_bend_radius() -> f32 { 2.0 }
fn default_bend_strength() -> f32 { 0.8 }
fn default_density_threshold() -> f32 { 0.1 }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_grass_disabled() {
        let cfg = GrassConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.density, 8.0);
        assert_eq!(cfg.density_source, DensitySource::Splat);
    }

    #[test]
    fn parse_minimal_grass_config() {
        let toml_str = r#"enabled = true"#;
        let cfg: GrassConfig = toml::from_str(toml_str).unwrap();
        assert!(cfg.enabled);
        // All other fields should be defaults
        assert_eq!(cfg.density, 8.0);
        assert_eq!(cfg.max_distance, 80.0);
        assert_eq!(cfg.fade_start, 60.0);
        assert_eq!(cfg.blade_width, 0.08);
        assert_eq!(cfg.blade_height, 0.4);
        assert_eq!(cfg.density_source, DensitySource::Splat);
        assert_eq!(cfg.density_layer, 0);
    }

    #[test]
    fn parse_full_grass_config() {
        let toml_str = r#"
enabled = true
density = 12.0
max_distance = 100.0
fade_start = 75.0
blade_width = 0.1
blade_height = 0.6
height_variation = 0.5
color_base = [0.1, 0.4, 0.05]
color_tip = [0.2, 0.6, 0.1]
color_dry = [0.6, 0.55, 0.25]
dry_amount = 0.2
wind_direction = [0.7, 0.0, 0.7]
wind_speed = 1.5
wind_strength = 0.2
bend_radius = 3.0
bend_strength = 0.9
density_source = "map"
density_layer = 2
density_threshold = 0.2
"#;
        let cfg: GrassConfig = toml::from_str(toml_str).unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.density, 12.0);
        assert_eq!(cfg.max_distance, 100.0);
        assert_eq!(cfg.fade_start, 75.0);
        assert_eq!(cfg.blade_width, 0.1);
        assert_eq!(cfg.blade_height, 0.6);
        assert_eq!(cfg.height_variation, 0.5);
        assert_eq!(cfg.color_base, [0.1, 0.4, 0.05]);
        assert_eq!(cfg.color_tip, [0.2, 0.6, 0.1]);
        assert_eq!(cfg.color_dry, [0.6, 0.55, 0.25]);
        assert_eq!(cfg.dry_amount, 0.2);
        assert_eq!(cfg.wind_direction, [0.7, 0.0, 0.7]);
        assert_eq!(cfg.wind_speed, 1.5);
        assert_eq!(cfg.wind_strength, 0.2);
        assert_eq!(cfg.bend_radius, 3.0);
        assert_eq!(cfg.bend_strength, 0.9);
        assert_eq!(cfg.density_source, DensitySource::Map);
        assert_eq!(cfg.density_layer, 2);
        assert_eq!(cfg.density_threshold, 0.2);
    }

    #[test]
    fn max_instances_estimate() {
        let cfg = GrassConfig::default(); // density = 8.0
        // 256 * 256 * 8 * 0.5 = 262144
        assert_eq!(cfg.max_instances(256.0, 256.0), 262144);
    }

    #[test]
    fn round_trip_serialize() {
        let original = GrassConfig {
            enabled: true,
            density: 10.0,
            max_distance: 90.0,
            density_source: DensitySource::Map,
            density_layer: 1,
            ..GrassConfig::default()
        };
        let toml_str = toml::to_string_pretty(&original).unwrap();
        let parsed: GrassConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.enabled, original.enabled);
        assert_eq!(parsed.density, original.density);
        assert_eq!(parsed.max_distance, original.max_distance);
        assert_eq!(parsed.density_source, original.density_source);
        assert_eq!(parsed.density_layer, original.density_layer);
        assert_eq!(parsed.blade_width, original.blade_width);
        assert_eq!(parsed.color_base, original.color_base);
    }
}
