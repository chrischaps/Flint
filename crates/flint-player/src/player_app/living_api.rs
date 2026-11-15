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
    /// `wooze`, `living` and `boil`.
    Tune { knobs: toml::Value },
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

/// When the living subsystem runs, and so its three modules are enabled.
const WITH_LIVING: &str = "the scene has living props, a [living] or [wooze] table, \
                           a shallow_wooze entity or woozy grass";

/// The `living` module: Living Props' knobs, the F3 panel's by another
/// door, for every prop at once or one species (#246), and the clock
/// everything woozy runs on.
pub(crate) struct LivingModule;

/// The `wooze` module: the scene's wooze field, which every living prop and
/// shallow-wooze mesh reads, its global knobs and its named zones.
pub(crate) struct WoozeModule;

/// The `debug` module: the look-development calls that are the Living
/// Props panel's actions: the A/B comparison (F5/F6/F7) and the commit to
/// the scene file. Tools for tuning a scene, not for a shipped game.
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
        m.func(
            "tune_living",
            "Turn Living Props knobs: what a .wooze.toml holds as a map, or one species' own values with species: name.",
        )
        .arg("knobs", Map)
        .run(|call, (knobs,)| {
            queue(
                call,
                LivingCommand::Tune {
                    knobs: map_to_toml(&knobs),
                },
            );
        });

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
    }
}

impl ScriptModule for DebugModule {
    fn name(&self) -> &'static str {
        "debug"
    }

    fn enabled_when(&self) -> &'static str {
        WITH_LIVING
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
