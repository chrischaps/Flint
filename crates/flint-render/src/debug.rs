//! Debug visualization modes for the scene renderer

/// Shading debug mode — selects which visualization the fragment shader produces
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DebugMode {
    /// Standard PBR Cook-Torrance shading
    #[default]
    Pbr,
    /// Wireframe-only rendering (solid geometry drawn as edge lines)
    WireframeOnly,
    /// World-space normals mapped to RGB
    Normals,
    /// Linearized depth as grayscale
    Depth,
    /// UV coordinates as procedural checkerboard
    UvChecker,
    /// Albedo only, no lighting
    Unlit,
    /// Metallic (red) / Roughness (green)
    MetallicRoughness,
}

impl DebugMode {
    /// Cycle to the next debug mode
    pub fn next(self) -> Self {
        match self {
            Self::Pbr => Self::WireframeOnly,
            Self::WireframeOnly => Self::Normals,
            Self::Normals => Self::Depth,
            Self::Depth => Self::UvChecker,
            Self::UvChecker => Self::Unlit,
            Self::Unlit => Self::MetallicRoughness,
            Self::MetallicRoughness => Self::Pbr,
        }
    }

    /// GPU-side mode value written into the material uniform debug_mode field
    pub fn as_u32(self) -> u32 {
        match self {
            Self::Pbr => 0,
            Self::WireframeOnly => 0, // wireframe is handled by pipeline swap, shader stays PBR
            Self::Normals => 1,
            Self::Depth => 2,
            Self::UvChecker => 3,
            Self::Unlit => 4,
            Self::MetallicRoughness => 5,
        }
    }

    /// Human-readable label for display
    pub fn label(self) -> &'static str {
        match self {
            Self::Pbr => "PBR",
            Self::WireframeOnly => "Wireframe",
            Self::Normals => "Normals",
            Self::Depth => "Depth",
            Self::UvChecker => "UV Checker",
            Self::Unlit => "Unlit",
            Self::MetallicRoughness => "Metal/Rough",
        }
    }
}

/// Aggregated debug visualization state
pub struct DebugState {
    /// Current shading debug mode (F1 cycles)
    pub mode: DebugMode,
    /// Whether wireframe overlay is drawn on top of solid geometry (F2 toggles)
    pub wireframe_overlay: bool,
    /// Whether face-normal direction arrows are drawn (F3 toggles)
    pub show_normals: bool,
    /// Length of normal-direction arrows in model-space units
    pub normal_arrow_length: f32,
}

impl Default for DebugState {
    fn default() -> Self {
        Self {
            mode: DebugMode::Pbr,
            wireframe_overlay: false,
            show_normals: false,
            normal_arrow_length: 0.3,
        }
    }
}
