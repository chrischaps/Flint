//! Scripts reading the wooze field and its clock (#505): `wooze_at` and
//! `wooze_beat`, inside a zone and outside it, held to the clock shallow
//! wooze keeps, and `#{}` in a scene with no field.

use super::{ScriptClocks, WoozeReading};
use flint_core::EntityId;
use flint_wooze::{ShallowClock, WoozeField, WoozeSample};
use std::sync::Arc;

/// A field of 0.5 s beats, a quarter of each spent gliding, with a sphere of
/// 1 s beats and more glide two metres round (0, 0, 10). Neither glide starts
/// on a frame at 12 a second, so no frame sits on the edge of a hold.
fn field() -> WoozeField {
    let mut field = WoozeField::new(WoozeSample {
        step: 0.5,
        glide: 0.25,
        amplitude: 0.5,
        ..WoozeSample::NEUTRAL
    });
    let zone: toml::Table = toml::from_str(
        r#"
            name = "slow"
            shape = "sphere"
            center = [0, 0, 10]
            radius = 2
            falloff = 0
            step = 1.0
            glide = 0.4
        "#,
    )
    .unwrap();
    field.add_zone(&zone).unwrap();
    field
}

const INSIDE: [f32; 3] = [0.0, 0.0, 10.0];
const OUTSIDE: [f32; 3] = [0.0, 0.0, 0.0];

fn reading(field: &Arc<WoozeField>, time: f64) -> WoozeReading {
    WoozeReading {
        field: Some(field.clone()),
        time,
        scene: 1,
    }
}

fn num(map: &rhai::Map, key: &str) -> f64 {
    map.get(key)
        .unwrap_or_else(|| panic!("no {key} in {map:?}"))
        .as_float()
        .unwrap_or_else(|_| panic!("{key} isn't a float in {map:?}"))
}

/// A script's call, with the wooze module enabled and `reading` handed over.
fn eval(reading: WoozeReading, src: &str) -> rhai::Map {
    let mut system = crate::script_system();
    system.set_enabled_modules(vec!["wooze", "living", "debug"]);
    system.set_module_state(reading);
    system
        .rhai_engine_mut()
        .eval::<rhai::Map>(src)
        .unwrap_or_else(|e| panic!("{src}: {e}"))
}

#[test]
fn wooze_at_says_what_the_field_resolves_inside_a_zone_and_outside() {
    let field = Arc::new(field());
    let inside = eval(reading(&field, 3.0), "wooze_at(0, 0, 10.0)");
    assert_eq!(num(&inside, "step"), 1.0, "the zone's step");
    assert_eq!(num(&inside, "glide"), 0.4);
    assert_eq!(
        num(&inside, "amplitude"),
        0.5,
        "the global, which the zone leaves"
    );
    let zones = inside["zones"].clone().cast::<rhai::Map>();
    assert_eq!(zones.len(), 1);
    assert_eq!(
        zones["slow"].as_float().unwrap(),
        1.0,
        "all of it, at its centre"
    );

    let outside = eval(reading(&field, 3.0), "wooze_at(0.0, 0.0, 0.0)");
    assert_eq!(num(&outside, "step"), 0.5, "the global step");
    assert_eq!(num(&outside, "jitter"), 1.0);
    assert!(outside["zones"].clone().cast::<rhai::Map>().is_empty());
}

#[test]
fn the_reads_are_empty_in_a_scene_with_no_field() {
    let none = WoozeReading::default();
    assert!(eval(none.clone(), "wooze_at(0, 0, 0)").is_empty());
    assert!(eval(none, "wooze_beat(0, 0, 0)").is_empty());
}

#[test]
fn wooze_beat_is_a_map_of_the_clock() {
    let field = Arc::new(field());
    // 1.25 s into 0.5 s beats: beat 2, half way through, so still holding
    // (the glide is the last quarter)
    let beat = eval(reading(&field, 1.25), "wooze_beat(0, 0, 0)");
    assert_eq!(beat["index"].as_int().unwrap(), 2);
    assert_eq!(num(&beat, "weight"), 0.0, "held");
    assert_eq!(num(&beat, "beat"), 2.0);
    assert_eq!(num(&beat, "time"), 1.0, "two beats of half a second");
    assert_eq!(num(&beat, "delta"), 0.0, "its first read");
    assert_eq!(num(&beat, "step"), 0.5);
    assert_eq!(num(&beat, "amplitude"), 0.5);
}

/// What a bob reads, a frame at a time, from one entity's clock.
fn times_at(position: [f32; 3], frames: u32, dt: f64) -> Vec<f64> {
    let field = Arc::new(field());
    let mut clocks = ScriptClocks::default();
    let me = EntityId::from_raw(7);
    (1..=frames)
        .map(|frame| {
            let map = clocks.read(&reading(&field, f64::from(frame) * dt), me, position);
            num(&map, "time")
        })
        .collect()
}

/// Frames whose `time` changed from the frame before: `#` moves, `.` holds,
/// as `experiment.py` prints them.
fn pattern(times: &[f64]) -> String {
    times
        .windows(2)
        .map(|w| if w[1] != w[0] { '#' } else { '.' })
        .collect()
}

#[test]
fn wooze_beat_holds_on_the_beat_and_glides_between() {
    // 12 frames a second on 0.5 s beats with a quarter of glide: of each
    // six frame pairs, four hold and two move (into the glide's one frame,
    // and on to the next beat)
    let outside = pattern(&times_at(OUTSIDE, 25, 1.0 / 12.0));
    assert_eq!(outside, "...##....##....##....##.", "{outside}");
    // In the zone a beat is a second, 0.4 of it gliding: seven hold, five move
    let inside = pattern(&times_at(INSIDE, 37, 1.0 / 12.0));
    assert_eq!(inside, "......#####.......#####.......#####.", "{inside}");
}

#[test]
fn wooze_beat_keeps_the_wall_clocks_pace_on_average() {
    let times = times_at(OUTSIDE, 120, 1.0 / 60.0);
    // At a beat's start the clock and the wall agree: 2 s is 4 beats of 0.5 s
    assert!((times[119] - 2.0).abs() < 1e-9, "{}", times[119]);
}

/// The clock is shallow wooze's: a mesh on the field and a script beside it
/// read the same target and weight every frame, so they hold together.
#[test]
fn wooze_beat_keeps_the_beat_shallow_wooze_keeps() {
    let field = Arc::new(field());
    for position in [INSIDE, OUTSIDE] {
        let mut clocks = ScriptClocks::default();
        let dt = 1.0 / 60.0;
        let mut mesh: Option<ShallowClock> = None;
        for frame in 1..=300 {
            let time = f64::from(frame) * dt;
            let sample = field.sample(position, time);
            // As `ShallowWooze::advance` starts and moves one
            let mesh = mesh.get_or_insert_with(|| ShallowClock::starting(time - dt, &sample));
            let drawn = mesh.advance(dt, &sample);
            let read = clocks.read(&reading(&field, time), EntityId::from_raw(1), position);
            // The two sum their seconds in a different order, so on a beat's
            // very edge one may say the end of a glide and the other the next
            // beat's start: the same pose, and the same `beat`
            let drawn_beat = drawn.index as f64 + f64::from(drawn.weight);
            assert!(
                (num(&read, "beat") - drawn_beat).abs() < 1e-4,
                "frame {frame} at {position:?}: {read:?} against {drawn:?}"
            );
        }
    }
}

#[test]
fn a_new_step_bends_the_clock_instead_of_jumping_it() {
    let mut field = field();
    let mut clocks = ScriptClocks::default();
    let me = EntityId::from_raw(3);
    let dt = 1.0 / 60.0;
    let mut times = Vec::new();
    for frame in 1..=240 {
        let time = f64::from(frame) * dt;
        if frame == 120 {
            // Twice as slow, at once: 2 s in, 4 beats counted
            let slower: toml::Table = toml::from_str("step = 1.0").unwrap();
            field.set_global(&slower, time, 0.0).unwrap();
        }
        let map = clocks.read(&reading(&Arc::new(field.clone()), time), me, OUTSIDE);
        times.push((num(&map, "time"), num(&map, "beat")));
    }
    for pair in times.windows(2) {
        assert!(
            pair[1].0 >= pair[0].0,
            "the clock never goes back: {pair:?}"
        );
    }
    // Read as time over step alone, the beat would fall from 4 to 2 here
    assert!(times[121].1 >= 4.0, "{:?}", times[121]);
    // and from there it counts a beat a second: 2 s later, 6 beats
    assert!((times[239].1 - 6.0).abs() < 0.05, "{:?}", times[239]);
}

#[test]
fn delta_is_what_the_clock_moved_since_the_frame_before() {
    let field = Arc::new(field());
    let mut clocks = ScriptClocks::default();
    let me = EntityId::from_raw(2);
    let mut last = 0.0;
    let mut sum = 0.0;
    for frame in 1..=60 {
        let time = f64::from(frame) / 60.0;
        let first = clocks.read(&reading(&field, time), me, OUTSIDE);
        // A second read in the same frame says the same
        let again = clocks.read(&reading(&field, time), me, OUTSIDE);
        assert_eq!(num(&first, "delta"), num(&again, "delta"));
        if frame > 1 {
            assert!((num(&first, "delta") - (num(&first, "time") - last)).abs() < 1e-12);
        }
        sum += num(&first, "delta");
        last = num(&first, "time");
    }
    assert!((sum - last + times_first(&field)).abs() < 1e-12);
}

/// The time the clock reads on its first frame, which no delta covers.
fn times_first(field: &Arc<WoozeField>) -> f64 {
    let mut clocks = ScriptClocks::default();
    num(
        &clocks.read(&reading(field, 1.0 / 60.0), EntityId::from_raw(2), OUTSIDE),
        "time",
    )
}

#[test]
fn each_entity_has_its_own_clock_and_a_new_scene_starts_them_again() {
    let field = Arc::new(field());
    let mut clocks = ScriptClocks::default();
    let (a, b) = (EntityId::from_raw(1), EntityId::from_raw(2));
    clocks.read(&reading(&field, 0.1), a, OUTSIDE);
    clocks.read(&reading(&field, 0.1), b, INSIDE);
    assert_eq!(clocks.clocks.len(), 2);
    // Late to the scene, it still lands on the beat the scene has reached
    let late = clocks.read(&reading(&field, 5.0), EntityId::from_raw(9), OUTSIDE);
    assert_eq!(num(&late, "beat"), 10.0);
    // A new scene, whose clock starts again at 0
    let next = WoozeReading {
        scene: 2,
        ..reading(&field, 0.25)
    };
    let again = clocks.read(&next, a, OUTSIDE);
    assert_eq!(clocks.clocks.len(), 1);
    assert_eq!(
        num(&again, "beat"),
        0.0,
        "0.25 s into a 0.5 s beat, held at 0"
    );
}

#[test]
fn module_enabled_says_whether_the_wooze_reads_can_be_called() {
    let mut system = crate::script_system();
    let ask = r#"module_enabled("wooze")"#;
    assert!(!system.rhai_engine_mut().eval::<bool>(ask).unwrap());
    system.set_enabled_modules(vec!["wooze", "living", "debug"]);
    assert!(system.rhai_engine_mut().eval::<bool>(ask).unwrap());
}
