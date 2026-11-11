//! The `audio` module: one-shot sounds, the music clock and the master
//! low-pass (ADR 0116). The sounds are commands the host plays after the
//! script batch.

use super::builder::kind::{Any, Entity, Num, Str};
use super::builder::{Call, CallResult, ScriptApi};
use crate::context::ScriptCommand;
use rhai::Dynamic;

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("audio");

    m.func(
        "play_sound",
        "Play a sound at full volume, not placed in the world.",
    )
    .arg("name", Str)
    .run(|call, (name,)| play(call, &name, 1.0, 1.0));

    m.func(
        "play_sound",
        "Play a sound at a volume (0..1), not placed in the world.",
    )
    .arg("name", Str)
    .arg("volume", Num)
    .run(|call, (name, volume)| play(call, &name, volume, 1.0));

    m.func(
        "play_sound",
        "Play a sound at a volume and a pitch multiplier, not placed in the world.",
    )
    .arg("name", Str)
    .arg("volume", Num)
    .arg("pitch", Num)
    .run(|call, (name, volume, pitch)| play(call, &name, volume, pitch));

    m.func(
        "play_sound_at",
        "Play a sound at a point: a volume, or a map of any of volume, pitch, min_distance, max_distance, rolloff.",
    )
    .arg("name", Str)
    .arg("x", Num)
    .arg("y", Num)
    .arg("z", Num)
    .arg("volume_or_opts", Any)
    .run(|call, (name, x, y, z, last)| -> CallResult<()> {
        // One arity, two shapes: before the builder these were two
        // overloads, a float volume and an options map
        let opts = if let Some(map) = last.read_lock::<rhai::Map>() {
            SoundOpts::from_map(&map)
        } else if let Some(volume) = number(&last) {
            SoundOpts {
                volume,
                ..SoundOpts::default()
            }
        } else {
            return Err(call.error(format_args!(
                "argument `volume_or_opts` must be a number or a map of options, not {}",
                last.type_name()
            )));
        };
        play_at(call, &name, (x, y, z), opts);
        Ok(())
    });

    m.func(
        "play_sound_at",
        "Play a sound at a point, at a volume and a pitch multiplier.",
    )
    .arg("name", Str)
    .arg("x", Num)
    .arg("y", Num)
    .arg("z", Num)
    .arg("volume", Num)
    .arg("pitch", Num)
    .run(|call, (name, x, y, z, volume, pitch)| {
        let opts = SoundOpts {
            volume,
            pitch,
            ..SoundOpts::default()
        };
        play_at(call, &name, (x, y, z), opts);
    });

    m.func("stop_sound", "Stop a playing sound.")
        .arg("name", Str)
        .run(|call, (name,)| {
            call.ctx().commands.push(ScriptCommand::StopSound {
                name: name.to_string(),
            });
        });

    m.func(
        "audio_position",
        "Seconds into the sound the entity's audio_source plays, wrapped for a loop, on the mixer's clock.",
    )
    .arg("id", Entity)
    .returns(
        Num,
        "seconds; -1.0 before it has started, after it ends, and for no entity",
    )
    .or_return(-1.0_f64)
    .run(|call, (id,)| {
        call.ctx_ref()
            .audio_positions
            .get(&id)
            .copied()
            .unwrap_or(-1.0)
    });

    m.func(
        "audio_output_latency",
        "How long after it is mixed a sound is heard, where the platform says (the browser).",
    )
    .returns(Num, "seconds; -1.0 where it isn't known (the desktop)")
    .run(|call, ()| call.ctx_ref().audio_output_latency.unwrap_or(-1.0));

    m.func(
        "set_audio_lowpass",
        "The master bus low-pass cutoff in Hz (20000 for none), held until changed.",
    )
    .arg("cutoff_hz", Num)
    .run(|call, (cutoff_hz,)| {
        call.ctx().requests.audio_lowpass = Some(cutoff_hz as f32);
    });
}

fn play(call: &mut Call<'_>, name: &str, volume: f64, pitch: f64) {
    call.ctx().commands.push(ScriptCommand::PlaySound {
        name: name.to_string(),
        volume,
        pitch,
    });
}

fn play_at(call: &mut Call<'_>, name: &str, position: (f64, f64, f64), o: SoundOpts) {
    call.ctx().commands.push(ScriptCommand::PlaySoundAt {
        name: name.to_string(),
        position,
        volume: o.volume,
        pitch: o.pitch,
        distances: o.distances,
        rolloff: o.rolloff,
    });
}

/// An int or a float.
fn number(v: &Dynamic) -> Option<f64> {
    v.as_float()
        .ok()
        .or_else(|| v.as_int().ok().map(|i| i as f64))
}

/// Parsed `play_sound_at` option map. Every key is optional: full volume,
/// normal pitch, and the engine's falloff. Naming only one distance keeps
/// the engine's default (1 or 25) for the other.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SoundOpts {
    pub volume: f64,
    pub pitch: f64,
    pub distances: Option<(f32, f32)>,
    pub rolloff: Option<f32>,
}

impl Default for SoundOpts {
    fn default() -> Self {
        Self {
            volume: 1.0,
            pitch: 1.0,
            distances: None,
            rolloff: None,
        }
    }
}

impl SoundOpts {
    pub(crate) fn from_map(opts: &rhai::Map) -> Self {
        let get = |k: &str| opts.get(k).and_then(number).map(|v| v as f32);
        let (min, max) = (get("min_distance"), get("max_distance"));
        Self {
            volume: get("volume").map_or(1.0, f64::from),
            pitch: get("pitch").map_or(1.0, f64::from),
            distances: (min.is_some() || max.is_some())
                .then(|| (min.unwrap_or(1.0), max.unwrap_or(25.0))),
            rolloff: get("rolloff"),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::context::{ScriptCommand, WorldScope};
    use crate::engine::ScriptEngine;
    use flint_ecs::FlintWorld;

    #[test]
    fn a_script_reads_where_its_music_is() {
        let engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let id = world.spawn("music").unwrap();
        let src = |s: &str| s.replace("ID", &id.raw().to_string());
        let ask = |engine: &ScriptEngine, world: &mut FlintWorld, s: &str| -> f64 {
            let _scope = WorldScope::new(&engine.ctx, world);
            engine.rhai_engine().eval(&src(s)).unwrap()
        };
        assert_eq!(
            ask(&engine, &mut world, "audio_position(ID)"),
            -1.0,
            "not started"
        );
        assert_eq!(ask(&engine, &mut world, "audio_position(-1)"), -1.0);
        assert_eq!(
            ask(&engine, &mut world, "audio_output_latency()"),
            -1.0,
            "unknown"
        );
        {
            let mut c = engine.ctx.lock().unwrap();
            c.audio_positions.insert(id, 1.25);
            c.audio_output_latency = Some(0.042);
        }
        assert_eq!(ask(&engine, &mut world, "audio_position(ID)"), 1.25);
        assert_eq!(ask(&engine, &mut world, "audio_output_latency()"), 0.042);
    }

    #[test]
    fn play_sound_at_takes_a_volume_or_options_and_ints() {
        let engine = ScriptEngine::new();
        let run = |s: &str| engine.rhai_engine().run(s).unwrap();
        run("play_sound_at(\"a\", 1, 2, 3, 0.5)");
        run("play_sound_at(\"b\", 1.0, 2.0, 3.0, 1)");
        run("play_sound_at(\"c\", 0, 0, 0, #{ pitch: 2, max_distance: 80 })");
        run("play_sound_at(\"d\", 0, 0, 0, 0.5, 1.5)");
        run("play_sound(\"e\", 1)");
        let err = engine
            .rhai_engine()
            .run("play_sound_at(\"f\", 0, 0, 0, \"loud\")")
            .unwrap_err()
            .to_string();
        assert!(err.contains("volume_or_opts"), "{err}");
        let cmds = std::mem::take(&mut engine.ctx.lock().unwrap().commands);
        let got: Vec<_> = cmds
            .into_iter()
            .map(|c| match c {
                ScriptCommand::PlaySoundAt {
                    name,
                    position,
                    volume,
                    pitch,
                    distances,
                    ..
                } => (name, position, volume, pitch, distances),
                ScriptCommand::PlaySound {
                    name,
                    volume,
                    pitch,
                } => (name, (0.0, 0.0, 0.0), volume, pitch, None),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            got,
            vec![
                ("a".into(), (1.0, 2.0, 3.0), 0.5, 1.0, None),
                ("b".into(), (1.0, 2.0, 3.0), 1.0, 1.0, None),
                ("c".into(), (0.0, 0.0, 0.0), 1.0, 2.0, Some((1.0, 80.0))),
                ("d".into(), (0.0, 0.0, 0.0), 0.5, 1.5, None),
                ("e".into(), (0.0, 0.0, 0.0), 1.0, 1.0, None),
            ]
        );
    }
}
