//! The `audio` module: sounds a script plays, stops and fades, the music
//! clock and the master low-pass (ADR 0116). The sounds are commands the
//! host plays after the script batch; each `play_sound*` returns a handle
//! the script names it by later (#699).

use super::builder::kind::{Any, Bool, Entity, Int, Num, Str};
use super::builder::{Call, CallResult, ScriptApi};
use crate::context::{ScriptCallContext, ScriptCommand, SoundPlay, SoundRef};
use rhai::Dynamic;
use std::sync::atomic::{AtomicU64, Ordering};

/// Handles for the sounds scripts start, never reused in a run (0 is none).
static NEXT_SOUND_HANDLE: AtomicU64 = AtomicU64::new(1);

/// What `play_sound`'s opts map takes, and `play_sound_at`'s.
const PLAY_KEYS: &[&str] = &["volume", "pitch", "looping", "bus"];
const PLACE_KEYS: &[&str] = &[
    "volume",
    "pitch",
    "looping",
    "bus",
    "min_distance",
    "max_distance",
    "rolloff",
];

const HANDLE_NOTE: &str = "its handle, for stop_sound, set_sound_volume and is_sound_playing";

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("audio");

    m.func(
        "play_sound",
        "Play a sound at full volume, not placed in the world.",
    )
    .arg("name", Str)
    .returns(Int, HANDLE_NOTE)
    .run(|call, (name,)| play(call, &name, SoundPlay::default()));

    m.func(
        "play_sound",
        "Play a sound, not placed in the world: a volume (0..1), or a map of any of volume, pitch, looping (until stop_sound), bus (sfx, music or one the game declares).",
    )
    .arg("name", Str)
    .arg("volume_or_opts", Any)
    .returns(Int, HANDLE_NOTE)
    .run(|call, (name, last)| -> CallResult<i64> {
        let opts = volume_or_opts(call, "play_sound", &last, PLAY_KEYS)?;
        Ok(play(call, &name, opts.play))
    });

    m.func(
        "play_sound",
        "Play a sound at a volume and a pitch multiplier, not placed in the world.",
    )
    .arg("name", Str)
    .arg("volume", Num)
    .arg("pitch", Num)
    .returns(Int, HANDLE_NOTE)
    .run(|call, (name, volume, pitch)| {
        let opts = SoundPlay {
            volume,
            pitch,
            ..SoundPlay::default()
        };
        play(call, &name, opts)
    });

    m.func(
        "play_sound_at",
        "Play a sound at a point: a volume, or a map of any of volume, pitch, looping, bus, min_distance, max_distance, rolloff.",
    )
    .arg("name", Str)
    .arg("x", Num)
    .arg("y", Num)
    .arg("z", Num)
    .arg("volume_or_opts", Any)
    .returns(Int, HANDLE_NOTE)
    .run(|call, (name, x, y, z, last)| -> CallResult<i64> {
        // One arity, two shapes: before the builder these were two
        // overloads, a float volume and an options map
        let opts = volume_or_opts(call, "play_sound_at", &last, PLACE_KEYS)?;
        Ok(play_at(call, &name, (x, y, z), opts))
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
    .returns(Int, HANDLE_NOTE)
    .run(|call, (name, x, y, z, volume, pitch)| {
        let opts = SoundOpts {
            play: SoundPlay {
                volume,
                pitch,
                ..SoundPlay::default()
            },
            ..SoundOpts::default()
        };
        play_at(call, &name, (x, y, z), opts)
    });

    m.func(
        "stop_sound",
        "Stop a sound a script started: its handle, or a name for every one playing it.",
    )
    .arg("sound", Any)
    .run(|call, (sound,)| stop(call, &sound, 0.0));

    m.func(
        "stop_sound",
        "Stop a sound a script started (a handle or a name), fading out over seconds.",
    )
    .arg("sound", Any)
    .arg("fade", Num)
    .run(|call, (sound, fade)| stop(call, &sound, fade));

    m.func(
        "set_sound_volume",
        "Set the volume (0..1) of a sound a script started, by its handle.",
    )
    .arg("handle", Int)
    .arg("volume", Num)
    .run(|call, (h, volume)| set_volume(call, h, volume, 0.0));

    m.func(
        "set_sound_volume",
        "Fade a sound a script started to a volume (0..1) over seconds, by its handle.",
    )
    .arg("handle", Int)
    .arg("volume", Num)
    .arg("fade", Num)
    .run(|call, (h, volume, fade)| set_volume(call, h, volume, fade));

    m.func(
        "is_sound_playing",
        "Whether a sound a script started is still playing, by its handle, as the frame began: true from the frame after play_sound until it ends or is stopped. Headless (flint render --scripts, a capture) there is no sound, so always false.",
    )
    .arg("handle", Int)
    .returns(
        Bool,
        "false for a sound that ended, was stopped or never started (an unknown name, no sound device, a one-shot placed out of earshot, the web before Begin) and for an unknown handle",
    )
    .run(|call, (h,)| {
        u64::try_from(h).is_ok_and(|h| call.ctx_ref().sounds_playing.contains(&h))
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
        "set_bus_volume",
        "A mixer bus's volume (0 mutes, 1 is unity), by name: sfx, music or one the game declares in flint.toml's [audio.buses]; it scales the bus's children too.",
    )
    .arg("bus", Str)
    .arg("volume", Num)
    .run(|call, (bus, volume)| {
        call.ctx().commands.push(ScriptCommand::SetBusVolume {
            bus: bus.to_string(),
            volume: volume.max(0.0),
        });
    });

    m.func(
        "user_bus_volume",
        "The player's own volume for a bus (a settings menu's), by name, apart from the game's set_bus_volume; saved between runs.",
    )
    .arg("bus", Str)
    .returns(Num, "0..1; 1 for a bus the player never set")
    .run(|call, (bus,)| {
        call.ctx_ref()
            .user_bus_volumes
            .get(bus.as_str())
            .copied()
            .unwrap_or(1.0)
    });

    m.func(
        "set_user_bus_volume",
        "The player's own volume for a bus (0..1), for a settings menu: heard times the game's set_bus_volume, so a fade never overwrites it, and saved for the next run.",
    )
    .arg("bus", Str)
    .arg("volume", Num)
    .run(|call, (bus, volume)| {
        let volume = if volume.is_finite() {
            volume.clamp(0.0, 1.0)
        } else {
            1.0
        };
        let ctx = call.ctx();
        ctx.user_bus_volumes.insert(bus.to_string(), volume);
        ctx.commands.push(ScriptCommand::SetUserBusVolume {
            bus: bus.to_string(),
            volume,
        });
    });

    m.func(
        "set_bus_lowpass",
        "A mixer bus's own low-pass cutoff in Hz (20000 for none, as low as 10), by name: it muffles the bus and the buses under it and nothing else, and holds until changed.",
    )
    .arg("bus", Str)
    .arg("cutoff_hz", Num)
    .run(|call, (bus, cutoff_hz)| {
        call.ctx().commands.push(ScriptCommand::SetBusLowpass {
            bus: bus.to_string(),
            hz: cutoff_hz,
        });
    });

    m.func(
        "set_audio_lowpass",
        "The master bus low-pass cutoff in Hz (20000 for none), held until changed.",
    )
    .arg("cutoff_hz", Num)
    .run(|call, (cutoff_hz,)| {
        call.ctx().requests.audio_lowpass = Some(cutoff_hz as f32);
    });
}

fn next_handle() -> u64 {
    NEXT_SOUND_HANDLE.fetch_add(1, Ordering::Relaxed)
}

fn play(call: &mut Call<'_>, name: &str, play: SoundPlay) -> i64 {
    let handle = next_handle();
    call.ctx().commands.push(ScriptCommand::PlaySound {
        handle,
        name: name.to_string(),
        play,
    });
    handle as i64
}

fn play_at(call: &mut Call<'_>, name: &str, position: (f64, f64, f64), o: SoundOpts) -> i64 {
    let handle = next_handle();
    call.ctx().commands.push(ScriptCommand::PlaySoundAt {
        handle,
        name: name.to_string(),
        position,
        play: o.play,
        distances: o.distances,
        rolloff: o.rolloff,
    });
    handle as i64
}

/// `stop_sound`'s first argument: a handle (an int) or a sound's name.
fn stop(call: &mut Call<'_>, sound: &Dynamic, fade: f64) -> CallResult<()> {
    let sound = if let Ok(h) = sound.as_int() {
        SoundRef::Handle(h.max(0) as u64)
    } else if let Some(name) = sound.read_lock::<rhai::ImmutableString>() {
        SoundRef::Name(name.to_string())
    } else {
        return Err(call.error(format_args!(
            "argument `sound` must be a handle from play_sound or a sound's name, not {}",
            sound.type_name()
        )));
    };
    call.ctx().commands.push(ScriptCommand::StopSound {
        sound,
        fade: fade.max(0.0),
    });
    Ok(())
}

fn set_volume(call: &mut Call<'_>, handle: i64, volume: f64, fade: f64) {
    call.ctx().commands.push(ScriptCommand::SetSoundVolume {
        handle: handle.max(0) as u64,
        volume: volume.max(0.0),
        fade: fade.max(0.0),
    });
}

/// The last argument of `play_sound` / `play_sound_at`: a volume, or an
/// opts map of `keys`, whose problems warn once per function and key.
fn volume_or_opts(
    call: &mut Call<'_>,
    func: &'static str,
    last: &Dynamic,
    keys: &[&str],
) -> CallResult<SoundOpts> {
    if let Some(map) = last.read_lock::<rhai::Map>() {
        let (opts, problems) = SoundOpts::from_map(&map, keys);
        for p in problems {
            warn_opts(call.ctx(), func, p);
        }
        Ok(opts)
    } else if let Some(volume) = number(last) {
        Ok(SoundOpts {
            play: SoundPlay {
                volume,
                ..SoundPlay::default()
            },
            ..SoundOpts::default()
        })
    } else {
        Err(call.error(format_args!(
            "argument `volume_or_opts` must be a number or a map of options, not {}",
            last.type_name()
        )))
    }
}

fn warn_opts(c: &mut ScriptCallContext, func: &'static str, p: OptsProblem) {
    if !c.opts_warned.insert((func, p.key.clone())) {
        return;
    }
    tracing::warn!(
        "{func}: opts key \"{}\" {}; ignored. Warned once for this function and key",
        p.key,
        p.why
    );
}

/// An int or a float.
fn number(v: &Dynamic) -> Option<f64> {
    v.as_float()
        .ok()
        .or_else(|| v.as_int().ok().map(|i| i as f64))
}

/// A key of a sound's opts map that was ignored, and why.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OptsProblem {
    pub key: String,
    pub why: String,
}

/// Parsed `play_sound` / `play_sound_at` option map. Every key is
/// optional: full volume, normal pitch, once, on `sfx`, and the engine's
/// falloff. Naming only one distance keeps the engine's default (1 or 25)
/// for the other.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct SoundOpts {
    pub play: SoundPlay,
    pub distances: Option<(f32, f32)>,
    pub rolloff: Option<f32>,
}

impl SoundOpts {
    /// The options `opts` gives of `keys`, and what it got wrong: a key not
    /// in `keys` (`loop` is a Rhai keyword, so the key is `looping`), a
    /// value of the wrong shape, a bus that isn't one.
    pub(crate) fn from_map(opts: &rhai::Map, keys: &[&str]) -> (Self, Vec<OptsProblem>) {
        let mut problems = Vec::new();
        let mut problem = |key: &str, why: String| {
            problems.push(OptsProblem {
                key: key.to_string(),
                why,
            })
        };
        let mut o = Self::default();
        let (mut min, mut max) = (None, None);
        for (key, value) in opts {
            let key = key.as_str();
            if !keys.contains(&key) {
                problem(key, format!("is not one it takes ({})", keys.join(", ")));
                continue;
            }
            match key {
                "looping" => match value.as_bool() {
                    Ok(b) => o.play.looping = b,
                    Err(_) => problem(key, "should be true or false".into()),
                },
                // Any bus the game declares (#730): the engine says once
                // when a name is none and plays it on sfx.
                "bus" => match value.read_lock::<rhai::ImmutableString>() {
                    Some(bus) if !bus.is_empty() => o.play.bus = bus.to_string(),
                    _ => problem(
                        key,
                        "should be a bus's name (sfx, music or the game's)".into(),
                    ),
                },
                _ => {
                    let Some(v) = number(value) else {
                        problem(key, "should be a number".into());
                        continue;
                    };
                    match key {
                        "volume" => o.play.volume = v,
                        "pitch" => o.play.pitch = v,
                        "min_distance" => min = Some(v as f32),
                        "max_distance" => max = Some(v as f32),
                        _ => o.rolloff = Some(v as f32),
                    }
                }
            }
        }
        o.distances =
            (min.is_some() || max.is_some()).then(|| (min.unwrap_or(1.0), max.unwrap_or(25.0)));
        (o, problems)
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

    /// #1000: `is_sound_playing` reads the host's set of playing handles,
    /// which nothing fills headless; an unknown or negative handle is false.
    #[test]
    fn is_sound_playing_reads_the_hosts_set() {
        let mut system = crate::ScriptSystem::new();
        let ask = |system: &crate::ScriptSystem, s: &str| -> bool {
            system.engine.rhai_engine().eval(s).unwrap()
        };
        let h: i64 = system
            .engine
            .rhai_engine()
            .eval(r#"play_sound("hum")"#)
            .unwrap();
        let asks = format!("is_sound_playing({h})");
        assert!(!ask(&system, &asks), "no host has said so: headless");
        system.set_sounds_playing([h as u64, 7]);
        assert!(ask(&system, &asks));
        assert!(ask(&system, "is_sound_playing(7)"));
        assert!(!ask(&system, "is_sound_playing(8)"), "unknown");
        assert!(!ask(&system, "is_sound_playing(-1)"), "never a handle");
        system.set_sounds_playing([7]);
        assert!(!ask(&system, &asks), "each frame's set replaces the last");
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
                    play,
                    distances,
                    ..
                } => (name, position, play.volume, play.pitch, distances),
                ScriptCommand::PlaySound { name, play, .. } => {
                    (name, (0.0, 0.0, 0.0), play.volume, play.pitch, None)
                }
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

    /// #699: a sound a script starts comes back as a handle, which stops
    /// it, fades it and sets its volume; a name stops every one by that
    /// name; and the opts map loops it and picks its bus.
    #[test]
    fn a_handle_from_play_sound_stops_and_fades_it() {
        use crate::context::{SoundPlay, SoundRef};
        let engine = ScriptEngine::new();
        let handles: rhai::Array = engine
            .rhai_engine()
            .eval(
                r#"
                let hum = play_sound("hum", #{ looping: true, bus: "music", volume: 0.5 });
                let tap = play_sound_at("tap", 1, 2, 3, #{ looping: true, max_distance: 40 });
                let once = play_sound("ding");
                set_sound_volume(hum, 0.2, 1.5);
                set_sound_volume(tap, 0);
                stop_sound(hum, 2);
                stop_sound(tap);
                stop_sound("ding", 0.25);
                [hum, tap, once]
                "#,
            )
            .unwrap();
        let h: Vec<u64> = handles.iter().map(|d| d.as_int().unwrap() as u64).collect();
        assert!(
            h[0] > 0 && h[1] > h[0] && h[2] > h[1],
            "fresh handles: {h:?}"
        );
        let cmds = std::mem::take(&mut engine.ctx.lock().unwrap().commands);
        let hum = SoundPlay {
            volume: 0.5,
            looping: true,
            bus: "music".into(),
            ..SoundPlay::default()
        };
        let tap = SoundPlay {
            looping: true,
            ..SoundPlay::default()
        };
        assert!(matches!(&cmds[0],
            ScriptCommand::PlaySound { handle, name, play }
                if *handle == h[0] && name == "hum" && *play == hum));
        assert!(matches!(&cmds[1],
            ScriptCommand::PlaySoundAt { handle, play, distances, .. }
                if *handle == h[1] && *play == tap && *distances == Some((1.0, 40.0))));
        assert!(matches!(&cmds[2],
            ScriptCommand::PlaySound { handle, play, .. }
                if *handle == h[2] && *play == SoundPlay::default()));
        let rest: Vec<String> = cmds[3..].iter().map(|c| format!("{c:?}")).collect();
        assert_eq!(
            rest,
            [
                format!(
                    "SetSoundVolume {{ handle: {}, volume: 0.2, fade: 1.5 }}",
                    h[0]
                ),
                format!(
                    "SetSoundVolume {{ handle: {}, volume: 0.0, fade: 0.0 }}",
                    h[1]
                ),
                format!(
                    "StopSound {{ sound: {:?}, fade: 2.0 }}",
                    SoundRef::Handle(h[0])
                ),
                format!(
                    "StopSound {{ sound: {:?}, fade: 0.0 }}",
                    SoundRef::Handle(h[1])
                ),
                format!(
                    "StopSound {{ sound: {:?}, fade: 0.25 }}",
                    SoundRef::Name("ding".into())
                ),
            ]
        );
        let err = engine
            .rhai_engine()
            .run("stop_sound(#{})")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("stop_sound") && err.contains("handle"),
            "{err}"
        );
    }

    /// A wrong key (`loop` is Rhai's keyword: the key is `looping`), a
    /// wrong shape or an unknown bus is ignored with a warning, once per
    /// function and key; the rest of the map still counts.
    #[test]
    fn sound_opts_name_what_they_ignore() {
        use super::{SoundOpts, PLACE_KEYS, PLAY_KEYS};
        let engine = ScriptEngine::new();
        let map: rhai::Map = engine
            .rhai_engine()
            .eval(r#"#{ "loop": true, looping: 1, bus: 3, pitch: 2, max_distance: 9 }"#)
            .unwrap();
        let (o, problems) = SoundOpts::from_map(&map, PLAY_KEYS);
        assert_eq!(o.play.pitch, 2.0);
        assert!(!o.play.looping && o.play.bus == "sfx" && o.distances.is_none());
        let mut keys: Vec<&str> = problems.iter().map(|p| p.key.as_str()).collect();
        keys.sort();
        assert_eq!(keys, ["bus", "loop", "looping", "max_distance"]);
        let (o, problems) = SoundOpts::from_map(&map, PLACE_KEYS);
        assert_eq!(o.distances, Some((1.0, 9.0)));
        assert_eq!(problems.len(), 3, "max_distance is play_sound_at's");

        let warn = r#"play_sound("a", #{ "loop": true }); play_sound("b", #{ "loop": true });"#;
        engine.rhai_engine().run(warn).unwrap();
        assert_eq!(engine.ctx.lock().unwrap().opts_warned.len(), 1, "once");
    }
}
