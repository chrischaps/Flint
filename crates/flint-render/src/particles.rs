//! What the renderer reads of particles (#1615, ADR 0138 decision 5): the
//! GPU instance, the blend that keys its pipeline, and one emitter's draw.
//!
//! These were flint-particles' types, which made the kernel's renderer link
//! the particles package. They are plain data, and the renderer's: the
//! particle system builds them (`ParticleSync::draw_data`) and hands the
//! frame a slice of [`ParticleDraw`]s through `FrameInputs::particles`.
//! flint-particles re-exports them, so its paths still build.

use bytemuck::{Pod, Zeroable};
use flint_core::EntityId;
use serde::{Deserialize, Serialize};

/// GPU instance data — matches WGSL `ParticleInstance` struct.
/// 64 bytes, 16-byte aligned (4 rows of vec4). This is the single
/// sim → GPU contract (ADR 0068): the simulation packs it, the instance
/// buffer holds it and a frame capture's `particles.bin` is its bytes, so a
/// future compute path only has to write the same layout.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable, PartialEq)]
pub struct ParticleInstance {
    /// xyz = world position, w = width (size.x)
    pub pos_size: [f32; 4],
    /// rgba tint
    pub color: [f32; 4],
    /// x = rotation, y = frame, z = frames_x, w = frames_y
    pub rotation_frame: [f32; 4],
    /// xyz = velocity pre-scaled by the emitter's stretch factor (all zero
    /// disables velocity-aligned stretching); w = height (size.y).
    pub vel_stretch: [f32; 4],
}

/// Blend mode for particle rendering
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
#[serde(rename_all = "lowercase")]
pub enum ParticleBlendMode {
    #[default]
    Alpha,
    Additive,
    /// Texture colour is already multiplied by alpha (`One, OneMinusSrcAlpha`).
    Premultiplied,
    /// Darkens what is behind it (`Dst, OneMinusSrcAlpha`).
    Multiply,
}

impl ParticleBlendMode {
    pub const ALL: [ParticleBlendMode; 4] = [
        ParticleBlendMode::Alpha,
        ParticleBlendMode::Additive,
        ParticleBlendMode::Premultiplied,
        ParticleBlendMode::Multiply,
    ];

    /// Stable index into per-blend pipeline arrays.
    pub fn index(self) -> usize {
        match self {
            ParticleBlendMode::Alpha => 0,
            ParticleBlendMode::Additive => 1,
            ParticleBlendMode::Premultiplied => 2,
            ParticleBlendMode::Multiply => 3,
        }
    }

    /// Additive blending is order-independent and is always drawn last.
    pub fn is_order_independent(self) -> bool {
        matches!(self, ParticleBlendMode::Additive)
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "alpha" => Some(Self::Alpha),
            "additive" => Some(Self::Additive),
            "premultiplied" => Some(Self::Premultiplied),
            "multiply" => Some(Self::Multiply),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ParticleBlendMode::Alpha => "alpha",
            ParticleBlendMode::Additive => "additive",
            ParticleBlendMode::Premultiplied => "premultiplied",
            ParticleBlendMode::Multiply => "multiply",
        }
    }
}

/// One emitter's draw: its packed instances and what the renderer draws
/// them with. The particle system makes one per emitter with alive
/// particles, in its deterministic instance order (`ParticleSync`'s
/// `EntityId` order, #858), and a frame capture's `particles.bin` and
/// `inputs.json` are written from the same slice (#1089).
#[derive(Clone, Copy, Debug, Default)]
pub struct ParticleDraw<'a> {
    /// The emitting entity; `None` for a detached burst.
    pub entity_id: Option<EntityId>,
    pub emitter_index: usize,
    pub emitter_name: &'a str,
    pub instances: &'a [ParticleInstance],
    pub blend_mode: ParticleBlendMode,
    /// The sprite's texture name; empty draws the soft disc.
    pub texture: &'a str,
    pub frames_x: u32,
    pub frames_y: u32,
    /// View distance of the emitter origin (larger = farther); renderers
    /// draw order-dependent blends far-to-near.
    pub sort_key: f32,
    pub soft_distance: f32,
    pub fade_near: f32,
    pub fade_far: f32,
    pub lighting: f32,
    pub fog: bool,
    pub stretch: f32,
}

/// A scene's particles, as a host that draws them steps them (#1622): the
/// particles package's system, which the player's frame, `flint render`'s
/// captures and the scene viewer reach through this, so none of them names
/// the package's crate. The package makes one
/// (`flint_package::particles::Particles`) and is the one implementation.
///
/// Every call is what the host did on the system itself before the
/// package was one, so a frame steps, packs and draws the same.
pub trait ParticleSource: std::any::Any + Send {
    /// The effects the scene at `scene_path` can see (`particles/*.particles.toml`,
    /// through the project's search), then the world's emitters found.
    fn load(
        &mut self,
        project: &flint_core::project::Project,
        scene_path: &str,
        world: &flint_ecs::FlintWorld,
    ) -> flint_core::Result<()>;

    /// Every texture the emitters and effects name, for the renderer to
    /// load (`load_scene_particle_textures`).
    fn texture_names(&self) -> Vec<String>;

    /// Pick up the world's emitters (a component edited, an entity gone)
    /// without moving time on.
    fn sync_from_world(&mut self, world: &flint_ecs::FlintWorld);

    /// One step of `dt` seconds: the world's emitters picked up, then every
    /// particle moved on.
    fn step(&mut self, world: &flint_ecs::FlintWorld, dt: f32);

    /// `seconds` on from where it stands, in fixed steps of `dt` (the count
    /// rounded, ADR 0068), so a capture at a time is the same run to run.
    fn simulate_to(&mut self, world: &flint_ecs::FlintWorld, seconds: f32, dt: f32);

    /// The clock emitters under a `motion_hold` emit on (#873), and the
    /// frame's whole hold rate.
    fn set_motion(&mut self, seconds: f64, whole_hold: Option<f32>);

    /// Pack the alive particles for the upload, sorted from `eye`.
    fn pack(&mut self, eye: Option<[f32; 3]>);

    /// Each emitter's draw, as packed last.
    fn draw_data(&self) -> Vec<ParticleDraw<'_>>;

    /// Particles alive.
    fn total_alive(&self) -> usize;

    /// Emitters, across every effect instance.
    fn emitter_count(&self) -> usize;

    /// Effect instances: an entity's emitter or effect, or a detached one.
    fn instance_count(&self) -> usize;

    /// Each entity whose instance plays an effect asset, with the effect's
    /// name (the viewer's F4 offers to edit each).
    fn entity_effects(&self) -> Vec<(EntityId, String)>;

    /// Every instance started again from its beginning.
    fn restart_all(&mut self);

    /// Every instance and particle dropped, for a new scene.
    fn clear(&mut self);

    /// Itself, for the package's own code to read back.
    fn as_any(&self) -> &dyn std::any::Any;

    /// Itself, for the package's own code to change.
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The instance is the WGSL struct's 64 bytes, and a capture's
    /// `particles.bin` is read back by that size (ADR 0068, #1089).
    #[test]
    fn the_instance_is_sixty_four_bytes_in_four_rows() {
        assert_eq!(std::mem::size_of::<ParticleInstance>(), 64);
        assert_eq!(std::mem::align_of::<ParticleInstance>(), 4);
        let i = ParticleInstance {
            pos_size: [1.0, 2.0, 3.0, 4.0],
            color: [5.0, 6.0, 7.0, 8.0],
            rotation_frame: [9.0, 10.0, 11.0, 12.0],
            vel_stretch: [13.0, 14.0, 15.0, 16.0],
        };
        let floats: &[f32] = bytemuck::cast_slice(std::slice::from_ref(&i));
        assert_eq!(floats, (1..=16).map(|n| n as f32).collect::<Vec<_>>());
    }
}
