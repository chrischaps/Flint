//! Tree generator — produces procedural tree meshes with bark materials.
//!
//! The trunk (`trunk`), the limbs along it (`branches`), leaf cards in
//! season-tinted shades (`leaves`) and levels of detail that simplify the
//! bark and thin the leaves apart (`lod`) make up [`TreeGenerator`].

mod bark;
pub mod branches;
pub mod leaves;
pub mod lod;
mod trunk;

pub use bark::generate_bark_normal_map;
pub use branches::{generate_branches, TrunkSpine};
pub use leaves::generate_leaves;
pub use lod::{generate_lods, join_bark_and_leaves, thin_cards, tree_lod};
pub use trunk::{generate_trunk, TrunkOutput};

use crate::algorithms::lsystem::grammar::{self, Grammar};
use crate::algorithms::mesh_builder::MeshBuilder;
use crate::generator::{GenerationCost, Generator};
use crate::generators::util::{
    parse_hex_color, toml_f32, toml_string, toml_string_array, toml_u32,
};
use crate::output::{GeneratorOutput, OutputKind};
use crate::spec::ProcGenSpec;
use crate::types::{BranchSegment, ImageData, MeshData};
use crate::{ProcGenError, Result, SeededRng};
use flint_core::Vec3;

/// Typed trunk configuration extracted from a `ProcGenSpec`'s params table.
#[derive(Debug, Clone)]
pub struct TrunkParams {
    /// Total trunk height in meters.
    pub height: f32,
    /// Radius at the base of the trunk.
    pub radius_base: f32,
    /// Radius at the top of the trunk.
    pub radius_top: f32,
    /// Number of segments along the spine (vertical resolution).
    pub segments: u32,
    /// Number of radial segments in the cross-section.
    pub radial_segments: u32,
    /// Perlin displacement magnitude for trunk curvature.
    pub curve_noise: f32,
    /// Bark base color as linear RGBA.
    pub bark_color: [f32; 4],
    /// Bark roughness factor.
    pub bark_roughness: f32,
    /// Normal map strength multiplier.
    pub bark_normal_strength: f32,
    /// Normal map resolution (width = height).
    pub bark_normal_resolution: u32,
}

impl Default for TrunkParams {
    fn default() -> Self {
        Self {
            height: 5.0,
            radius_base: 0.3,
            radius_top: 0.1,
            segments: 10,
            radial_segments: 8,
            curve_noise: 0.1,
            bark_color: [0.34, 0.2, 0.1, 1.0],
            bark_roughness: 0.9,
            bark_normal_strength: 1.0,
            bark_normal_resolution: 256,
        }
    }
}

impl TrunkParams {
    /// Parse trunk parameters from a TOML `params` value.
    ///
    /// Missing fields fall back to sensible defaults. Invalid values produce
    /// descriptive errors.
    pub fn from_toml(params: &toml::Value) -> Result<Self> {
        let table = params
            .as_table()
            .ok_or_else(|| ProcGenError::InvalidParameter {
                name: "params".into(),
                reason: "expected a TOML table".into(),
            })?;

        let mut p = Self::default();

        if let Some(v) = table.get("trunk_height") {
            p.height = toml_f32(v, "trunk_height")?;
            if p.height <= 0.0 {
                return Err(ProcGenError::InvalidParameter {
                    name: "trunk_height".into(),
                    reason: "must be positive".into(),
                });
            }
        }

        if let Some(v) = table.get("trunk_radius_base") {
            p.radius_base = toml_f32(v, "trunk_radius_base")?;
            if p.radius_base <= 0.0 {
                return Err(ProcGenError::InvalidParameter {
                    name: "trunk_radius_base".into(),
                    reason: "must be positive".into(),
                });
            }
        }

        if let Some(v) = table.get("trunk_radius_top") {
            p.radius_top = toml_f32(v, "trunk_radius_top")?;
            if p.radius_top < 0.0 {
                return Err(ProcGenError::InvalidParameter {
                    name: "trunk_radius_top".into(),
                    reason: "must be non-negative".into(),
                });
            }
        }

        if let Some(v) = table.get("trunk_segments") {
            p.segments = toml_u32(v, "trunk_segments")?;
            if p.segments < 2 {
                return Err(ProcGenError::InvalidParameter {
                    name: "trunk_segments".into(),
                    reason: "must be at least 2".into(),
                });
            }
        }

        if let Some(v) = table.get("radial_segments") {
            p.radial_segments = toml_u32(v, "radial_segments")?;
            if p.radial_segments < 3 {
                return Err(ProcGenError::InvalidParameter {
                    name: "radial_segments".into(),
                    reason: "must be at least 3".into(),
                });
            }
        }

        if let Some(v) = table.get("trunk_curve_noise") {
            p.curve_noise = toml_f32(v, "trunk_curve_noise")?;
            if p.curve_noise < 0.0 {
                return Err(ProcGenError::InvalidParameter {
                    name: "trunk_curve_noise".into(),
                    reason: "must be non-negative".into(),
                });
            }
        }

        if let Some(v) = table.get("bark_color_base") {
            let hex = v.as_str().ok_or_else(|| ProcGenError::InvalidParameter {
                name: "bark_color_base".into(),
                reason: "expected a hex color string (e.g. \"#8B4513\")".into(),
            })?;
            p.bark_color = parse_hex_color(hex)?;
        }

        if let Some(v) = table.get("bark_roughness") {
            p.bark_roughness = toml_f32(v, "bark_roughness")?;
        }

        if let Some(v) = table.get("bark_normal_strength") {
            p.bark_normal_strength = toml_f32(v, "bark_normal_strength")?;
        }

        if let Some(v) = table.get("bark_normal_resolution") {
            p.bark_normal_resolution = toml_u32(v, "bark_normal_resolution")?;
            if !p.bark_normal_resolution.is_power_of_two() || p.bark_normal_resolution < 16 {
                return Err(ProcGenError::InvalidParameter {
                    name: "bark_normal_resolution".into(),
                    reason: "must be a power of 2 and at least 16".into(),
                });
            }
        }

        Ok(p)
    }
}

// ─── Branch Method & Crown Shape ────────────────────────────────────────────

/// Which branching algorithm to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchMethod {
    /// L-system rewriting with stochastic angle variation.
    LSystem,
    /// Space colonization with attraction point cloud.
    SpaceColonization,
}

/// Shape of the crown volume for space colonization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrownShape {
    Sphere,
    Ellipsoid,
    Hemisphere,
    /// A box `2 * crown_radius` wide and deep, `crown_radius * crown_height_ratio` tall.
    Box,
    /// A cone `crown_radius` wide at its base, narrowing to a point
    /// `crown_radius * crown_height_ratio` above it (conifers).
    Cone,
}

// ─── BranchParams ───────────────────────────────────────────────────────────

/// Typed branch configuration extracted from a `ProcGenSpec`'s params table.
#[derive(Debug, Clone)]
pub struct BranchParams {
    /// Which branching algorithm to use.
    pub method: BranchMethod,
    /// Number of branching depth levels.
    pub branch_levels: u32,
    /// Minimum branching angle in degrees.
    pub branch_angle_min: f32,
    /// Maximum branching angle in degrees.
    pub branch_angle_max: f32,
    /// Length decay factor per depth level.
    pub branch_length_falloff: f32,
    /// Radius decay factor per depth level.
    pub branch_radius_falloff: f32,
    /// Maximum number of branch segments (budget guard).
    pub max_segments: u32,
    /// Base radial segments for branch cross-sections.
    pub radial_segments: u32,
    /// How far up the trunk (0 = root, 1 = tip) the lowest limb leaves it.
    /// The built-in L-system spreads its scaffold limbs from here to the tip,
    /// and space colonization may sprout from any of that stretch. A text
    /// grammar ignores it and grows from the tip.
    pub branch_start: f32,

    // ── Crown ──
    /// Crown volume shape (space colonization).
    pub crown_shape: CrownShape,
    /// Crown radius: the attraction volume's size for space colonization,
    /// and how far the built-in L-system's limbs reach.
    pub crown_radius: f32,
    /// Crown height as a ratio of crown radius (for ellipsoid).
    pub crown_height_ratio: f32,
    /// Number of attraction points.
    pub num_attraction_points: u32,
    /// Kill distance for attraction point removal.
    pub kill_distance: f32,
    /// Maximum influence distance for attraction points.
    pub influence_distance: f32,
    /// Growth step size per iteration.
    pub step_size: f32,
    /// Pipe model exponent for branch radii (2 = da Vinci's rule).
    pub pipe_exponent: f32,

    // ── L-system specific ──
    /// Number of L-system rewriting iterations.
    pub lsystem_iterations: u32,
    /// Angle variation range for stochastic L-system (degrees).
    pub lsystem_angle_variation: f32,
    /// A text grammar (`lsystem_preset`, or `lsystem_axiom` plus
    /// `lsystem_rules`). When set it replaces the built-in branching rule.
    pub lsystem_grammar: Option<Grammar>,
    /// Turtle angle for the grammar, degrees. Defaults to the preset's angle,
    /// else the middle of `branch_angle_min..max`.
    pub lsystem_angle: Option<f32>,
    /// Fixed length of a bare `F`. When unset, the grammar's crown is scaled
    /// to reach `lsystem_size` metres from the trunk tip instead.
    pub lsystem_step: Option<f32>,
    /// Crown reach in metres when `lsystem_step` is unset.
    pub lsystem_size: f32,
    /// Bend toward the ground per step (negative bends upward toward light).
    pub lsystem_tropism: f32,
}

impl Default for BranchParams {
    fn default() -> Self {
        Self {
            method: BranchMethod::LSystem,
            branch_levels: 3,
            branch_angle_min: 20.0,
            branch_angle_max: 40.0,
            branch_length_falloff: 0.7,
            branch_radius_falloff: 0.6,
            max_segments: 5000,
            radial_segments: 6,
            branch_start: 0.6,

            crown_shape: CrownShape::Sphere,
            crown_radius: 3.0,
            crown_height_ratio: 1.5,
            num_attraction_points: 500,
            kill_distance: 0.5,
            influence_distance: 4.0,
            step_size: 0.3,
            pipe_exponent: 2.0,

            lsystem_iterations: 3,
            lsystem_angle_variation: 10.0,
            lsystem_grammar: None,
            lsystem_angle: None,
            lsystem_step: None,
            lsystem_size: 4.0,
            lsystem_tropism: 0.0,
        }
    }
}

impl BranchParams {
    /// Parse branch parameters from a TOML `params` value.
    ///
    /// Missing fields fall back to sensible defaults. Invalid values produce
    /// descriptive errors.
    pub fn from_toml(params: &toml::Value) -> Result<Self> {
        let table = params
            .as_table()
            .ok_or_else(|| ProcGenError::InvalidParameter {
                name: "params".into(),
                reason: "expected a TOML table".into(),
            })?;

        let mut p = Self::default();

        if let Some(v) = table.get("branch_method") {
            let s = toml_string(v, "branch_method")?;
            p.method = match s.as_str() {
                "lsystem" | "l_system" | "LSystem" => BranchMethod::LSystem,
                "space_colonization" | "SpaceColonization" => BranchMethod::SpaceColonization,
                other => {
                    return Err(ProcGenError::InvalidParameter {
                        name: "branch_method".into(),
                        reason: format!(
                            "unknown method \"{other}\"; expected \"lsystem\" or \"space_colonization\""
                        ),
                    });
                }
            };
        }

        if let Some(v) = table.get("branch_levels") {
            p.branch_levels = toml_u32(v, "branch_levels")?;
            if p.branch_levels < 1 {
                return Err(ProcGenError::InvalidParameter {
                    name: "branch_levels".into(),
                    reason: "must be at least 1".into(),
                });
            }
        }

        if let Some(v) = table.get("branch_angle_min") {
            p.branch_angle_min = toml_f32(v, "branch_angle_min")?;
        }
        if let Some(v) = table.get("branch_angle_max") {
            p.branch_angle_max = toml_f32(v, "branch_angle_max")?;
        }

        if let Some(v) = table.get("branch_length_falloff") {
            p.branch_length_falloff = toml_f32(v, "branch_length_falloff")?;
            if p.branch_length_falloff <= 0.0 || p.branch_length_falloff > 1.0 {
                return Err(ProcGenError::InvalidParameter {
                    name: "branch_length_falloff".into(),
                    reason: "must be in (0.0, 1.0]".into(),
                });
            }
        }
        if let Some(v) = table.get("branch_radius_falloff") {
            p.branch_radius_falloff = toml_f32(v, "branch_radius_falloff")?;
            if p.branch_radius_falloff <= 0.0 || p.branch_radius_falloff > 1.0 {
                return Err(ProcGenError::InvalidParameter {
                    name: "branch_radius_falloff".into(),
                    reason: "must be in (0.0, 1.0]".into(),
                });
            }
        }

        if let Some(v) = table.get("branch_max_segments") {
            p.max_segments = toml_u32(v, "branch_max_segments")?;
        }
        if let Some(v) = table.get("branch_radial_segments") {
            p.radial_segments = toml_u32(v, "branch_radial_segments")?;
            if p.radial_segments < 3 {
                return Err(ProcGenError::InvalidParameter {
                    name: "branch_radial_segments".into(),
                    reason: "must be at least 3".into(),
                });
            }
        }

        if let Some(v) = table.get("branch_start") {
            p.branch_start = toml_f32(v, "branch_start")?;
            if !(0.0..=1.0).contains(&p.branch_start) {
                return Err(ProcGenError::InvalidParameter {
                    name: "branch_start".into(),
                    reason: "must be in [0.0, 1.0], a fraction of the trunk's height".into(),
                });
            }
        }

        // Space colonization params
        if let Some(v) = table.get("crown_shape") {
            let s = toml_string(v, "crown_shape")?;
            p.crown_shape = match s.as_str() {
                "sphere" | "Sphere" => CrownShape::Sphere,
                "ellipsoid" | "Ellipsoid" => CrownShape::Ellipsoid,
                "hemisphere" | "Hemisphere" => CrownShape::Hemisphere,
                "box" | "Box" => CrownShape::Box,
                "cone" | "Cone" => CrownShape::Cone,
                other => {
                    return Err(ProcGenError::InvalidParameter {
                        name: "crown_shape".into(),
                        reason: format!(
                            "unknown shape \"{other}\"; expected \"sphere\", \"ellipsoid\", \"hemisphere\", \"box\" or \"cone\""
                        ),
                    });
                }
            };
        }

        if let Some(v) = table.get("crown_radius") {
            p.crown_radius = toml_f32(v, "crown_radius")?;
            if p.crown_radius <= 0.0 {
                return Err(ProcGenError::InvalidParameter {
                    name: "crown_radius".into(),
                    reason: "must be positive".into(),
                });
            }
        }
        if let Some(v) = table.get("crown_height_ratio") {
            p.crown_height_ratio = toml_f32(v, "crown_height_ratio")?;
            if p.crown_height_ratio <= 0.0 {
                return Err(ProcGenError::InvalidParameter {
                    name: "crown_height_ratio".into(),
                    reason: "must be positive".into(),
                });
            }
        }
        if let Some(v) = table.get("num_attraction_points") {
            p.num_attraction_points = toml_u32(v, "num_attraction_points")?;
        }
        if let Some(v) = table.get("kill_distance") {
            p.kill_distance = toml_f32(v, "kill_distance")?;
        }
        if let Some(v) = table.get("influence_distance") {
            p.influence_distance = toml_f32(v, "influence_distance")?;
        }
        if let Some(v) = table.get("step_size") {
            p.step_size = toml_f32(v, "step_size")?;
        }
        if let Some(v) = table.get("branch_pipe_exponent") {
            p.pipe_exponent = toml_f32(v, "branch_pipe_exponent")?;
            if p.pipe_exponent < 1.0 {
                return Err(ProcGenError::InvalidParameter {
                    name: "branch_pipe_exponent".into(),
                    reason: "must be at least 1".into(),
                });
            }
        }

        // L-system params
        if let Some(v) = table.get("lsystem_iterations") {
            p.lsystem_iterations = toml_u32(v, "lsystem_iterations")?;
            if p.lsystem_iterations < 1 {
                return Err(ProcGenError::InvalidParameter {
                    name: "lsystem_iterations".into(),
                    reason: "must be at least 1".into(),
                });
            }
        }
        if let Some(v) = table.get("lsystem_angle_variation") {
            p.lsystem_angle_variation = toml_f32(v, "lsystem_angle_variation")?;
        }

        // Text grammar: a named preset, or an axiom with rules.
        if let Some(v) = table.get("lsystem_preset") {
            let name = toml_string(v, "lsystem_preset")?;
            let preset = grammar::preset(&name).ok_or_else(|| {
                let names: Vec<&str> = grammar::PRESETS.iter().map(|p| p.name).collect();
                ProcGenError::InvalidParameter {
                    name: "lsystem_preset".into(),
                    reason: format!("unknown preset \"{name}\"; expected one of {names:?}"),
                }
            })?;
            p.lsystem_grammar = Some(preset.grammar());
            p.lsystem_angle = Some(preset.angle);
            if !table.contains_key("lsystem_iterations") {
                p.lsystem_iterations = preset.iterations as u32;
            }
        }
        match (table.get("lsystem_axiom"), table.get("lsystem_rules")) {
            (None, None) => {}
            (Some(axiom), Some(rules)) => {
                let axiom = toml_string(axiom, "lsystem_axiom")?;
                let rules = toml_string_array(rules, "lsystem_rules")?;
                p.lsystem_grammar = Some(Grammar::parse(&axiom, &rules)?);
            }
            _ => {
                return Err(ProcGenError::InvalidParameter {
                    name: "lsystem_rules".into(),
                    reason: "lsystem_axiom and lsystem_rules go together".into(),
                });
            }
        }
        if let Some(v) = table.get("lsystem_angle") {
            p.lsystem_angle = Some(toml_f32(v, "lsystem_angle")?);
        }
        if let Some(v) = table.get("lsystem_step") {
            p.lsystem_step = Some(toml_f32(v, "lsystem_step")?);
        }
        if let Some(v) = table.get("lsystem_size") {
            p.lsystem_size = toml_f32(v, "lsystem_size")?;
        }
        if let Some(v) = table.get("lsystem_tropism") {
            p.lsystem_tropism = toml_f32(v, "lsystem_tropism")?;
        }

        Ok(p)
    }
}

// ─── Leaf Style & Params ────────────────────────────────────────────────────

/// How leaf geometry is constructed at branch tips.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeafStyle {
    /// Flat quads clustered at branch tips, each facing along its branch
    /// with a random spin, drawn from both sides.
    BillboardCluster,
    /// Small diamond/rhombus meshes oriented along branch direction.
    MeshCluster,
    /// No leaves.
    None,
}

/// Typed leaf configuration extracted from a `ProcGenSpec`'s params table.
#[derive(Debug, Clone)]
pub struct LeafParams {
    /// How leaf geometry is constructed.
    pub style: LeafStyle,
    /// Fraction of branch tips that receive leaves (0.0–1.0).
    pub density: f32,
    /// Base leaf color as linear RGBA.
    pub color_base: [f32; 4],
    /// Lightness spread between leaf shades (HSL, ±).
    pub color_variation: f32,
    /// Where the leaves are in the year: 0 = the base colour, 0.4 yellow,
    /// 0.7 orange, 1.0 red.
    pub season: f32,
    /// How far each shade's season strays from `season` (±), so an autumn
    /// tree turns unevenly.
    pub season_variation: f32,
    /// Number of leaf shades. Each is its own material; every leaf wears one.
    pub shades: u32,
    /// Base size of a single leaf quad/mesh in meters.
    pub leaf_size: f32,
    /// Random scale variation range (e.g. 0.3 means ±30%).
    pub leaf_size_variation: f32,
    /// How many leaf quads/meshes per branch tip.
    pub leaves_per_tip: u32,
    /// Radius of the sphere around each placement point within which leaves scatter.
    pub leaf_spread_radius: f32,
    /// Probability of placing leaves at each qualifying branch segment (0.0–1.0).
    /// 0.0 disables along-branch placement (default, backward compatible).
    pub leaf_along_branch_density: f32,
    /// Minimum branch depth for along-branch leaf placement.
    pub leaf_along_branch_min_depth: u32,
}

impl Default for LeafParams {
    fn default() -> Self {
        Self {
            style: LeafStyle::BillboardCluster,
            density: 0.8,
            color_base: [0.15, 0.35, 0.05, 1.0], // forest green (linear)
            color_variation: 0.08,
            season: 0.0,
            season_variation: 0.06,
            shades: 4,
            leaf_size: 0.15,
            leaf_size_variation: 0.3,
            leaves_per_tip: 5,
            leaf_spread_radius: 0.3,
            leaf_along_branch_density: 0.0,
            leaf_along_branch_min_depth: 2,
        }
    }
}

impl LeafParams {
    /// Parse leaf parameters from a TOML `params` value.
    ///
    /// Missing fields fall back to sensible defaults.
    pub fn from_toml(params: &toml::Value) -> Result<Self> {
        let table = params
            .as_table()
            .ok_or_else(|| ProcGenError::InvalidParameter {
                name: "params".into(),
                reason: "expected a TOML table".into(),
            })?;

        let mut p = Self::default();

        if let Some(v) = table.get("leaf_style") {
            let s = toml_string(v, "leaf_style")?;
            p.style = match s.as_str() {
                "billboard" | "billboard_cluster" | "BillboardCluster" => {
                    LeafStyle::BillboardCluster
                }
                "mesh" | "mesh_cluster" | "MeshCluster" => LeafStyle::MeshCluster,
                "none" | "None" => LeafStyle::None,
                other => {
                    return Err(ProcGenError::InvalidParameter {
                        name: "leaf_style".into(),
                        reason: format!(
                            "unknown style \"{other}\"; expected \"billboard\", \"mesh\", or \"none\""
                        ),
                    });
                }
            };
        }

        if let Some(v) = table.get("leaf_density") {
            p.density = toml_f32(v, "leaf_density")?;
            if !(0.0..=1.0).contains(&p.density) {
                return Err(ProcGenError::InvalidParameter {
                    name: "leaf_density".into(),
                    reason: "must be in [0.0, 1.0]".into(),
                });
            }
        }

        if let Some(v) = table.get("leaf_color_base") {
            let hex = v.as_str().ok_or_else(|| ProcGenError::InvalidParameter {
                name: "leaf_color_base".into(),
                reason: "expected a hex color string (e.g. \"#228B22\")".into(),
            })?;
            p.color_base = parse_hex_color(hex)?;
        }

        if let Some(v) = table.get("leaf_color_variation") {
            p.color_variation = toml_f32(v, "leaf_color_variation")?;
            if p.color_variation < 0.0 {
                return Err(ProcGenError::InvalidParameter {
                    name: "leaf_color_variation".into(),
                    reason: "must be non-negative".into(),
                });
            }
        }

        if let Some(v) = table.get("leaf_season") {
            p.season = toml_f32(v, "leaf_season")?;
            if !(0.0..=1.0).contains(&p.season) {
                return Err(ProcGenError::InvalidParameter {
                    name: "leaf_season".into(),
                    reason: "must be in [0.0, 1.0] (0 base colour, 0.4 yellow, 0.7 orange, 1 red)"
                        .into(),
                });
            }
        }

        if let Some(v) = table.get("leaf_season_variation") {
            p.season_variation = toml_f32(v, "leaf_season_variation")?;
            if !(0.0..=1.0).contains(&p.season_variation) {
                return Err(ProcGenError::InvalidParameter {
                    name: "leaf_season_variation".into(),
                    reason: "must be in [0.0, 1.0]".into(),
                });
            }
        }

        if let Some(v) = table.get("leaf_shades") {
            p.shades = toml_u32(v, "leaf_shades")?;
            if !(1..=8).contains(&p.shades) {
                return Err(ProcGenError::InvalidParameter {
                    name: "leaf_shades".into(),
                    reason: "must be in 1..=8; each shade is a material and a draw".into(),
                });
            }
        }

        if let Some(v) = table.get("leaf_size") {
            p.leaf_size = toml_f32(v, "leaf_size")?;
            if p.leaf_size <= 0.0 {
                return Err(ProcGenError::InvalidParameter {
                    name: "leaf_size".into(),
                    reason: "must be positive".into(),
                });
            }
        }

        if let Some(v) = table.get("leaf_size_variation") {
            p.leaf_size_variation = toml_f32(v, "leaf_size_variation")?;
            if p.leaf_size_variation < 0.0 || p.leaf_size_variation > 1.0 {
                return Err(ProcGenError::InvalidParameter {
                    name: "leaf_size_variation".into(),
                    reason: "must be in [0.0, 1.0]".into(),
                });
            }
        }

        if let Some(v) = table.get("leaves_per_tip") {
            p.leaves_per_tip = toml_u32(v, "leaves_per_tip")?;
            if p.leaves_per_tip < 1 {
                return Err(ProcGenError::InvalidParameter {
                    name: "leaves_per_tip".into(),
                    reason: "must be at least 1".into(),
                });
            }
        }

        if let Some(v) = table.get("leaf_spread_radius") {
            p.leaf_spread_radius = toml_f32(v, "leaf_spread_radius")?;
            if p.leaf_spread_radius < 0.0 {
                return Err(ProcGenError::InvalidParameter {
                    name: "leaf_spread_radius".into(),
                    reason: "must be non-negative".into(),
                });
            }
        }

        if let Some(v) = table.get("leaf_along_branch_density") {
            p.leaf_along_branch_density = toml_f32(v, "leaf_along_branch_density")?;
            if !(0.0..=1.0).contains(&p.leaf_along_branch_density) {
                return Err(ProcGenError::InvalidParameter {
                    name: "leaf_along_branch_density".into(),
                    reason: "must be in [0.0, 1.0]".into(),
                });
            }
        }

        if let Some(v) = table.get("leaf_along_branch_min_depth") {
            p.leaf_along_branch_min_depth = toml_u32(v, "leaf_along_branch_min_depth")?;
        }

        Ok(p)
    }
}

// ─── Branch & Tree Output ───────────────────────────────────────────────────

/// Output of branch generation.
#[derive(Debug, Clone)]
pub struct BranchOutput {
    /// Combined branch mesh.
    pub mesh: MeshData,
    /// World-space positions of terminal branch tips (for leaf placement).
    pub tip_positions: Vec<Vec3>,
    /// Directions at terminal branch tips.
    pub tip_directions: Vec<Vec3>,
    /// Total number of branch segments generated.
    pub segment_count: usize,
    /// All branch segments (for along-branch leaf placement).
    pub segments: Vec<BranchSegment>,
}

/// Output of full tree generation (trunk + branches).
#[derive(Debug, Clone)]
pub struct TreeOutput {
    /// Combined trunk + branch mesh.
    pub mesh: MeshData,
    /// Procedural bark normal map.
    pub normal_map: ImageData,
    /// World-space positions of terminal branch tips (for leaf placement).
    pub branch_tips: Vec<Vec3>,
    /// Directions at terminal branch tips.
    pub branch_tip_directions: Vec<Vec3>,
    /// All branch segments (for along-branch leaf placement).
    pub branch_segments: Vec<BranchSegment>,
}

// ─── Combined Tree Generation ───────────────────────────────────────────────

/// Generate a complete tree (trunk + branches) from parameters and RNG.
///
/// 1. Generates the trunk via [`generate_trunk`]
/// 2. Generates branches via [`generate_branches`] attached along the trunk
/// 3. Merges both meshes and returns the combined result
pub fn generate_tree(
    trunk_params: &TrunkParams,
    branch_params: &BranchParams,
    rng: &mut SeededRng,
) -> Result<TreeOutput> {
    let mut trunk_rng = rng.fork("trunk");
    let trunk = generate_trunk(trunk_params, &mut trunk_rng)?;

    let mut branch_rng = rng.fork("branches");
    let trunk_spine = TrunkSpine {
        points: trunk.spine.clone(),
        radius_base: trunk_params.radius_base,
        radius_top: trunk_params.radius_top,
    };
    let branch_output = generate_branches(branch_params, &trunk_spine, &mut branch_rng)?;

    // Merge trunk + branch meshes
    let mut builder = MeshBuilder::new();
    builder.merge(&trunk.mesh);
    builder.merge(&branch_output.mesh);
    builder.compute_normals();
    builder.compute_tangents();

    // Preserve bark material
    let mut mesh = builder.build();
    mesh.materials = trunk.mesh.materials.clone();

    Ok(TreeOutput {
        mesh,
        normal_map: trunk.normal_map,
        branch_tips: branch_output.tip_positions,
        branch_tip_directions: branch_output.tip_directions,
        branch_segments: branch_output.segments,
    })
}

// ─── TreeGenerator ──────────────────────────────────────────────────────────

/// Procedural tree generator — produces trunk, branches, and leaves with
/// optional LOD levels.
///
/// Registered as `"tree_v1"` in the generator registry. All parameters are
/// parsed from the spec's `params` table via [`TrunkParams`], [`BranchParams`],
/// and [`LeafParams`].
pub struct TreeGenerator;

impl Generator for TreeGenerator {
    fn type_name(&self) -> &str {
        "tree_v1"
    }

    fn output_kind(&self) -> OutputKind {
        OutputKind::Mesh
    }

    fn generate(&self, spec: &ProcGenSpec, rng: &mut SeededRng) -> Result<GeneratorOutput> {
        // 1. Parse params
        let trunk_params = TrunkParams::from_toml(&spec.params)?;
        let branch_params = BranchParams::from_toml(&spec.params)?;
        let leaf_params = LeafParams::from_toml(&spec.params)?;

        // 2. Generate tree (trunk + branches)
        let tree = generate_tree(&trunk_params, &branch_params, rng)?;

        // 3. Generate leaves
        let leaf_mesh = generate_leaves(
            &tree.branch_tips,
            &tree.branch_tip_directions,
            &tree.branch_segments,
            &leaf_params,
            &mut rng.fork("leaves"),
        )?;

        // 4. LODs: output index = `[[lod]] level`. Each entry caps its
        //    level (LOD 0 included) and is reduced from the full tree. LOD 0
        //    without an entry is the full tree; a skipped level repeats the
        //    one before it.
        let card_indices = leaves::card_index_count(leaf_params.style);
        if let Some(lod_levels) = spec.lod.as_ref().filter(|l| !l.is_empty()) {
            let deepest = lod_levels.iter().map(|l| l.level).max().unwrap_or(0);
            let mut lods: Vec<MeshData> = Vec::with_capacity(deepest as usize + 1);
            for level in 0..=deepest {
                let lod = match lod_levels.iter().find(|l| l.level == level) {
                    Some(l) => tree_lod(
                        &tree.mesh,
                        &leaf_mesh,
                        card_indices,
                        l.target_triangles as usize,
                    ),
                    None => match lods.last() {
                        Some(previous) => previous.clone(),
                        None => join_bark_and_leaves(&tree.mesh, &leaf_mesh),
                    },
                };
                lods.push(lod);
            }
            return Ok(GeneratorOutput::MeshWithLods(lods));
        }

        // 5. Otherwise the whole tree: bark, then a submesh per leaf shade
        Ok(GeneratorOutput::Mesh(join_bark_and_leaves(
            &tree.mesh, &leaf_mesh,
        )))
    }

    fn param_schema(&self) -> serde_json::Value {
        use serde_json::{json, Value};

        // `x-when` says which branch method reads a key (see
        // `param_ui::lint_params`): the editor hides the rest and `flint gen`
        // warns about them. `tree_schema_says_which_method_reads_each_key`
        // holds the schema to it.
        let only = |method: &str, mut prop: Value| {
            prop["x-when"] = json!({ "branch_method": [method] });
            prop
        };
        let lsys = |prop| only("lsystem", prop);
        let colon = |prop| only("space_colonization", prop);
        let hex = "^#?[0-9a-fA-F]{6,8}$";

        json!({
            "type": "object",
            "properties": {
                // ── Trunk ──
                "trunk_height": { "type": "number", "minimum": 0.01, "default": 5.0,
                    "description": "Trunk length in metres, from the ground to where it ends" },
                "trunk_radius_base": { "type": "number", "minimum": 0.01, "default": 0.3,
                    "description": "Trunk radius at the ground, metres" },
                "trunk_radius_top": { "type": "number", "minimum": 0.0, "default": 0.1,
                    "description": "Trunk radius at its tip, metres; the limbs carrying on from the tip start this thick" },
                "trunk_segments": { "type": "integer", "minimum": 2, "default": 10,
                    "description": "Rings along the trunk; more follow its sway more smoothly" },
                "radial_segments": { "type": "integer", "minimum": 3, "default": 8,
                    "description": "Sides around the trunk" },
                "trunk_curve_noise": { "type": "number", "minimum": 0.0, "default": 0.1,
                    "description": "Largest sideways sway of the trunk, metres (wavelength about 0.6 of its height); 0 grows it straight" },

                // ── Bark ──
                "bark_color_base": { "type": "string", "pattern": hex, "default": "#573214",
                    "description": "Bark albedo, sRGB hex" },
                "bark_roughness": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.9,
                    "description": "Bark material roughness" },
                "bark_normal_strength": { "type": "number", "default": 1.0,
                    "description": "Depth of the bark normal map's furrows and cracks; 0 is smooth" },
                "bark_normal_resolution": { "type": "integer", "default": 256,
                    "description": "Side of the square bark normal map in pixels; a power of two, at least 16" },

                // ── Branching ──
                "branch_method": { "type": "string", "enum": ["lsystem", "space_colonization"], "default": "lsystem",
                    "description": "How limbs grow: an L-system (a built-in rule, or a grammar of your own) or space colonization toward points filling the crown" },
                "branch_levels": { "type": "integer", "minimum": 1, "default": 3, "deprecated": true,
                    "description": "Has no effect: the L-system's depth is lsystem_iterations, and space colonization grows until its points run out" },
                "branch_start": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.6,
                    "description": "Where on the trunk the lowest limb leaves it, 0 the ground to 1 the tip. A grammar ignores it and grows from the tip" },
                "branch_max_segments": { "type": "integer", "minimum": 1, "default": 5000,
                    "description": "Cap on branch segments the growth may emit, a guard against runaway grammars" },
                "branch_radial_segments": { "type": "integer", "minimum": 3, "default": 6,
                    "description": "Sides around the thickest limbs; thinner ones get fewer, down to three" },
                "crown_radius": { "type": "number", "exclusiveMinimum": 0.0, "default": 3.0,
                    "description": "Crown size, metres: the built-in L-system's reach, or the colonized volume's radius. A grammar uses lsystem_size instead" },

                // ── L-system ──
                "branch_angle_min": lsys(json!({ "type": "number", "default": 20.0,
                    "description": "Smallest angle a limb leaves its parent at, degrees" })),
                "branch_angle_max": lsys(json!({ "type": "number", "default": 40.0,
                    "description": "Largest angle a limb leaves its parent at, degrees; a grammar without lsystem_angle turns by the middle of min and max" })),
                "branch_length_falloff": lsys(json!({ "type": "number", "exclusiveMinimum": 0.0, "maximum": 1.0, "default": 0.7,
                    "description": "Each branching level's length as a fraction of its parent's" })),
                "branch_radius_falloff": lsys(json!({ "type": "number", "exclusiveMinimum": 0.0, "maximum": 1.0, "default": 0.6,
                    "description": "Each branching level's radius as a fraction of its parent's" })),
                "lsystem_iterations": lsys(json!({ "type": "integer", "minimum": 1, "default": 3,
                    "description": "Rewriting passes: the crown's branching depth. Each one multiplies the segment count" })),
                "lsystem_angle_variation": lsys(json!({ "type": "number", "minimum": 0.0, "default": 10.0,
                    "description": "Random spread added to every turn of the built-in rule, degrees. A grammar ignores it" })),
                "lsystem_preset": lsys(json!({ "type": "string", "enum": ["fractal_plant", "weed", "stochastic_plant", "bush"],
                    "description": "A built-in grammar from The Algorithmic Beauty of Plants, in place of the built-in rule" })),
                "lsystem_axiom": lsys(json!({ "type": "string",
                    "description": "Your own grammar's starting string; with lsystem_rules it replaces the built-in rule" })),
                "lsystem_rules": lsys(json!({ "type": "array", "items": { "type": "string" },
                    "description": "Your own grammar's productions, e.g. \"X -> F[+X][-X]FX\" or weighted \"F (0.7) -> FF\"" })),
                "lsystem_angle": lsys(json!({ "type": "number",
                    "description": "A grammar's turn angle, degrees; defaults to the preset's, else the middle of branch_angle_min/max" })),
                "lsystem_step": lsys(json!({ "type": "number", "exclusiveMinimum": 0.0,
                    "description": "A grammar's fixed step length in metres; when unset its crown is scaled to lsystem_size" })),
                "lsystem_size": lsys(json!({ "type": "number", "exclusiveMinimum": 0.0, "default": 4.0,
                    "description": "How far a grammar's crown reaches from the trunk tip, metres, when lsystem_step is unset" })),
                "lsystem_tropism": lsys(json!({ "type": "number", "default": 0.0,
                    "description": "A grammar's bend toward the ground per step; negative bends up toward the light" })),

                // ── Space colonization ──
                "crown_shape": colon(json!({ "type": "string", "enum": ["sphere", "ellipsoid", "hemisphere", "box", "cone"], "default": "sphere",
                    "description": "The volume the attraction points fill; it rests on the trunk at branch_start" })),
                "crown_height_ratio": colon(json!({ "type": "number", "exclusiveMinimum": 0.0, "default": 1.5,
                    "description": "Crown height as a multiple of crown_radius, for the ellipsoid, box and cone" })),
                "num_attraction_points": colon(json!({ "type": "integer", "minimum": 0, "default": 500,
                    "description": "Points scattered through the crown; branches grow until they have consumed them. More gives a denser crown" })),
                "influence_distance": colon(json!({ "type": "number", "exclusiveMinimum": 0.0, "default": 4.0,
                    "description": "How far a point pulls the nearest branch tip, metres" })),
                "kill_distance": colon(json!({ "type": "number", "exclusiveMinimum": 0.0, "default": 0.5,
                    "description": "How close a branch must come to consume a point, metres; smaller fills the crown more finely" })),
                "step_size": colon(json!({ "type": "number", "exclusiveMinimum": 0.0, "default": 0.3,
                    "description": "Length of each growth step, metres" })),
                "branch_pipe_exponent": colon(json!({ "type": "number", "minimum": 1.0, "default": 2.0,
                    "description": "Pipe model exponent n in r_parent^n = sum(r_child^n): 2 is da Vinci's rule, about 3 keeps twigs sturdier" })),

                // ── Leaves ──
                "leaf_style": { "type": "string", "enum": ["billboard", "mesh", "none"], "default": "billboard",
                    "description": "Leaf cards: quads facing along their branch, diamonds tilted off it, or no leaves" },
                "leaf_density": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.8,
                    "description": "Fraction of branch tips that get a cluster of leaves" },
                "leaves_per_tip": { "type": "integer", "minimum": 1, "default": 5,
                    "description": "Leaves in each cluster" },
                "leaf_spread_radius": { "type": "number", "minimum": 0.0, "default": 0.3,
                    "description": "Radius a cluster's leaves scatter within, metres" },
                "leaf_along_branch_density": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.0,
                    "description": "Chance of a cluster on each branch segment as well as the tips; 0 leaves the tips only" },
                "leaf_along_branch_min_depth": { "type": "integer", "minimum": 0, "default": 2,
                    "description": "Shallowest branching level that gets along-branch leaves (1 = limbs off the trunk)" },
                "leaf_size": { "type": "number", "minimum": 0.01, "default": 0.15,
                    "description": "Leaf card size, metres" },
                "leaf_size_variation": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.3,
                    "description": "Random size spread, as a fraction (0.3 = ±30%)" },
                "leaf_color_base": { "type": "string", "pattern": hex, "default": "#2D8C0D",
                    "description": "Leaf albedo before the season tints it, sRGB hex" },
                "leaf_shades": { "type": "integer", "minimum": 1, "maximum": 8, "default": 4,
                    "description": "Distinct leaf shades, each its own material" },
                "leaf_color_variation": { "type": "number", "minimum": 0.0, "default": 0.08,
                    "description": "Lightness spread between the shades (HSL, ±)" },
                "leaf_season": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.0,
                    "description": "Time of year: 0 the base colour, 0.4 yellow, 0.7 orange, 1 red" },
                "leaf_season_variation": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.06,
                    "description": "How far each shade's season strays from leaf_season, so a tree turns unevenly" }
            },
            "additionalProperties": false
        })
    }

    fn estimate_cost(&self, spec: &ProcGenSpec) -> GenerationCost {
        let trunk = TrunkParams::from_toml(&spec.params).unwrap_or_default();
        let branch = BranchParams::from_toml(&spec.params).unwrap_or_default();

        let trunk_verts = trunk.segments * trunk.radial_segments;
        let branch_verts = branch.max_segments * branch.radial_segments;
        let total_verts = trunk_verts + branch_verts;

        GenerationCost {
            estimated_vertices: total_verts,
            estimated_triangles: total_verts, // rough estimate
            estimated_texture_bytes: (trunk.bark_normal_resolution as u64)
                .pow(2)
                .saturating_mul(4),
            estimated_generation_ms: 50.0 + branch.max_segments as f64 * 0.01,
        }
    }
}

// Shared helpers (parse_hex_color, toml_f32, etc.) live in generators::util.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generators::util::srgb_to_linear;

    #[test]
    fn parse_hex_6_digit() {
        let c = parse_hex_color("#8B4513").unwrap();
        // sRGB (139, 69, 19) → linear; verify channels are in expected range
        // r: 139/255 = 0.545 sRGB → ~0.26 linear
        // g: 69/255 = 0.271 sRGB → ~0.06 linear
        // b: 19/255 = 0.075 sRGB → ~0.005 linear
        assert!(c[0] > 0.2 && c[0] < 0.35, "red channel: {}", c[0]);
        assert!(c[1] > 0.04 && c[1] < 0.08, "green channel: {}", c[1]);
        assert!(c[2] > 0.003 && c[2] < 0.01, "blue channel: {}", c[2]);
        assert!((c[3] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn parse_hex_without_hash() {
        let c = parse_hex_color("8B4513").unwrap();
        assert!((c[3] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn parse_hex_8_digit() {
        let c = parse_hex_color("#8B451380").unwrap();
        assert!((c[3] - 128.0 / 255.0).abs() < 0.01);
    }

    #[test]
    fn parse_hex_invalid() {
        assert!(parse_hex_color("#ZZZ").is_err());
        assert!(parse_hex_color("#12345").is_err());
        assert!(parse_hex_color("").is_err());
    }

    #[test]
    fn srgb_to_linear_black_and_white() {
        assert!((srgb_to_linear(0.0)).abs() < 1e-6);
        assert!((srgb_to_linear(1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn trunk_params_defaults() {
        let toml_val: toml::Value = toml::toml! {
            trunk_height = 8.0
        }
        .into();
        let p = TrunkParams::from_toml(&toml_val).unwrap();
        assert!((p.height - 8.0).abs() < 1e-5);
        assert_eq!(p.radial_segments, 8); // default
        assert_eq!(p.segments, 10); // default
    }

    #[test]
    fn trunk_params_full() {
        let toml_val: toml::Value = toml::toml! {
            trunk_height = 10.0
            trunk_radius_base = 0.5
            trunk_radius_top = 0.15
            trunk_segments = 20
            radial_segments = 12
            trunk_curve_noise = 0.3
            bark_color_base = "#8B4513"
            bark_roughness = 0.85
            bark_normal_strength = 1.5
            bark_normal_resolution = 128
        }
        .into();
        let p = TrunkParams::from_toml(&toml_val).unwrap();
        assert!((p.height - 10.0).abs() < 1e-5);
        assert!((p.radius_base - 0.5).abs() < 1e-5);
        assert!((p.radius_top - 0.15).abs() < 1e-5);
        assert_eq!(p.segments, 20);
        assert_eq!(p.radial_segments, 12);
        assert!((p.curve_noise - 0.3).abs() < 1e-5);
        assert!((p.bark_roughness - 0.85).abs() < 1e-5);
        assert!((p.bark_normal_strength - 1.5).abs() < 1e-5);
        assert_eq!(p.bark_normal_resolution, 128);
    }

    #[test]
    fn trunk_params_invalid_height() {
        let toml_val: toml::Value = toml::toml! {
            trunk_height = -1.0
        }
        .into();
        assert!(TrunkParams::from_toml(&toml_val).is_err());
    }

    #[test]
    fn trunk_params_invalid_resolution() {
        let toml_val: toml::Value = toml::toml! {
            bark_normal_resolution = 100
        }
        .into();
        assert!(TrunkParams::from_toml(&toml_val).is_err());
    }

    #[test]
    fn trunk_params_not_a_table() {
        let toml_val = toml::Value::String("not a table".into());
        assert!(TrunkParams::from_toml(&toml_val).is_err());
    }

    // ─── LeafParams tests ──────────────────────────────────────────────────

    #[test]
    fn leaf_params_defaults() {
        let toml_val: toml::Value = toml::toml! {
            trunk_height = 5.0
        }
        .into();
        let p = LeafParams::from_toml(&toml_val).unwrap();
        assert_eq!(p.style, LeafStyle::BillboardCluster);
        assert!((p.density - 0.8).abs() < 1e-5);
        assert_eq!(p.leaves_per_tip, 5);
    }

    #[test]
    fn leaf_params_full() {
        let toml_val: toml::Value = toml::toml! {
            leaf_style = "mesh"
            leaf_density = 0.6
            leaf_color_base = "#228B22"
            leaf_color_variation = 0.1
            leaf_size = 0.2
            leaf_size_variation = 0.4
            leaves_per_tip = 8
        }
        .into();
        let p = LeafParams::from_toml(&toml_val).unwrap();
        assert_eq!(p.style, LeafStyle::MeshCluster);
        assert!((p.density - 0.6).abs() < 1e-5);
        assert!((p.leaf_size - 0.2).abs() < 1e-5);
        assert_eq!(p.leaves_per_tip, 8);
    }

    #[test]
    fn leaf_params_none_style() {
        let toml_val: toml::Value = toml::toml! {
            leaf_style = "none"
        }
        .into();
        let p = LeafParams::from_toml(&toml_val).unwrap();
        assert_eq!(p.style, LeafStyle::None);
    }

    #[test]
    fn leaf_params_invalid_density() {
        let toml_val: toml::Value = toml::toml! {
            leaf_density = 1.5
        }
        .into();
        assert!(LeafParams::from_toml(&toml_val).is_err());
    }

    #[test]
    fn leaf_params_invalid_style() {
        let toml_val: toml::Value = toml::toml! {
            leaf_style = "feathers"
        }
        .into();
        assert!(LeafParams::from_toml(&toml_val).is_err());
    }

    // ─── TreeGenerator integration tests ────────────────────────────────────

    fn tree_spec_toml() -> &'static str {
        r#"
generator = "tree_v1"
[meta]
name = "test_tree"
version = "1.0.0"
[seed]
mode = "fixed"
value = 42
[params]
trunk_height = 5.0
trunk_radius_base = 0.3
trunk_radius_top = 0.1
branch_method = "lsystem"
branch_levels = 2
leaf_style = "billboard"
leaf_density = 0.8
leaves_per_tip = 3
"#
    }

    #[test]
    fn tree_generator_object_safety() {
        // Proves the Generator trait is object-safe with TreeGenerator.
        let _: Box<dyn Generator> = Box::new(TreeGenerator);
    }

    #[test]
    fn tree_generator_type_name() {
        assert_eq!(TreeGenerator.type_name(), "tree_v1");
    }

    #[test]
    fn tree_generator_output_kind() {
        assert_eq!(TreeGenerator.output_kind(), OutputKind::Mesh);
    }

    #[test]
    fn tree_generator_end_to_end() {
        let spec = ProcGenSpec::parse(tree_spec_toml()).unwrap();
        let mut rng = SeededRng::new(42);
        let output = TreeGenerator.generate(&spec, &mut rng).unwrap();

        // Without LOD, should produce a single Mesh
        let mesh = output.as_mesh().unwrap();
        assert!(mesh.vertex_count() > 0);
        assert!(mesh.triangle_count() > 0);
        assert!(mesh.validate().is_ok());
    }

    #[test]
    fn tree_generator_with_lods() {
        let spec = ProcGenSpec::parse(
            r#"
generator = "tree_v1"
[meta]
name = "test_tree_lod"
version = "1.0.0"
[seed]
mode = "fixed"
value = 42
[params]
trunk_height = 5.0
branch_method = "lsystem"
branch_levels = 2
leaf_style = "billboard"
[[lod]]
level = 1
target_triangles = 500
[[lod]]
level = 2
target_triangles = 200
"#,
        )
        .unwrap();

        let mut rng = SeededRng::new(42);
        let output = TreeGenerator.generate(&spec, &mut rng).unwrap();

        let lods = output.as_mesh_lods().unwrap();
        assert!(lods.len() >= 3); // LOD 0 + 2 simplified
        assert!(lods[0].triangle_count() >= lods[1].triangle_count());
        for lod in lods {
            assert!(lod.validate().is_ok());
        }
    }

    /// Area of a mesh's triangles that use a material named `leaf_*`, front
    /// faces only (each card's first half).
    fn leaf_area(mesh: &MeshData) -> f32 {
        let pos = |i: u32| Vec3::from_array(mesh.vertices[i as usize].position);
        mesh.submeshes
            .iter()
            .filter(|s| mesh.materials[s.material_index].name.starts_with("leaf_"))
            .flat_map(|s| {
                mesh.indices[s.index_start as usize..(s.index_start + s.index_count) as usize]
                    .chunks(3)
            })
            .map(|t| {
                0.5 * (pos(t[1]) - pos(t[0]))
                    .cross(&(pos(t[2]) - pos(t[0])))
                    .length()
            })
            .sum::<f32>()
            / 2.0
    }

    #[test]
    fn oak_spec_lods_hit_targets_and_keep_the_trunk() {
        // The shipped example spec, end to end.
        let spec =
            ProcGenSpec::parse(include_str!("../../../../../specs/oak_tree.procgen.toml")).unwrap();
        let targets: Vec<usize> = spec
            .lod
            .as_ref()
            .unwrap()
            .iter()
            .map(|l| l.target_triangles as usize)
            .collect();
        let output = TreeGenerator
            .generate(&spec, &mut SeededRng::new(spec.seed.value.unwrap_or(42)))
            .unwrap();
        let lods = output.as_mesh_lods().unwrap();
        assert_eq!(lods.len(), targets.len());

        // Same spec, same seed, same LODs. The simplifier once summed its
        // boundary quadrics in hash-map order, so every run differed.
        let again = TreeGenerator
            .generate(&spec, &mut SeededRng::new(spec.seed.value.unwrap_or(42)))
            .unwrap();
        assert!(again == output, "the oak's LODs changed between two runs");

        let report = crate::validate::validate_output(
            &output,
            &spec,
            &crate::validate::ValidationConstraints::default(),
        );
        for c in report
            .checks
            .iter()
            .filter(|c| c.name.contains("degenerate"))
        {
            assert_eq!(
                c.status,
                crate::validate::CheckStatus::Pass,
                "{}: {}",
                c.name,
                c.detail
            );
        }

        let full = TreeGenerator
            .generate(
                &ProcGenSpec {
                    lod: None,
                    ..spec.clone()
                },
                &mut SeededRng::new(spec.seed.value.unwrap_or(42)),
            )
            .unwrap();
        let full_leaf_area = leaf_area(full.as_mesh().unwrap());
        for (i, (lod, &target)) in lods.iter().zip(&targets).enumerate() {
            assert!(lod.validate().is_ok());
            let tris = lod.triangle_count();
            assert!(
                tris <= target && tris as f32 >= target as f32 * 0.9,
                "LOD {i}: {tris} triangles for a target of {target}"
            );
            if i > 0 {
                assert!(tris < lods[i - 1].triangle_count());
            }

            // The bark is submesh 0, and the trunk's foot is still in it.
            let bark = &lod.submeshes[0];
            assert_eq!(lod.materials[bark.material_index].name, "bark");
            let foot = lod.indices
                [bark.index_start as usize..(bark.index_start + bark.index_count) as usize]
                .iter()
                .any(|&v| lod.vertices[v as usize].position[1] < 0.3);
            assert!(foot, "LOD {i} lost the foot of the trunk");

            // Leaves keep their shades and roughly their coverage.
            assert!(lod.submeshes.len() >= 2, "LOD {i} lost its leaf shades");
            let area = leaf_area(lod);
            assert!(
                (area / full_leaf_area - 1.0).abs() < 0.25,
                "LOD {i}: leaf area {area:.2} m² against {full_leaf_area:.2}"
            );
        }
    }

    #[test]
    fn tree_generator_determinism() {
        let spec = ProcGenSpec::parse(tree_spec_toml()).unwrap();

        let mut rng_a = SeededRng::new(42);
        let output_a = TreeGenerator.generate(&spec, &mut rng_a).unwrap();

        let mut rng_b = SeededRng::new(42);
        let output_b = TreeGenerator.generate(&spec, &mut rng_b).unwrap();

        assert_eq!(output_a, output_b);
    }

    #[test]
    fn tree_generator_missing_leaf_params_uses_defaults() {
        let spec = ProcGenSpec::parse(
            r#"
generator = "tree_v1"
[meta]
name = "bare_tree"
version = "1.0.0"
[seed]
mode = "fixed"
value = 42
[params]
trunk_height = 5.0
branch_method = "lsystem"
branch_levels = 2
"#,
        )
        .unwrap();

        let mut rng = SeededRng::new(42);
        let output = TreeGenerator.generate(&spec, &mut rng).unwrap();
        assert!(output.as_mesh().is_some());
        assert!(output.as_mesh().unwrap().validate().is_ok());
    }

    #[test]
    fn tree_generator_param_schema_valid_json() {
        let schema = TreeGenerator.param_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["trunk_height"].is_object());
        assert!(schema["properties"]["leaf_style"].is_object());
    }

    /// Nudge one key and see whether the tree changes; an error counts as
    /// a change (the key was read and refused).
    fn tree_nudge(prop: &serde_json::Value, current: Option<&toml::Value>) -> toml::Value {
        let max = prop["maximum"].as_f64();
        match prop["type"].as_str().unwrap() {
            "integer" => {
                let now = current
                    .and_then(|v| v.as_integer())
                    .or(prop["default"].as_i64())
                    .unwrap_or(4);
                let min = prop["minimum"].as_i64().unwrap_or(0);
                // Powers of two stay powers of two (bark_normal_resolution).
                if now >= 32 && (now as u64).is_power_of_two() {
                    (now / 2).into()
                } else if now > min {
                    (now - 1).into()
                } else {
                    (now + 2).into()
                }
            }
            "number" => {
                let now = current
                    .and_then(|v| v.as_float().or(v.as_integer().map(|i| i as f64)))
                    .or(prop["default"].as_f64());
                let Some(now) = now else {
                    return if max.is_some() { 0.3 } else { 7.0 }.into();
                };
                let up = now * 1.3 + 0.05;
                match max {
                    Some(m) if up > m => (now * 0.6).into(),
                    _ => up.into(),
                }
            }
            "string" if prop["enum"].is_array() => {
                let now = current
                    .and_then(|v| v.as_str())
                    .or(prop["default"].as_str())
                    .unwrap_or("");
                let other = prop["enum"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|v| v.as_str() != Some(now))
                    .unwrap();
                other.as_str().unwrap().into()
            }
            "string" => "#20A0C0".into(),
            t => panic!("no nudge for {t}"),
        }
    }

    fn small_tree(method: &str) -> ProcGenSpec {
        let mut spec = ProcGenSpec::parse(&format!(
            r##"
generator = "tree_v1"
[meta]
name = "nudge_tree"
version = "1.0.0"
[seed]
mode = "fixed"
value = 11
[params]
branch_method = "{method}"
trunk_segments = 6
lsystem_iterations = 3
num_attraction_points = 120
leaf_density = 0.6
leaf_along_branch_density = 0.4
leaf_along_branch_min_depth = 2
bark_normal_resolution = 32
"##
        ))
        .unwrap();
        if method == "space_colonization" {
            spec.params
                .as_table_mut()
                .unwrap()
                .insert("crown_shape".into(), "ellipsoid".into());
        }
        spec
    }

    #[test]
    fn tree_schema_says_which_method_reads_each_key() {
        // Keys the L-system reads only with a grammar, and only without one.
        // `x-when` can name one controlling key, so these two splits live in
        // the descriptions; this test holds both to the generator.
        const GRAMMAR_ONLY: [&str; 4] = [
            "lsystem_angle",
            "lsystem_step",
            "lsystem_size",
            "lsystem_tropism",
        ];
        const BUILT_IN_ONLY: [&str; 3] =
            ["crown_radius", "branch_start", "lsystem_angle_variation"];

        let schema = TreeGenerator.param_schema();
        let props = schema["properties"].as_object().unwrap();
        let fields = crate::parse_param_schema(&schema);
        let run = |spec: &ProcGenSpec| TreeGenerator.generate(spec, &mut SeededRng::new(11)).ok();
        for route in ["lsystem", "grammar", "space_colonization"] {
            let mut base = small_tree(if route == "grammar" { "lsystem" } else { route });
            if route == "grammar" {
                // A grammar with no angle of its own, so it turns by
                // branch_angle_min/max as the descriptions say.
                let t = base.params.as_table_mut().unwrap();
                t.insert("lsystem_axiom".into(), "X".into());
                t.insert(
                    "lsystem_rules".into(),
                    toml::Value::Array(vec!["X -> F[+X][-X]FX".into(), "F -> FF".into()]),
                );
            }
            let reference = run(&base).expect("base tree builds");
            for (key, prop) in props {
                let key = key.as_str();
                if [
                    "branch_method",
                    "lsystem_axiom",
                    "lsystem_rules",
                    "lsystem_preset",
                ]
                .contains(&key)
                {
                    continue;
                }
                let mut spec = base.clone();
                let value = if key == "branch_max_segments" {
                    // A cap only shows when growth reaches it.
                    toml::Value::Integer(12)
                } else {
                    tree_nudge(prop, spec.params.get(key))
                };
                spec.params
                    .as_table_mut()
                    .unwrap()
                    .insert(key.into(), value.clone());
                let changed = run(&spec).as_ref() != Some(&reference);
                let field = fields.iter().find(|f| f.name == key).unwrap();
                let expected = prop["deprecated"] != true
                    && field.applies(&schema, spec.params.as_table().unwrap())
                    && !(route == "lsystem" && GRAMMAR_ONLY.contains(&key))
                    && !(route == "grammar" && BUILT_IN_ONLY.contains(&key));
                assert_eq!(
                    changed,
                    expected,
                    "{route}: {key} = {value} {} the tree, but the schema says it {}",
                    if changed { "changed" } else { "did not change" },
                    if expected { "applies" } else { "is ignored" }
                );
            }
        }
    }
}
