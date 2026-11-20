//! The script modules Living Props owns (#698, ADR 0116 decision 5):
//! `living` (its knobs and presets, #218, #246), `wooze` (the scene's wooze
//! field, #213) and `debug` (the panel's A/B comparison, #238, and its
//! commit to the scene file, #249). They were flint-script's core; now the
//! living subsystem declares them (`subsystems/living.rs`) and enables them
//! in a scene it runs in, so a script calling `tune_living` in a scene with
//! no living props is told the module is not enabled, where it used to
//! queue a command nothing took.
//!
//! Each function queues a [`LivingCommand`] (a `ScriptCommand::Module`)
//! that `LivingSubsystem::command` carries out after the batch, in the
//! order the scripts asked, as the core's variants were before. What it
//! can't use is logged, naming the key, and the rest still happens.

use flint_script::api::builder::kind::{Array, Bool, Map, Num, Str};
use flint_script::api::{map_to_toml, Call, Module};
use flint_script::context::ScriptCommand;
use flint_script::{ModuleCommand, ScriptModule};

/// What a Living Props or wooze function asked, for the living subsystem.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum LivingCommand {
    /// `set_wooze`: the field's global knobs (any of `amplitude`, `step`,
    /// `glide`, `jitter` and `preset`), over `seconds`.
    SetWooze { knobs: toml::Value, seconds: f64 },
    /// `set_wooze_zone`: change a zone, or add it, over `seconds`.
    SetWoozeZone {
        name: String,
        fields: toml::Value,
        seconds: f64,
    },
    /// `remove_wooze_zone`: fade a zone out over `seconds` and drop it.
    RemoveWoozeZone { name: String, seconds: f64 },
    /// `tune_living`: a table shaped like a `.wooze.toml`, with any of
    /// `wooze`, `living` and `boil`, its continuous knobs eased over
    /// `seconds` (#514; 0 is a step, as it always was).
    Tune { knobs: toml::Value, seconds: f64 },
    /// `clear_living`: give a species' own values (the named keys, or all)
    /// back to the layers below.
    Clear { species: String, keys: Vec<String> },
    /// `freeze_living`: stop or start the living clock.
    Freeze { frozen: bool },
    /// `step_living`: one beat of that clock.
    Step,
    /// `save_living_preset`: every knob as `wooze/<name>.wooze.toml`.
    SavePreset { name: String },
    /// `load_living_preset`: a preset file or a built-in, live.
    LoadPreset { name: String },
    /// `commit_living`: every value the scene file doesn't say, into it.
    Commit,
    /// `compare_living`: two presets on the same grove; "" is the knobs as
    /// they stand.
    Compare { a: String, b: String },
    /// `flip_living`: the comparison's other side.
    Flip,
    /// `split_living`: both sides at once, A left and B right, or one.
    Split { split: bool },
    /// `stop_comparing_living`: end it, keeping the side shown.
    StopComparing,
}

/// Queue `command` as the function being called.
fn queue(call: &mut Call<'_>, command: LivingCommand) {
    let function = call.name();
    call.ctx()
        .commands
        .push(ScriptCommand::Module(ModuleCommand::new(function, command)));
}

/// When the living subsystem runs, and so `living` and `wooze` are enabled.
const WITH_LIVING: &str = "the scene has living props, a [living] or [wooze] table, \
                           a shallow_wooze entity or woozy grass";

/// When `debug` is enabled (#854): where the living subsystem runs, in a
/// player built with the debug HUD. The A/B and the commit are the Living
/// Props panel's tools, so a shipped build without the panel has neither.
const WITH_DEBUG_HUD: &str = "the player is built with its debug-hud feature (on by \
                              default) and the scene has living props, a [living] or \
                              [wooze] table, a shallow_wooze entity or woozy grass";

/// The modules the living subsystem enables where it runs: `debug` only in
/// a build with the `debug-hud` feature (#854), so a shipped build's
/// `compare_living` is told the module is not enabled and how to enable it.
#[cfg(feature = "debug-hud")]
pub(crate) const ENABLED: &[&str] = &["wooze", "living", "debug"];
#[cfg(not(feature = "debug-hud"))]
pub(crate) const ENABLED: &[&str] = &["wooze", "living"];

/// The `living` module: Living Props' knobs, the F3 panel's by another
/// door, for every prop at once or one species (#246), and the clock
/// everything woozy runs on.
pub(crate) struct LivingModule;

/// The `wooze` module: the scene's wooze field, which every living prop and
/// shallow-wooze mesh reads, its global knobs and its named zones.
pub(crate) struct WoozeModule;

/// The `debug` module: the look-development calls that are the Living
/// Props panel's actions: the A/B comparison (F5/F6/F7) and the commit to
/// the scene file. Tools for tuning a scene, not for a shipped game: it is
/// declared in every build, so a call is always the "not enabled" error
/// rather than an unknown function, and enabled only with `debug-hud`.
pub(crate) struct DebugModule;

pub(crate) static LIVING: LivingModule = LivingModule;
pub(crate) static WOOZE: WoozeModule = WoozeModule;
pub(crate) static DEBUG: DebugModule = DebugModule;

impl ScriptModule for LivingModule {
    fn name(&self) -> &'static str {
        "living"
    }

    fn enabled_when(&self) -> &'static str {
        WITH_LIVING
    }

    fn declare(&self, m: &mut Module<'_>) {
        fn tune(call: &mut Call<'_>, knobs: &rhai::Map, seconds: f64) {
            let knobs = map_to_toml(knobs);
            queue(call, LivingCommand::Tune { knobs, seconds });
        }

        m.func(
            "tune_living",
            "Turn Living Props knobs: what a .wooze.toml holds as a map, or one species' own values with species: name.",
        )
        .arg("knobs", Map)
        .run(|call, (knobs,)| tune(call, &knobs, 0.0));

        m.func(
            "tune_living",
            "Turn Living Props knobs over some seconds: the continuous ones (step, glide, amplitude, jitter, echoes.spread, echoes.alpha, variance.amount, variance.steady, softness, jitter_rate, row_rate) ease there as set_wooze does; the rest switch at once.",
        )
        .arg("knobs", Map)
        .arg("seconds", Num)
        .run(|call, (knobs, seconds)| tune(call, &knobs, seconds));

        m.func(
            "clear_living",
            "Give all of a species' own values back to the layers below.",
        )
        .arg("species", Str)
        .run(|call, (species,)| {
            queue(
                call,
                LivingCommand::Clear {
                    species: species.to_string(),
                    keys: Vec::new(),
                },
            );
        });

        m.func(
            "clear_living",
            "Give the keys named of a species' own values back to the layers below ([\"step\", \"echoes.count\"]).",
        )
        .arg("species", Str)
        .arg("keys", Array)
        .run(|call, (species, keys)| {
            queue(
                call,
                LivingCommand::Clear {
                    species: species.to_string(),
                    keys: keys.iter().map(|key| key.to_string()).collect(),
                },
            );
        });

        m.func(
            "freeze_living",
            "Stop or start the clock everything woozy runs on; the game goes on.",
        )
        .arg("frozen", Bool)
        .run(|call, (frozen,)| queue(call, LivingCommand::Freeze { frozen }));

        m.func(
            "step_living",
            "Move the woozy clock on by exactly one beat, for use while frozen.",
        )
        .run(|call, ()| queue(call, LivingCommand::Step));

        m.func(
            "save_living_preset",
            "Save the knobs the props agree on, and each species' own, as wooze/<name>.wooze.toml (not on the web).",
        )
        .arg("name", Str)
        .run(|call, (name,)| {
            queue(
                call,
                LivingCommand::SavePreset {
                    name: name.to_string(),
                },
            );
        });

        m.func(
            "load_living_preset",
            "Load a .wooze.toml preset or a built-in, live.",
        )
        .arg("name", Str)
        .run(|call, (name,)| {
            queue(
                call,
                LivingCommand::LoadPreset {
                    name: name.to_string(),
                },
            );
        });
    }
}

impl ScriptModule for WoozeModule {
    fn name(&self) -> &'static str {
        "wooze"
    }

    fn enabled_when(&self) -> &'static str {
        WITH_LIVING
    }

    /// Each takes an optional last argument of seconds to get there over
    /// (an int or a float); without it the change is immediate. The change
    /// reaches a living prop on the next cycle of each of its parts, or at
    /// once for step, which bends the clock. A living scene with no
    /// `[wooze]` table has no field to change, and says so.
    fn declare(&self, m: &mut Module<'_>) {
        fn set_wooze(call: &mut Call<'_>, knobs: &rhai::Map, seconds: f64) {
            let knobs = map_to_toml(knobs);
            queue(call, LivingCommand::SetWooze { knobs, seconds });
        }
        fn set_zone(call: &mut Call<'_>, name: &str, fields: &rhai::Map, seconds: f64) {
            let command = LivingCommand::SetWoozeZone {
                name: name.to_string(),
                fields: map_to_toml(fields),
                seconds,
            };
            queue(call, command);
        }
        fn remove_zone(call: &mut Call<'_>, name: &str, seconds: f64) {
            let name = name.to_string();
            queue(call, LivingCommand::RemoveWoozeZone { name, seconds });
        }

        m.func(
            "set_wooze",
            "Change the field's global knobs at once: any of amplitude, step, glide, jitter, preset.",
        )
        .arg("knobs", Map)
        .run(|call, (knobs,)| set_wooze(call, &knobs, 0.0));

        m.func(
            "set_wooze",
            "Change the field's global knobs over some seconds.",
        )
        .arg("knobs", Map)
        .arg("seconds", Num)
        .run(|call, (knobs, seconds)| set_wooze(call, &knobs, seconds));

        m.func(
            "set_wooze_zone",
            "Change a named zone at once, keeping what fields leaves out, or add it (shape, center, radius or size, a knob).",
        )
        .arg("name", Str)
        .arg("fields", Map)
        .run(|call, (name, fields)| set_zone(call, &name, &fields, 0.0));

        m.func(
            "set_wooze_zone",
            "Change or add a named zone, its knobs easing and a new zone fading in over some seconds.",
        )
        .arg("name", Str)
        .arg("fields", Map)
        .arg("seconds", Num)
        .run(|call, (name, fields, seconds)| set_zone(call, &name, &fields, seconds));

        m.func("remove_wooze_zone", "Drop a named zone at once.")
            .arg("name", Str)
            .run(|call, (name,)| remove_zone(call, &name, 0.0));

        m.func(
            "remove_wooze_zone",
            "Fade a named zone out over some seconds, then drop it.",
        )
        .arg("name", Str)
        .arg("seconds", Num)
        .run(|call, (name, seconds)| remove_zone(call, &name, seconds));

        // The reads (#505): what the living subsystem published for this
        // frame, `WoozeReading`, so scripted motion can hold with the world.
        m.func(
            "wooze_at",
            "How woozy a place is now, zones and scripts' changes included: amplitude, step, glide, jitter and the zones there; #{} where the scene has no [wooze] table.",
        )
        .arg("x", Num)
        .arg("y", Num)
        .arg("z", Num)
        .returns(
            Map,
            "#{amplitude, step, glide, jitter, zones: #{name: share}}",
        )
        .run(|call, (x, y, z)| {
            let reading = call.ctx().module_state.get_mut::<WoozeReading>().clone();
            wooze_at(&reading, [x as f32, y as f32, z as f32])
        });

        m.func(
            "wooze_beat",
            "The hold-and-glide clock at a place, on the beat shallow wooze and living props keep: time held still on the beat and gliding between, for sin(b.time * k); #{} where the scene has no [wooze] table.",
        )
        .arg("x", Num)
        .arg("y", Num)
        .arg("z", Num)
        .returns(
            Map,
            "#{time, delta, beat, index, weight, amplitude, step}: time and delta in seconds of the beat's clock",
        )
        .run(|call, (x, y, z)| {
            let ctx = call.ctx();
            let entity = ctx.current_entity;
            let reading = ctx.module_state.get_mut::<WoozeReading>().clone();
            let clocks = ctx.module_state.get_mut::<ScriptClocks>();
            clocks.read(&reading, entity, [x as f32, y as f32, z as f32])
        });
    }
}

/// What the living subsystem hands the `wooze` module's reads each frame,
/// just before scripts' `on_update` (and at a scene's start, for `on_init`):
/// the field as it stands and the time on its clock the frame will reach, so
/// a script reads the moment the props and shallow wooze are drawn at.
#[derive(Debug, Clone, Default)]
pub(crate) struct WoozeReading {
    /// `None` in a scene with no `[wooze]` table: the reads answer `#{}`.
    pub field: Option<std::sync::Arc<flint_wooze::WoozeField>>,
    /// Seconds on the field's clock (the living clock, which a game state's
    /// pause and F5's freeze stop).
    pub time: f64,
    /// Which run of the subsystem this is, so a new scene starts its scripts'
    /// clocks again.
    pub scene: u64,
}

/// The fields of `wooze_at`'s map, from `field` at `position`.
fn wooze_at(reading: &WoozeReading, position: [f32; 3]) -> rhai::Map {
    let mut map = rhai::Map::new();
    let Some(field) = &reading.field else {
        return map;
    };
    let sample = field.sample(position, reading.time);
    // Widened by the shortest decimal that is the same f32, so a knob
    // written 0.4 reads 0.4, not 0.4000000059604645
    let num = |v: f32| rhai::Dynamic::from(v.to_string().parse::<f64>().unwrap_or(f64::from(v)));
    map.insert("amplitude".into(), num(sample.amplitude));
    map.insert("step".into(), num(sample.step));
    map.insert("glide".into(), num(sample.glide));
    map.insert("jitter".into(), num(sample.jitter));
    let mut zones = rhai::Map::new();
    for (name, share) in field.zones_in(position, reading.time) {
        zones.insert(name.into(), num(share));
    }
    map.insert("zones".into(), rhai::Dynamic::from(zones));
    map
}

/// One script entity's hold-and-glide clock: a [`flint_wooze::ShallowClock`],
/// the one shallow wooze keeps, counted in beats so a change of step bends it
/// from where it stands rather than jumping.
#[derive(Debug, Clone)]
struct ScriptClock {
    clock: flint_wooze::ShallowClock,
    now: flint_wooze::ShallowNow,
    /// The field's time the clock has counted to.
    at: f64,
    /// Seconds a beat is worth to `time`: the step where it started, as
    /// woozy grass's sway does, so a motion keeps its authored speed on average.
    seconds_per_beat: f64,
    /// `time` as of the frame before, for `delta`.
    before: f64,
}

/// Every script entity's clock. One per entity that asks, read wherever it
/// last asked, so two reads by one script in a frame share a count.
#[derive(Debug, Default)]
pub(crate) struct ScriptClocks {
    scene: u64,
    clocks: std::collections::HashMap<flint_core::EntityId, ScriptClock>,
}

impl ScriptClocks {
    /// `entity`'s clock read at `position`: started on the beat the scene has
    /// reached the first time it asks, as a shallow-wooze mesh spawned late
    /// is, and moved on by however much the field's clock has since.
    fn read(
        &mut self,
        reading: &WoozeReading,
        entity: flint_core::EntityId,
        position: [f32; 3],
    ) -> rhai::Map {
        let Some(field) = &reading.field else {
            return rhai::Map::new();
        };
        if self.scene != reading.scene {
            self.scene = reading.scene;
            self.clocks.clear();
        }
        let sample = field.sample(position, reading.time);
        let time = reading.time;
        let clock = self
            .clocks
            .entry(entity)
            .and_modify(|c| {
                // The clock went back (a scene reloaded on the same run):
                // start again rather than count backwards
                if time < c.at {
                    *c = ScriptClock::starting(time, &sample);
                }
            })
            .or_insert_with(|| ScriptClock::starting(time, &sample));
        if time > clock.at {
            clock.before = clock.time();
            clock.now = clock.clock.advance(time - clock.at, &sample);
            clock.at = time;
        }
        let now = clock.now;
        let num = rhai::Dynamic::from;
        let mut map = rhai::Map::new();
        map.insert("time".into(), num(clock.time()));
        map.insert("delta".into(), num(clock.time() - clock.before));
        map.insert("beat".into(), num(now.index as f64 + f64::from(now.weight)));
        map.insert("index".into(), rhai::Dynamic::from(now.index as i64));
        map.insert("weight".into(), num(f64::from(now.weight)));
        map.insert("amplitude".into(), num(f64::from(now.amplitude)));
        map.insert("step".into(), num(f64::from(sample.step)));
        map
    }
}

impl ScriptClock {
    /// A clock at `time` where `sample` was read, on the beat the scene's
    /// clock has reached there.
    fn starting(time: f64, sample: &flint_wooze::WoozeSample) -> Self {
        let mut clock = flint_wooze::ShallowClock::starting(time, sample);
        let now = clock.advance(0.0, sample);
        let mut started = Self {
            clock,
            now,
            at: time,
            seconds_per_beat: f64::from(sample.step.max(flint_wooze::STEP_MIN)),
            before: 0.0,
        };
        started.before = started.time();
        started
    }

    /// Seconds of the beat's clock: held while the beat holds, gliding
    /// between, and keeping the wall clock's pace on average.
    fn time(&self) -> f64 {
        (self.now.index as f64 + f64::from(self.now.weight)) * self.seconds_per_beat
    }
}

impl ScriptModule for DebugModule {
    fn name(&self) -> &'static str {
        "debug"
    }

    fn enabled_when(&self) -> &'static str {
        WITH_DEBUG_HUD
    }

    fn declare(&self, m: &mut Module<'_>) {
        m.func(
            "commit_living",
            "Write every Living Props value the scene file doesn't say into it, one line each (an error on the web).",
        )
        .run(|call, ()| queue(call, LivingCommand::Commit));

        m.func(
            "compare_living",
            "Put two presets on the same grove, showing A; \"\" is the knobs as they stand.",
        )
        .arg("a", Str)
        .arg("b", Str)
        .run(|call, (a, b)| {
            queue(
                call,
                LivingCommand::Compare {
                    a: a.to_string(),
                    b: b.to_string(),
                },
            );
        });

        m.func(
            "flip_living",
            "Show the comparison's other side, on the same clock and camera.",
        )
        .run(|call, ()| queue(call, LivingCommand::Flip));

        m.func(
            "split_living",
            "Show both sides of the comparison at once, A left and B right, or one at a time.",
        )
        .arg("split", Bool)
        .run(|call, (split,)| queue(call, LivingCommand::Split { split }));

        m.func(
            "stop_comparing_living",
            "End the comparison, keeping the side that's shown.",
        )
        .run(|call, ()| queue(call, LivingCommand::StopComparing));
    }
}

#[cfg(test)]
#[path = "living_api_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "wooze_read_tests.rs"]
mod read_tests;
