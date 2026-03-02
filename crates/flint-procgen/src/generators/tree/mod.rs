//! Tree generator — produces procedural tree meshes with bark materials.
//!
//! Tasks 3.1 (trunk) and 3.2 (branches) are complete. Task 3.3
//! (leaves/LOD) will extend this module into a full `TreeGenerator`.

mod bark;
pub mod branches;
pub mod leaves;
pub mod lod;
mod trunk;

pub use bark::generate_bark_normal_map;
pub use branches::generate_branches;
pub use leaves::generate_leaves;
pub use lod::generate_lods;
pub use trunk::{generate_trunk, TrunkOutput};

use crate::algorithms::mesh_builder::MeshBuilder;
use crate::generator::{GenerationCost, Generator};
use crate::output::{GeneratorOutput, OutputKind};
use crate::spec::ProcGenSpec;
use crate::types::{ImageData, MeshData};
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

    // ── Space colonization specific ──
    /// Crown volume shape.
    pub crown_shape: CrownShape,
    /// Crown radius.
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

    // ── L-system specific ──
    /// Number of L-system rewriting iterations.
    pub lsystem_iterations: u32,
    /// Angle variation range for stochastic L-system (degrees).
    pub lsystem_angle_variation: f32,
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

            crown_shape: CrownShape::Sphere,
            crown_radius: 3.0,
            crown_height_ratio: 1.5,
            num_attraction_points: 500,
            kill_distance: 0.5,
            influence_distance: 4.0,
            step_size: 0.3,

            lsystem_iterations: 3,
            lsystem_angle_variation: 10.0,
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

        // Space colonization params
        if let Some(v) = table.get("crown_shape") {
            let s = toml_string(v, "crown_shape")?;
            p.crown_shape = match s.as_str() {
                "sphere" | "Sphere" => CrownShape::Sphere,
                "ellipsoid" | "Ellipsoid" => CrownShape::Ellipsoid,
                "hemisphere" | "Hemisphere" => CrownShape::Hemisphere,
                other => {
                    return Err(ProcGenError::InvalidParameter {
                        name: "crown_shape".into(),
                        reason: format!(
                            "unknown shape \"{other}\"; expected \"sphere\", \"ellipsoid\", or \"hemisphere\""
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

        Ok(p)
    }
}

// ─── Leaf Style & Params ────────────────────────────────────────────────────

/// How leaf geometry is constructed at branch tips.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeafStyle {
    /// Camera-facing quads clustered at branch tips.
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
    /// Per-leaf color perturbation magnitude.
    pub color_variation: f32,
    /// Base size of a single leaf quad/mesh in meters.
    pub leaf_size: f32,
    /// Random scale variation range (e.g. 0.3 means ±30%).
    pub leaf_size_variation: f32,
    /// How many leaf quads/meshes per branch tip.
    pub leaves_per_tip: u32,
}

impl Default for LeafParams {
    fn default() -> Self {
        Self {
            style: LeafStyle::BillboardCluster,
            density: 0.8,
            color_base: [0.15, 0.35, 0.05, 1.0], // forest green (linear)
            color_variation: 0.08,
            leaf_size: 0.15,
            leaf_size_variation: 0.3,
            leaves_per_tip: 5,
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
}

// ─── Combined Tree Generation ───────────────────────────────────────────────

/// Generate a complete tree (trunk + branches) from parameters and RNG.
///
/// 1. Generates the trunk via [`generate_trunk`]
/// 2. Generates branches via [`generate_branches`] attached to the trunk tip
/// 3. Merges both meshes and returns the combined result
pub fn generate_tree(
    trunk_params: &TrunkParams,
    branch_params: &BranchParams,
    rng: &mut SeededRng,
) -> Result<TreeOutput> {
    let mut trunk_rng = rng.fork("trunk");
    let trunk = generate_trunk(trunk_params, &mut trunk_rng)?;

    let mut branch_rng = rng.fork("branches");
    let branch_output = generate_branches(
        branch_params,
        trunk.tip_position,
        trunk.tip_direction,
        trunk_params.radius_top,
        &mut branch_rng,
    )?;

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
            &leaf_params,
            &mut rng.fork("leaves"),
        )?;

        // 4. Merge tree + leaves
        let mut builder = MeshBuilder::new();
        builder.merge(&tree.mesh);
        if leaf_mesh.vertex_count() > 0 {
            builder.merge(&leaf_mesh);
        }
        builder.compute_normals();
        builder.compute_tangents();
        let mut full_mesh = builder.build();

        // Merge materials from bark + leaf
        let mut materials = tree.mesh.materials.clone();
        if leaf_mesh.vertex_count() > 0 {
            materials.extend(leaf_mesh.materials.iter().cloned());
        }
        full_mesh.materials = materials;

        // 5. Generate LODs if specified
        if let Some(ref lod_levels) = spec.lod {
            if !lod_levels.is_empty() {
                let targets: Vec<u32> = lod_levels.iter().map(|l| l.target_triangles).collect();
                let lods = generate_lods(&full_mesh, &targets);
                return Ok(GeneratorOutput::MeshWithLods(lods));
            }
        }

        Ok(GeneratorOutput::Mesh(full_mesh))
    }

    fn param_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "trunk_height": { "type": "number", "minimum": 0.01, "default": 5.0 },
                "trunk_radius_base": { "type": "number", "minimum": 0.01, "default": 0.3 },
                "trunk_radius_top": { "type": "number", "minimum": 0.0, "default": 0.1 },
                "trunk_segments": { "type": "integer", "minimum": 2, "default": 10 },
                "radial_segments": { "type": "integer", "minimum": 3, "default": 8 },
                "trunk_curve_noise": { "type": "number", "minimum": 0.0, "default": 0.1 },
                "bark_color_base": { "type": "string", "pattern": "^#?[0-9a-fA-F]{6,8}$", "default": "#573214" },
                "bark_roughness": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.9 },
                "bark_normal_strength": { "type": "number", "default": 1.0 },
                "bark_normal_resolution": { "type": "integer", "default": 256 },
                "branch_method": { "type": "string", "enum": ["lsystem", "space_colonization"], "default": "lsystem" },
                "branch_levels": { "type": "integer", "minimum": 1, "default": 3 },
                "branch_angle_min": { "type": "number", "default": 20.0 },
                "branch_angle_max": { "type": "number", "default": 40.0 },
                "branch_length_falloff": { "type": "number", "exclusiveMinimum": 0.0, "maximum": 1.0, "default": 0.7 },
                "branch_radius_falloff": { "type": "number", "exclusiveMinimum": 0.0, "maximum": 1.0, "default": 0.6 },
                "leaf_style": { "type": "string", "enum": ["billboard", "mesh", "none"], "default": "billboard" },
                "leaf_density": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.8 },
                "leaf_color_base": { "type": "string", "pattern": "^#?[0-9a-fA-F]{6,8}$", "default": "#2D8C0D" },
                "leaf_color_variation": { "type": "number", "minimum": 0.0, "default": 0.08 },
                "leaf_size": { "type": "number", "minimum": 0.01, "default": 0.15 },
                "leaf_size_variation": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.3 },
                "leaves_per_tip": { "type": "integer", "minimum": 1, "default": 5 }
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

/// Parse a hex color string (e.g. `"#8B4513"` or `"8B4513"`) into linear RGBA.
///
/// Supports 6-digit (`RRGGBB`) and 8-digit (`RRGGBBAA`) hex, with or without
/// a leading `#`. The sRGB→linear conversion uses the standard threshold
/// formula (IEC 61966-2-1).
pub fn parse_hex_color(hex: &str) -> Result<[f32; 4]> {
    let hex = hex.strip_prefix('#').unwrap_or(hex);

    let (r, g, b, a) = match hex.len() {
        6 => {
            let r = u8_from_hex(&hex[0..2], "red")?;
            let g = u8_from_hex(&hex[2..4], "green")?;
            let b = u8_from_hex(&hex[4..6], "blue")?;
            (r, g, b, 255u8)
        }
        8 => {
            let r = u8_from_hex(&hex[0..2], "red")?;
            let g = u8_from_hex(&hex[2..4], "green")?;
            let b = u8_from_hex(&hex[4..6], "blue")?;
            let a = u8_from_hex(&hex[6..8], "alpha")?;
            (r, g, b, a)
        }
        _ => {
            return Err(ProcGenError::InvalidParameter {
                name: "hex_color".into(),
                reason: format!(
                    "expected 6 or 8 hex digits (got {} chars: \"{}\")",
                    hex.len(),
                    hex
                ),
            });
        }
    };

    Ok([
        srgb_to_linear(r as f32 / 255.0),
        srgb_to_linear(g as f32 / 255.0),
        srgb_to_linear(b as f32 / 255.0),
        a as f32 / 255.0,
    ])
}

/// Convert a single sRGB channel value to linear.
fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Parse a 2-character hex string into a u8.
fn u8_from_hex(s: &str, channel: &str) -> Result<u8> {
    u8::from_str_radix(s, 16).map_err(|_| ProcGenError::InvalidParameter {
        name: "hex_color".into(),
        reason: format!("invalid hex for {channel} channel: \"{s}\""),
    })
}

/// Extract an f32 from a TOML value (handles both integer and float).
fn toml_f32(v: &toml::Value, name: &str) -> Result<f32> {
    match v {
        toml::Value::Float(f) => Ok(*f as f32),
        toml::Value::Integer(i) => Ok(*i as f32),
        _ => Err(ProcGenError::InvalidParameter {
            name: name.into(),
            reason: "expected a number".into(),
        }),
    }
}

/// Extract a u32 from a TOML integer value.
fn toml_u32(v: &toml::Value, name: &str) -> Result<u32> {
    match v {
        toml::Value::Integer(i) => {
            if *i < 0 {
                Err(ProcGenError::InvalidParameter {
                    name: name.into(),
                    reason: "must be non-negative".into(),
                })
            } else {
                Ok(*i as u32)
            }
        }
        _ => Err(ProcGenError::InvalidParameter {
            name: name.into(),
            reason: "expected an integer".into(),
        }),
    }
}

/// Extract a string from a TOML value.
fn toml_string(v: &toml::Value, name: &str) -> Result<String> {
    v.as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| ProcGenError::InvalidParameter {
            name: name.into(),
            reason: "expected a string".into(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
