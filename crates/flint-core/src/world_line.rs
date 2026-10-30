//! World-space lines drawn in the 3D frame with a depth test (ADR 0083).
//!
//! A `WorldLine` is a segment between two world points with a colour and a
//! width in logical points. Scripts queue them with `draw_world_line`; the
//! renderer projects them with the camera the frame renders with and draws
//! them over the finished image, hidden wherever the scene's depth is nearer.
//! Unlike `draw_line_3d`, which projects onto the HUD, a ring drawn round a
//! model passes behind it.

/// One depth-tested world-space segment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldLine {
    /// Start, in world units.
    pub a: [f32; 3],
    /// End, in world units.
    pub b: [f32; 3],
    /// Display-space (sRGB) RGBA, the same space as the HUD's colours.
    pub color: [f32; 4],
    /// Width in logical points, the HUD's unit. Below one pixel the line
    /// fades instead of thinning.
    pub thickness: f32,
}
