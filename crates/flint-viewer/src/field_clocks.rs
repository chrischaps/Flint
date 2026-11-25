//! The viewer's share of the wooze field (#214, #215, #252): the scene's field
//! and the clocks the viewer keeps on it, the shading boil's, shallow wooze's
//! and woozy grass's, as the player's living subsystem keeps them.
//!
//! All of it is the `living` feature's (default on), which links flint-wooze
//! (#1330). Without it the field is never read, every clock says nothing,
//! and a scene with a `[wooze]` table is told so once; the renderer takes
//! the clocks' answers as `flint_core::field_frame` data either way, so
//! `app.rs` reads one API in both builds.

pub(crate) use imp::*;

#[cfg(feature = "living")]
mod imp {
    use flint_core::field_frame::{BoilFrame, ShallowWoozeDraw};
    use flint_core::EntityId;
    use flint_ecs::FlintWorld;
    use flint_wooze::shallow::{GrassWooze, ShallowWooze};
    use flint_wooze::{BoilClock, WoozeField};

    /// The scene's wooze field.
    pub(crate) type SceneField = WoozeField;

    /// The wooze field a scene describes, naming what's wrong with it and going
    /// on without that part, as the player does.
    pub(crate) fn field_of(scene: &flint_scene::SceneFile) -> Option<SceneField> {
        let (field, issues) = flint_procgen::LivingTables::or_warn(scene).wooze?.field();
        for issue in issues {
            tracing::warn!("{issue}; going without it");
        }
        Some(field)
    }

    /// The clocks the viewer keeps on the field.
    #[derive(Default)]
    pub(crate) struct FieldClocks {
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

    impl FieldClocks {
        /// The scene's entities are new (a reload): their clocks start over.
        pub(crate) fn restart_meshes(&mut self) {
            self.shallow = Default::default();
            self.grass = Default::default();
        }

        /// Move every clock on by `dt`, with the viewer at `eye`. `grass_comb` is
        /// the grass's `GrassConfig::wooze_comb`, `boil_on` the drawn config's.
        /// With no field the boil is off and nothing else moves.
        pub(crate) fn tick(
            &mut self,
            field: Option<&SceneField>,
            world: &FlintWorld,
            eye: [f32; 3],
            grass_comb: Option<(f32, f32)>,
            boil_on: u32,
            dt: f64,
        ) {
            self.frame_boil = match field {
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

        /// This frame's boil drawing.
        pub(crate) fn boil(&self) -> BoilFrame {
            self.frame_boil
        }

        /// Woozy grass's sway clock, or `None` for the frame's.
        pub(crate) fn grass_time(&self) -> Option<f32> {
            self.grass.sway_time()
        }

        /// Woozy grass's comb.
        pub(crate) fn grass_comb(&self) -> Option<ShallowWoozeDraw> {
            self.grass.comb()
        }

        /// What each shallow-wooze entity draws with, as of the last tick.
        pub(crate) fn shallow_draws(&self) -> &[(EntityId, ShallowWoozeDraw)] {
            self.shallow.draws()
        }
    }
}

#[cfg(not(feature = "living"))]
mod imp {
    use flint_core::field_frame::{BoilFrame, ShallowWoozeDraw};
    use flint_core::EntityId;
    use flint_ecs::FlintWorld;

    /// No field: a viewer without `living` never reads one.
    #[derive(Debug, Clone, PartialEq)]
    pub(crate) enum SceneField {}

    /// A scene's `[wooze]` table, in a viewer built without `living`: said
    /// once, and the scene is drawn with no boil or sway.
    pub(crate) fn field_of(scene: &flint_scene::SceneFile) -> Option<SceneField> {
        if scene.tables.contains_key("wooze") {
            tracing::warn!(
                target: "viewer",
                "this viewer is built without the `living` feature: the scene's [wooze] \
                 field doesn't boil its lines or sway its meshes and grass"
            );
        }
        None
    }

    /// The clocks, all still.
    #[derive(Default)]
    pub(crate) struct FieldClocks;

    impl FieldClocks {
        pub(crate) fn restart_meshes(&mut self) {}

        pub(crate) fn tick(
            &mut self,
            field: Option<&SceneField>,
            _world: &FlintWorld,
            _eye: [f32; 3],
            _grass_comb: Option<(f32, f32)>,
            _boil_on: u32,
            _dt: f64,
        ) {
            if let Some(field) = field {
                match *field {}
            }
        }

        pub(crate) fn boil(&self) -> BoilFrame {
            BoilFrame::default()
        }

        pub(crate) fn grass_time(&self) -> Option<f32> {
            None
        }

        pub(crate) fn grass_comb(&self) -> Option<ShallowWoozeDraw> {
            None
        }

        pub(crate) fn shallow_draws(&self) -> &[(EntityId, ShallowWoozeDraw)] {
            &[]
        }
    }
}
