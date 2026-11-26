//! The scene viewer's share of the wooze field (#214, #215, #252, #1558):
//! the scene's field and the clocks the viewer keeps on it, the shading
//! boil's, shallow wooze's and woozy grass's, as the living subsystem keeps
//! them, registered as `flint_viewer::field_clocks::SceneFields`.

use flint_core::field_frame::{BoilFrame, ShallowWoozeDraw};
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_viewer::field_clocks::{Clocks, SceneField, SceneFields};
use flint_wooze::shallow::{GrassWooze, ShallowWooze};
use flint_wooze::{BoilClock, WoozeField};

/// The point living fills.
pub fn fields() -> SceneFields {
    SceneFields {
        settle: flint_procgen::settle_scene,
        field_of,
        clocks: || Box::<FieldClocks>::default(),
    }
}

/// The wooze field a scene describes, naming what's wrong with it and going
/// on without that part, as the player does.
fn field_of(scene: &flint_scene::SceneFile) -> Option<SceneField> {
    let (field, issues) = flint_procgen::LivingTables::or_warn(scene).wooze?.field();
    for issue in issues {
        tracing::warn!("{issue}; going without it");
    }
    Some(Box::new(field))
}

/// The clocks the viewer keeps on the field.
#[derive(Default)]
struct FieldClocks {
    /// The shading boil's clock (#214): beats, and seconds for the field's ramps.
    boil_clock: BoilClock,
    boil_time: f64,
    /// This frame's boil drawing, for the frame's inputs.
    frame_boil: BoilFrame,
    /// Shallow wooze's and woozy grass's clocks (#215, #252), on the same
    /// field clock as the boil.
    shallow: ShallowWooze,
    grass: GrassWooze,
}

impl Clocks for FieldClocks {
    fn restart_meshes(&mut self) {
        self.shallow = Default::default();
        self.grass = Default::default();
    }

    fn tick(
        &mut self,
        field: Option<&SceneField>,
        world: &FlintWorld,
        eye: [f32; 3],
        grass_comb: Option<(f32, f32)>,
        boil_on: u32,
        dt: f64,
    ) {
        self.frame_boil = match field.and_then(|field| field.downcast_ref::<WoozeField>()) {
            Some(field) => {
                self.boil_time += dt;
                let sample = field.sample(eye, self.boil_time);
                self.boil_clock.tick(dt, &sample);
                self.shallow.advance(world, field, self.boil_time, dt);
                self.grass
                    .advance(grass_comb, field, eye, self.boil_time, dt);
                self.boil_clock.frame(&sample, boil_on)
            }
            None => Default::default(),
        };
    }

    fn boil(&self) -> BoilFrame {
        self.frame_boil
    }

    fn grass_time(&self) -> Option<f32> {
        self.grass.sway_time()
    }

    fn grass_comb(&self) -> Option<ShallowWoozeDraw> {
        self.grass.comb()
    }

    fn shallow_draws(&self) -> &[(EntityId, ShallowWoozeDraw)] {
        self.shallow.draws()
    }
}
