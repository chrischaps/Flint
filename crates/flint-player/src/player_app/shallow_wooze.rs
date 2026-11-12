//! Shallow wooze in the player (#215): what a `shallow_wooze` component asks of the
//! renderer each frame.
//!
//! A component covers its entity and everything below it, because a hand-made bench
//! is usually several entities and a GLB can expand into several. The topmost
//! entity with a component holds the clock for its whole group, read where it
//! stands, so a group that crosses the edge of a zone can't drift apart. A component
//! further down changes its own branch's amplitude or wavelength, or with an
//! amplitude of 0 opts the branch out, while staying on the group's clock.
//!
//! The clock counts the way a living prop's does, from the same field and the same
//! `dt`, so the two keep the same tempo side by side. A scene with no `[wooze]`
//! table never gets here.

use std::collections::HashMap;

use flint_core::components as comp;
use flint_core::toml_util::toml_f32;
use flint_core::wooze::{ShallowClock, ShallowNow, WoozeField};
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_render::{ShallowWoozeDraw, WAVELENGTH_MIN};

/// Metres at full swing, unless a component says.
pub const DEFAULT_AMPLITUDE: f32 = 0.03;
/// Metres from one swell to the next, unless a component says.
pub const DEFAULT_WAVELENGTH: f32 = 1.0;

/// One entity's place in a group: whose clock it's on, and which component it obeys.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Member {
    entity: EntityId,
    clock: EntityId,
    component: EntityId,
}

/// Every group's clock, and who's in each group.
#[derive(Default, Clone)]
pub struct ShallowWooze {
    clocks: HashMap<EntityId, ShallowClock>,
    members: Vec<Member>,
    /// `(entities, components)` the members were worked out for. Walking the tree
    /// costs a scan per entity, so it's done when these change, and so a script that
    /// reparents something into a group at run time isn't noticed until they do.
    built_for: (usize, usize),
    /// What each clock says this frame, kept to save allocating it.
    now: HashMap<EntityId, ShallowNow>,
    draws: Vec<(EntityId, ShallowWoozeDraw)>,
}

impl ShallowWooze {
    /// Move every clock on by `dt` at `time`, and say what each entity draws with.
    /// With nothing in the scene to drive, it does no more than look that up.
    pub fn advance(
        &mut self,
        world: &FlintWorld,
        field: &WoozeField,
        time: f64,
        dt: f64,
    ) -> &[(EntityId, ShallowWoozeDraw)] {
        self.draws.clear();
        let with_component = world.entities_with_component(comp::SHALLOW_WOOZE).len();
        let key = (world.entity_count(), with_component);
        if key != self.built_for {
            self.members = members_of(world);
            self.built_for = key;
            let members = &self.members;
            self.clocks
                .retain(|id, _| members.iter().any(|member| member.clock == *id));
        }
        if self.members.is_empty() {
            return &self.draws;
        }

        self.now.clear();
        for member in &self.members {
            let id = member.clock;
            if self.now.contains_key(&id) {
                continue;
            }
            let position = world
                .get_world_position(id)
                .map_or([0.0; 3], |p| [p.x, p.y, p.z]);
            let sample = field.sample(position, time);
            let clock = self
                .clocks
                .entry(id)
                .or_insert_with(|| ShallowClock::starting(time - dt, &sample));
            self.now.insert(id, clock.advance(dt, &sample));
        }

        for member in &self.members {
            let (amplitude, wavelength) = settings(world, member.component);
            if amplitude <= 0.0 {
                continue;
            }
            let now = self.now[&member.clock];
            self.draws.push((
                member.entity,
                ShallowWoozeDraw {
                    amplitude: amplitude * now.amplitude,
                    wavelength,
                    index: now.index,
                    weight: now.weight,
                },
            ));
        }
        &self.draws
    }

    /// What each entity draws with, as of the last [`Self::advance`].
    pub fn draws(&self) -> &[(EntityId, ShallowWoozeDraw)] {
        &self.draws
    }

    /// How many entities were drawn with it this frame, for `--exit-after`.
    pub fn draw_count(&self) -> usize {
        self.draws.len()
    }

    /// How many clocks are running, for `--exit-after`.
    pub fn clock_count(&self) -> usize {
        self.clocks.len()
    }
}

/// A `shallow_wooze` component does nothing without a field to read, which would
/// otherwise be a silent way for a mesh to hold still.
pub fn warn_without_a_field(world: &FlintWorld, field: Option<&WoozeField>) {
    let with_it = world.entities_with_component(comp::SHALLOW_WOOZE).len();
    if field.is_none() && with_it > 0 {
        tracing::warn!(
            "{with_it} entities have shallow_wooze and this scene has no [wooze] table, so they hold still: give it one, even an empty one, to opt in"
        );
    }
}

/// An entity's `amplitude` and `wavelength`, with a bad value meaning its default.
fn settings(world: &FlintWorld, entity: EntityId) -> (f32, f32) {
    let read = |key: &str| {
        world
            .get_component(entity, comp::SHALLOW_WOOZE)
            .and_then(|table| table.get(key))
            .and_then(toml_f32)
            .filter(|value| value.is_finite())
    };
    (
        read("amplitude").unwrap_or(DEFAULT_AMPLITUDE),
        read("wavelength")
            .unwrap_or(DEFAULT_WAVELENGTH)
            .max(WAVELENGTH_MIN),
    )
}

/// Everyone a `shallow_wooze` component covers.
fn members_of(world: &FlintWorld) -> Vec<Member> {
    let mut members = Vec::new();
    let has = |id: EntityId| world.get_component(id, comp::SHALLOW_WOOZE).is_some();
    for &component in world.entities_with_component(comp::SHALLOW_WOOZE) {
        // The topmost component above it holds the clock.
        let mut clock = component;
        let mut above = world.get_parent(component);
        while let Some(parent) = above {
            if has(parent) {
                clock = parent;
            }
            above = world.get_parent(parent);
        }
        // It covers its branch, down to any component that speaks for itself.
        let mut branch = vec![component];
        while let Some(entity) = branch.pop() {
            members.push(Member {
                entity,
                clock,
                component,
            });
            branch.extend(
                world
                    .get_children(entity)
                    .into_iter()
                    .filter(|child| !has(*child)),
            );
        }
    }
    members
}

/// Woozy grass (#252): a grass field with `grass.wooze` sways on the wooze field's
/// clock instead of the wall clock, and is combed each beat by the noise shallow
/// wooze moves meshes by.
///
/// One clock for the whole field, read where the viewer stands, like the shading
/// boil's: the grass compute knows nothing of zones, and the grass a viewer sees is
/// the grass round them. It counts beats as a [`ShallowClock`] does, so the meadow
/// changes drawing on the same beat as a bench standing in it.
#[derive(Debug, Clone, Default)]
pub struct GrassWooze {
    clock: Option<ShallowClock>,
    /// Seconds of sway a beat is worth: the field's step where it started, so the
    /// sway keeps its authored speed on average and slows where the field does.
    seconds_per_beat: f64,
    now: Option<ShallowNow>,
    /// The comb's `(amplitude, wavelength)` as of the last advance.
    comb: (f32, f32),
}

impl GrassWooze {
    /// Move on by `dt` at `time` with the viewer at `viewer`, if `config` asks for
    /// it. Grass that doesn't ask goes back to the wall clock.
    pub fn advance(
        &mut self,
        config: Option<&flint_terrain::GrassConfig>,
        field: &WoozeField,
        viewer: [f32; 3],
        time: f64,
        dt: f64,
    ) {
        let Some(config) = config.filter(|c| c.enabled && c.wooze) else {
            self.clock = None;
            self.now = None;
            return;
        };
        let sample = field.sample(viewer, time);
        if self.clock.is_none() {
            self.seconds_per_beat = f64::from(sample.step.max(flint_core::wooze::STEP_MIN));
        }
        let clock = self
            .clock
            .get_or_insert_with(|| ShallowClock::starting(time - dt, &sample));
        self.now = Some(clock.advance(dt, &sample));
        self.comb = (
            config.wooze_amplitude.max(0.0),
            config.wooze_wavelength.max(WAVELENGTH_MIN),
        );
    }

    /// What `SceneRenderer::grass_time` should be, or `None` for the wall clock.
    /// It holds for the first part of each beat and glides over the rest, as a
    /// tree's pose does, so the blades rest and then sweep on together.
    pub fn sway_time(&self) -> Option<f32> {
        self.now
            .map(|now| ((now.index as f64 + f64::from(now.weight)) * self.seconds_per_beat) as f32)
    }

    /// What `SceneRenderer::grass_wooze` should be.
    pub fn comb(&self) -> Option<ShallowWoozeDraw> {
        self.now.map(|now| ShallowWoozeDraw {
            amplitude: self.comb.0 * now.amplitude,
            wavelength: self.comb.1,
            index: now.index,
            weight: now.weight,
        })
    }

    /// Set the renderer's grass clock and comb from this, or `wall` seconds of the
    /// wall clock when the grass isn't woozy.
    pub fn apply(&self, renderer: &mut flint_render::SceneRenderer, wall: f64) {
        renderer.grass_time = self.sway_time().unwrap_or(wall as f32);
        renderer.grass_wooze = self.comb();
    }
}

/// `grass.wooze` does nothing without a field, which would otherwise be a silent
/// way for a meadow to keep the wall clock.
pub fn warn_grass_without_a_field(
    config: Option<&flint_terrain::GrassConfig>,
    field: Option<&WoozeField>,
) {
    if field.is_none() && config.is_some_and(|c| c.enabled && c.wooze) {
        tracing::warn!(
            "grass.wooze is on and this scene has no [wooze] table, so the grass keeps the wall clock: give it one to opt in"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flint_core::wooze::{WoozeSample, STEP_MIN};

    fn woozy_grass() -> flint_terrain::GrassConfig {
        flint_terrain::GrassConfig {
            enabled: true,
            wooze: true,
            wooze_amplitude: 0.2,
            wooze_wavelength: 3.0,
            ..Default::default()
        }
    }

    #[test]
    fn grass_that_doesnt_ask_keeps_the_wall_clock() {
        let mut grass = GrassWooze::default();
        let plain = flint_terrain::GrassConfig {
            enabled: true,
            ..Default::default()
        };
        grass.advance(Some(&plain), &field(0.5), [0.0; 3], 0.1, 0.1);
        assert_eq!(grass.sway_time(), None);
        assert_eq!(grass.comb(), None);
        grass.advance(None, &field(0.5), [0.0; 3], 0.2, 0.1);
        assert_eq!(grass.sway_time(), None);
    }

    #[test]
    fn woozy_grass_holds_then_glides_and_keeps_its_speed_on_average() {
        // Half-second steps, gliding over the last half of each.
        let field = WoozeField::new(WoozeSample {
            step: 0.5,
            glide: 0.5,
            ..WoozeSample::NEUTRAL
        });
        let config = woozy_grass();
        let mut grass = GrassWooze::default();
        let dt = 1.0 / 60.0;
        let mut time = 0.0;
        let mut sway = Vec::new();
        for _ in 0..600 {
            time += dt;
            grass.advance(Some(&config), &field, [0.0; 3], time, dt);
            sway.push(grass.sway_time().unwrap());
        }
        // Held through the first part of a beat: from 0.05 s to 0.2 s in, nothing moves.
        let beat = 60 * 3; // frames into the 7th beat (3 s = 6 beats)
        assert_eq!(sway[beat + 3], sway[beat + 12]);
        // and moving in the glide.
        assert!(sway[beat + 27] > sway[beat + 18]);
        // Ten seconds on the field's clock is ten seconds of sway.
        assert!((f64::from(sway[599]) - 10.0).abs() < 1e-3, "{}", sway[599]);
    }

    #[test]
    fn the_comb_takes_the_fields_amplitude_and_a_benchs_beat() {
        let field = WoozeField::new(WoozeSample {
            amplitude: 0.5,
            step: 0.5,
            ..WoozeSample::NEUTRAL
        });
        let mut world = FlintWorld::new();
        let bench = at(&mut world, "bench", [0.0; 3]);
        wooze(&mut world, bench, "");
        let mut shallow = ShallowWooze::default();
        let mut grass = GrassWooze::default();
        let config = woozy_grass();
        let dt = 1.0 / 60.0;
        let mut time = 0.0;
        for _ in 0..200 {
            time += dt;
            let bench_now = drawn(shallow.advance(&world, &field, time, dt), bench).unwrap();
            grass.advance(Some(&config), &field, [0.0; 3], time, dt);
            let comb = grass.comb().unwrap();
            assert_eq!(
                (comb.index, comb.weight),
                (bench_now.index, bench_now.weight)
            );
            assert_eq!(comb.amplitude, 0.1);
            assert_eq!(comb.wavelength, 3.0);
        }
    }

    fn field(step: f32) -> WoozeField {
        WoozeField::new(WoozeSample {
            step,
            ..WoozeSample::NEUTRAL
        })
    }

    fn at(world: &mut FlintWorld, name: &str, position: [f32; 3]) -> EntityId {
        let id = world.spawn(name).unwrap();
        let table: toml::Table = format!(
            "position = [{:?}, {:?}, {:?}]",
            position[0], position[1], position[2]
        )
        .parse()
        .unwrap();
        world
            .set_component(id, comp::TRANSFORM, toml::Value::Table(table))
            .unwrap();
        id
    }

    fn wooze(world: &mut FlintWorld, id: EntityId, src: &str) {
        let table: toml::Table = src.parse().unwrap();
        world
            .set_component(id, comp::SHALLOW_WOOZE, toml::Value::Table(table))
            .unwrap();
    }

    fn drawn(draws: &[(EntityId, ShallowWoozeDraw)], id: EntityId) -> Option<ShallowWoozeDraw> {
        draws.iter().find(|(e, _)| *e == id).map(|(_, d)| *d)
    }

    #[test]
    fn nothing_with_the_component_means_nothing_to_draw() {
        let mut world = FlintWorld::new();
        at(&mut world, "bench", [0.0; 3]);
        let mut shallow = ShallowWooze::default();
        assert!(shallow.advance(&world, &field(0.5), 0.1, 0.1).is_empty());
        assert_eq!(shallow.clock_count(), 0);
    }

    #[test]
    fn it_takes_the_fields_amplitude_as_a_share_of_what_was_authored() {
        let mut world = FlintWorld::new();
        let bench = at(&mut world, "bench", [0.0; 3]);
        wooze(&mut world, bench, "amplitude = 0.1\nwavelength = 2.0");
        let quiet = WoozeField::new(WoozeSample {
            amplitude: 0.5,
            ..WoozeSample::NEUTRAL
        });
        let mut shallow = ShallowWooze::default();
        let draws = shallow.advance(&world, &quiet, 0.1, 0.1);
        let draw = drawn(draws, bench).unwrap();
        assert_eq!(draw.amplitude, 0.05);
        assert_eq!(draw.wavelength, 2.0);
    }

    #[test]
    fn an_empty_component_means_its_defaults_and_a_bad_wavelength_its_shortest() {
        let mut world = FlintWorld::new();
        let bench = at(&mut world, "bench", [0.0; 3]);
        wooze(&mut world, bench, "");
        let chair = at(&mut world, "chair", [2.0, 0.0, 0.0]);
        wooze(&mut world, chair, "wavelength = 0.0");
        let mut shallow = ShallowWooze::default();
        let draws = shallow.advance(&world, &field(0.5), 0.1, 0.1);
        let draw = drawn(draws, bench).unwrap();
        assert_eq!(draw.amplitude, DEFAULT_AMPLITUDE);
        assert_eq!(draw.wavelength, DEFAULT_WAVELENGTH);
        assert_eq!(drawn(draws, chair).unwrap().wavelength, WAVELENGTH_MIN);
    }

    #[test]
    fn a_group_shares_one_clock_read_where_its_top_stands() {
        // The seat stands well inside a slower zone, and the bench outside it. A
        // clock each would drift them apart.
        let mut world = FlintWorld::new();
        let bench = at(&mut world, "bench", [0.0; 3]);
        wooze(&mut world, bench, "amplitude = 0.04");
        let seat = at(&mut world, "seat", [10.0, 0.0, 0.0]);
        world.set_parent(seat, bench).unwrap();
        let slow: toml::Table = r#"
            shape = "sphere"
            center = [10, 0, 0]
            radius = 2
            step = 3.0
        "#
        .parse()
        .unwrap();
        let mut slower = field(0.25);
        slower.add_zone(&slow).unwrap();

        let mut shallow = ShallowWooze::default();
        for frame in 0..200 {
            let time = f64::from(frame + 1) / 60.0;
            let draws = shallow.advance(&world, &slower, time, 1.0 / 60.0);
            let (top, part) = (drawn(draws, bench).unwrap(), drawn(draws, seat).unwrap());
            assert_eq!(top.index, part.index);
            assert_eq!(top.weight, part.weight);
        }
        assert_eq!(shallow.clock_count(), 1);
        // 200 frames at 4 beats a second is the bench's clock, not the seat's.
        assert_eq!(drawn(&shallow.draws, bench).unwrap().index, 13);
    }

    #[test]
    fn a_part_can_ask_for_its_own_settings_or_opt_out_and_stay_on_the_clock() {
        let mut world = FlintWorld::new();
        let bench = at(&mut world, "bench", [0.0; 3]);
        wooze(&mut world, bench, "amplitude = 0.04");
        let arm = at(&mut world, "arm", [1.0, 0.0, 0.0]);
        world.set_parent(arm, bench).unwrap();
        wooze(&mut world, arm, "amplitude = 0.01");
        let finger = at(&mut world, "finger", [1.5, 0.0, 0.0]);
        world.set_parent(finger, arm).unwrap();
        let plaque = at(&mut world, "plaque", [0.0, 1.0, 0.0]);
        world.set_parent(plaque, bench).unwrap();
        wooze(&mut world, plaque, "amplitude = 0.0");
        let screw = at(&mut world, "screw", [0.0, 1.1, 0.0]);
        world.set_parent(screw, plaque).unwrap();

        let mut shallow = ShallowWooze::default();
        let draws = shallow.advance(&world, &field(0.5), 0.1, 0.1);
        assert_eq!(drawn(draws, arm).unwrap().amplitude, 0.01);
        assert_eq!(drawn(draws, finger).unwrap().amplitude, 0.01, "inherits");
        assert!(drawn(draws, plaque).is_none(), "opted out");
        assert!(drawn(draws, screw).is_none(), "with what's below it");
        assert_eq!(shallow.clock_count(), 1);
    }

    /// The ticket's "in step": a bench and a tree beside it count the same beats,
    /// apart from the tree's own phase offset, through a change of step.
    #[test]
    fn it_keeps_a_living_props_tempo_through_a_change_of_step() {
        let mut bench = ShallowClock::starting(0.0, &field(0.5).sample([0.0; 3], 0.0));
        // What `LivingProps::tick` does with a prop's clock.
        let phase = 0.37_f64;
        let mut tree_beats = phase;
        let mut field = field(0.5);
        let mut time = 0.0;
        let dt = 1.0 / 60.0;
        for frame in 0..1200 {
            if frame == 600 {
                let table: toml::Table = "step = 0.25".parse().unwrap();
                field.set_global(&table, time, 2.0).unwrap();
            }
            time += dt;
            let sample = field.sample([0.0; 3], time);
            tree_beats += dt / f64::from(sample.step.max(STEP_MIN));
            bench.advance(dt, &sample);
            assert!(
                (tree_beats - bench.beats() - phase).abs() < 1e-9,
                "frame {frame}: tree {tree_beats}, bench {}",
                bench.beats()
            );
        }
    }

    #[test]
    fn something_spawned_late_lands_on_the_beat_the_scene_has_reached() {
        let mut world = FlintWorld::new();
        let bench = at(&mut world, "bench", [0.0; 3]);
        wooze(&mut world, bench, "");
        let woozy = field(0.5);
        let mut shallow = ShallowWooze::default();
        let dt = 1.0 / 60.0;
        let mut time = 0.0;
        for _ in 0..125 {
            time += dt;
            shallow.advance(&world, &woozy, time, dt);
        }
        let chair = at(&mut world, "chair", [4.0, 0.0, 0.0]);
        wooze(&mut world, chair, "");
        time += dt;
        let draws = shallow.advance(&world, &woozy, time, dt);
        let (a, b) = (drawn(draws, bench).unwrap(), drawn(draws, chair).unwrap());
        assert_eq!(a.index, b.index);
        assert!((a.weight - b.weight).abs() < 1e-3, "{a:?} and {b:?}");
    }

    #[test]
    fn a_despawned_groups_clock_goes_with_it() {
        let mut world = FlintWorld::new();
        let bench = at(&mut world, "bench", [0.0; 3]);
        wooze(&mut world, bench, "");
        let mut shallow = ShallowWooze::default();
        shallow.advance(&world, &field(0.5), 0.1, 0.1);
        assert_eq!(shallow.clock_count(), 1);
        world.despawn(bench).unwrap();
        assert!(shallow.advance(&world, &field(0.5), 0.2, 0.1).is_empty());
        assert_eq!(shallow.clock_count(), 0);
    }
}
