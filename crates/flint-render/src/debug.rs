//! Debug visualization modes for the scene renderer

/// Shading debug mode — selects which visualization the fragment shader produces
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DebugMode {
    /// Standard PBR Cook-Torrance shading
    #[default]
    Pbr,
    /// Wireframe overlay on top of solid PBR geometry
    WireframeOverlay,
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
    /// The entity style mask (#263): each `style_mask` style in its own
    /// hue over the scene, unstyled pixels grey. Drawn by the `mask_view`
    /// style, so it needs post-processing on.
    StyleMask,
    /// The ASCII style's edges (#349): each edge pixel coloured by its
    /// source (depth, crease, styled outline) and each glyph cell's
    /// decision (line glyph, yielded, texture, too little) with its edge
    /// length. Drawn by the `ascii` style in place of its glyphs (switched
    /// on at its defaults if the scene has none), so it needs
    /// post-processing on.
    StyleEdges,
}

impl DebugMode {
    /// All modes in cycle order, for UI enumeration (combo boxes)
    pub const ALL: [DebugMode; 10] = [
        Self::Pbr,
        Self::WireframeOverlay,
        Self::WireframeOnly,
        Self::Normals,
        Self::Depth,
        Self::UvChecker,
        Self::Unlit,
        Self::MetallicRoughness,
        Self::StyleMask,
        Self::StyleEdges,
    ];

    /// Cycle to the next debug mode
    pub fn next(self) -> Self {
        match self {
            Self::Pbr => Self::WireframeOverlay,
            Self::WireframeOverlay => Self::WireframeOnly,
            Self::WireframeOnly => Self::Normals,
            Self::Normals => Self::Depth,
            Self::Depth => Self::UvChecker,
            Self::UvChecker => Self::Unlit,
            Self::Unlit => Self::MetallicRoughness,
            Self::MetallicRoughness => Self::StyleMask,
            Self::StyleMask => Self::StyleEdges,
            Self::StyleEdges => Self::Pbr,
        }
    }

    /// GPU-side mode value written into the material uniform debug_mode field
    pub fn as_u32(self) -> u32 {
        match self {
            Self::Pbr => 0,
            Self::WireframeOverlay => 0, // shader stays PBR, overlay handled by pipeline
            Self::WireframeOnly => 0,    // wireframe is handled by pipeline swap, shader stays PBR
            Self::Normals => 1,
            Self::Depth => 2,
            Self::UvChecker => 3,
            Self::Unlit => 4,
            Self::MetallicRoughness => 5,
            Self::StyleMask => 0, // PBR shading; the mask_view style draws over it
            Self::StyleEdges => 0, // PBR shading; the ascii style draws its edge view
        }
    }

    /// Human-readable label for display
    pub fn label(self) -> &'static str {
        match self {
            Self::Pbr => "PBR",
            Self::WireframeOverlay => "Wireframe Overlay",
            Self::WireframeOnly => "Wireframe",
            Self::Normals => "Normals",
            Self::Depth => "Depth",
            Self::UvChecker => "UV Checker",
            Self::Unlit => "Unlit",
            Self::MetallicRoughness => "Metal/Rough",
            Self::StyleMask => "Style Mask",
            Self::StyleEdges => "Style Edges",
        }
    }
}

/// Aggregated debug visualization state
pub struct DebugState {
    /// Current shading debug mode (F1 cycles)
    pub mode: DebugMode,
    /// Whether face-normal direction arrows are drawn (F3 toggles)
    pub show_normals: bool,
    /// Length of normal-direction arrows in model-space units
    pub normal_arrow_length: f32,
    /// Whether skeleton/armature bone lines are drawn (F4 toggles)
    pub show_skeleton: bool,
}

impl Default for DebugState {
    fn default() -> Self {
        Self {
            mode: DebugMode::Pbr,
            show_normals: false,
            normal_arrow_length: 0.3,
            show_skeleton: false,
        }
    }
}
