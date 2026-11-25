//! What the wooze field's clocks hand the renderer each frame (#1330): which
//! drawing the shading boil is on, and how far each shallow-wooze mesh drifts.
//!
//! Plain data and no clock. The clocks that work these out read the field, and
//! live with it in `flint_wooze` (`BoilClock`, `ShallowWooze`, `GrassWooze`);
//! the renderer takes their answers through `FrameInputs` and names nothing of
//! the field, so a game built without living props links no `flint-wooze`.
//! Each type's default turns its feature off for the frame.

/// Which drawing the shading boil (#214) is on, and how hard it boils.
///
/// The default is no boil at all, which is what a scene with no field gets.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BoilFrame {
    /// The drawing: the same for as long as it's held, and new when it changes.
    pub seed: u32,
    /// How much of the authored boil shows, the field's `jitter` here and now.
    pub amount: f32,
    /// The field's step here and now in seconds, or 0 with no field.
    pub step: f32,
}

/// The shortest shallow-wooze wavelength, in metres. Under this it stops
/// reading as drift and starts tearing a mesh's triangles apart.
pub const WAVELENGTH_MIN: f32 = 0.05;

/// What one entity's draws do this frame under shallow wooze (#215).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShallowWoozeDraw {
    /// Metres at full swing on each axis: what's authored, times the field's
    /// amplitude where the entity stands.
    pub amplitude: f32,
    /// Metres from one swell of the noise to the next. Far above the mesh's size it
    /// moves as a whole; near it, it changes shape.
    pub wavelength: f32,
    /// The target being held or left.
    pub index: u64,
    /// How far toward the next target, 0 to 1.
    pub weight: f32,
}
