//! The `camera_rig` module (#1158): a 3D camera for films and cutscenes,
//! on game time. A path through a list of points (`camera_path`,
//! `set_camera_on_path`, with a bank into its turns), a shake in the
//! camera's own frame (`shake_view`), a hand-held camera
//! (`set_hand_held`) and a shot list (`shot`).
//!
//! The path is `flint_geometry::point_path`; the shake, the hand and the
//! shots are `crate::rig`, whose jitter the drain lays over the camera the
//! scripts asked for, after all of them have set it.

use super::builder::kind::{Array, Map as MapKind, Num};
use super::builder::{Call, CallResult, ScriptApi};
use crate::rig;
use flint_core::Vec3;
use flint_geometry::point_path::PointPath;
use rhai::{Dynamic, Map};
use std::hash::{Hash, Hasher};

/// The bank's limit when the opts name none, degrees.
const MAX_BANK_DEGREES: f64 = 30.0;

const PATH_KEYS: &[&str] = &["closed"];
const ON_PATH_KEYS: &[&str] = &["closed", "look_at", "look_ahead", "bank", "max_bank"];
const SHAKE_KEYS: &[&str] = &["degrees", "seed"];
const HAND_KEYS: &[&str] = &["steps", "seed"];

fn xyz(p: Vec3) -> Map {
    let mut m = Map::new();
    m.insert("x".into(), Dynamic::from(p.x as f64));
    m.insert("y".into(), Dynamic::from(p.y as f64));
    m.insert("z".into(), Dynamic::from(p.z as f64));
    m
}

fn number(v: &Dynamic) -> Option<f64> {
    v.as_float()
        .ok()
        .or_else(|| v.as_int().ok().map(|i| i as f64))
}

/// One point, from `[x, y, z]` or `#{x, y, z}`.
fn point_of(v: &Dynamic) -> Option<Vec3> {
    if let Some(a) = v.read_lock::<rhai::Array>() {
        return match a.as_slice() {
            [x, y, z] => Some(Vec3::new(
                number(x)? as f32,
                number(y)? as f32,
                number(z)? as f32,
            )),
            _ => None,
        };
    }
    let m = v.read_lock::<Map>()?;
    Some(Vec3::new(
        number(m.get("x")?)? as f32,
        number(m.get("y")?)? as f32,
        number(m.get("z")?)? as f32,
    ))
}

/// A path's points, or a script error naming the one that isn't.
fn points_of(call: &Call, points: &rhai::Array) -> CallResult<Vec<Vec3>> {
    points
        .iter()
        .enumerate()
        .map(|(i, p)| {
            point_of(p).ok_or_else(|| {
                call.error(format!(
                    "point {i} is a {}, not [x, y, z] or #{{x, y, z}} of numbers",
                    p.type_name()
                ))
            })
        })
        .collect()
}

fn path_of(call: &Call, points: &rhai::Array, closed: bool) -> CallResult<PointPath> {
    let pts = points_of(call, points)?;
    let n = pts.len();
    PointPath::new(pts, closed).ok_or_else(|| {
        call.error(format!(
            "a {} path needs at least {} points, not all in one place; this has {n}",
            if closed { "closed" } else { "open" },
            if closed { 3 } else { 2 }
        ))
    })
}

/// An opts map, read with the keys its function takes: a key it doesn't
/// take, or a value of the wrong shape, warns once per function and key
/// (`ScriptCallContext::opts_warned`) and is ignored.
struct Opts<'m> {
    func: &'static str,
    map: Option<&'m Map>,
}

impl<'m> Opts<'m> {
    fn read(call: &mut Call, func: &'static str, map: Option<&'m Map>, keys: &[&str]) -> Self {
        if let Some(map) = map {
            for key in map.keys() {
                if !keys.contains(&key.as_str()) {
                    let message = format!(
                        "opts key \"{key}\" is not one it takes ({})",
                        keys.join(", ")
                    );
                    warn_once(call, func, key, message);
                }
            }
        }
        Self { func, map }
    }

    fn get(&self, key: &str) -> Option<&'m Dynamic> {
        self.map?.get(key)
    }

    fn number(&self, call: &mut Call, key: &str) -> Option<f64> {
        let v = self.get(key)?;
        let n = number(v);
        if n.is_none() {
            let message = format!("opts key \"{key}\" should be a number");
            warn_once(call, self.func, key, message);
        }
        n
    }

    fn flag(&self, call: &mut Call, key: &str) -> bool {
        let Some(v) = self.get(key) else {
            return false;
        };
        v.as_bool().unwrap_or_else(|_| {
            let message = format!("opts key \"{key}\" should be true or false");
            warn_once(call, self.func, key, message);
            false
        })
    }
}

fn warn_once(call: &mut Call, func: &'static str, key: &str, message: String) {
    if call.ctx().opts_warned.insert((func, key.to_string())) {
        tracing::warn!("{func}: {message}; ignored. Warned once for this function and key");
    }
}

/// Where `set_camera_on_path` looks: `look_at` a point, an entity, or a
/// second path at the same `u`; else `look_ahead` metres along the path;
/// else along it.
fn target_of(
    call: &mut Call,
    opts: &Opts,
    path: &PointPath,
    u: f32,
    frame: &flint_geometry::point_path::PathFrame,
) -> CallResult<Vec3> {
    let along = frame.position + frame.forward;
    if let Some(look) = opts.get("look_at") {
        if let Ok(id) = look.as_int() {
            let Some(id) = call.entity(id) else {
                return Ok(along);
            };
            return Ok(call.world()?.get_world_position(id).unwrap_or(along));
        }
        if let Some(p) = point_of(look) {
            return Ok(p);
        }
        if let Some(a) = look.read_lock::<rhai::Array>() {
            let target = path_of(call, &a, path.closed())?;
            return Ok(target.at(u));
        }
        warn_once(
            call,
            "set_camera_on_path",
            "look_at",
            "opts key \"look_at\" should be a point, an entity or a list of points".into(),
        );
        return Ok(along);
    }
    let ahead = opts.number(call, "look_ahead").unwrap_or(0.0) as f32;
    if ahead > 0.0 {
        let p = path.at(u + ahead / path.length());
        if (p - frame.position).length() > 1e-3 {
            return Ok(p);
        }
    }
    Ok(along)
}

fn on_path(call: &mut Call, points: rhai::Array, u: f64, map: Option<&Map>) -> CallResult<()> {
    const FUNC: &str = "set_camera_on_path";
    let opts = Opts::read(call, FUNC, map, ON_PATH_KEYS);
    let closed = opts.flag(call, "closed");
    let path = path_of(call, &points, closed)?;
    let bank = opts.number(call, "bank").unwrap_or(0.0) as f32;
    let max_bank = opts
        .number(call, "max_bank")
        .unwrap_or(MAX_BANK_DEGREES)
        .to_radians() as f32;
    let u = u as f32;
    let frame = path.frame(u, bank, max_bank);
    let target = target_of(call, &opts, &path, u, &frame)?;
    let camera = &mut call.ctx().requests.camera;
    camera.position = Some(frame.position.to_array());
    camera.target = Some(target.to_array());
    if bank != 0.0 {
        camera.roll = Some(frame.roll);
    }
    Ok(())
}

fn shake(call: &mut Call, amplitude: f64, frequency: f64, decay: f64, map: Option<&Map>) {
    let opts = Opts::read(call, "shake_view", map, SHAKE_KEYS);
    let degrees = opts.number(call, "degrees");
    let seed = opts.number(call, "seed").unwrap_or(0.0) as i64;
    let c = call.ctx();
    let t = c.total_time;
    c.camera_rig
        .shake
        .kick(t, amplitude, frequency, decay, degrees, seed);
}

fn hand(call: &mut Call, strength: f64, map: Option<&Map>) {
    let opts = Opts::read(call, "set_hand_held", map, HAND_KEYS);
    let steps = opts.number(call, "steps").unwrap_or(0.0);
    let seed = opts.number(call, "seed").unwrap_or(0.0) as i64;
    call.ctx().camera_rig.hand = rig::HandHeld {
        strength: strength.max(0.0),
        steps: steps.max(0.0),
        seed,
    };
}

fn shot(call: &mut Call, t: f64, starts: rhai::Array) -> CallResult<Map> {
    let times: Vec<f64> = starts
        .iter()
        .enumerate()
        .map(|(i, v)| {
            number(v)
                .ok_or_else(|| call.error(format!("time {i} is a {}, not a number", v.type_name())))
        })
        .collect::<CallResult<_>>()?;
    let s = rig::shot_at(t, &times).map_err(|e| call.error(e))?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for v in &times {
        v.to_bits().hash(&mut h);
    }
    let key = h.finish();
    let c = call.ctx();
    let entity = c.current_entity;
    let first = c.camera_rig.shot_began(entity, key, s.index, t);
    let mut m = Map::new();
    m.insert("index".into(), Dynamic::from(s.index));
    m.insert("time".into(), Dynamic::from(s.time));
    m.insert("length".into(), Dynamic::from(s.length));
    m.insert("u".into(), Dynamic::from(s.u));
    m.insert("first".into(), Dynamic::from(first));
    m.insert("done".into(), Dynamic::from(s.done));
    Ok(m)
}

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("camera_rig");

    m.func(
        "camera_path",
        "The point u (0..1, by distance) of the way along a smooth path through a list of points.",
    )
    .arg("points", Array)
    .arg("u", Num)
    .returns(MapKind, "#{x, y, z}")
    .run(|call, (points, u)| -> CallResult<Map> {
        Ok(xyz(path_of(call, &points, false)?.at(u as f32)))
    });
    m.func(
        "camera_path",
        "`camera_path` with opts: `closed` (true runs from the last point back to the first, and u wraps).",
    )
    .arg("points", Array)
    .arg("u", Num)
    .arg("opts", MapKind)
    .returns(MapKind, "#{x, y, z}")
    .run(|call, (points, u, opts)| -> CallResult<Map> {
        let o = Opts::read(call, "camera_path", Some(&opts), PATH_KEYS);
        let closed = o.flag(call, "closed");
        Ok(xyz(path_of(call, &points, closed)?.at(u as f32)))
    });

    m.func(
        "camera_path_length",
        "The length of the path through a list of points, metres: divide a speed by it for u a second.",
    )
    .arg("points", Array)
    .returns(Num, "metres")
    .run(|call, (points,)| -> CallResult<f64> {
        Ok(path_of(call, &points, false)?.length() as f64)
    });
    m.func(
        "camera_path_length",
        "`camera_path_length` with opts: `closed`.",
    )
    .arg("points", Array)
    .arg("opts", MapKind)
    .returns(Num, "metres")
    .run(|call, (points, opts)| -> CallResult<f64> {
        let o = Opts::read(call, "camera_path_length", Some(&opts), PATH_KEYS);
        let closed = o.flag(call, "closed");
        Ok(path_of(call, &points, closed)?.length() as f64)
    });

    m.func(
        "set_camera_on_path",
        "Put the camera u (0..1, by distance) of the way along a smooth path through a list of points, looking along it.",
    )
    .arg("points", Array)
    .arg("u", Num)
    .run(|call, (points, u)| on_path(call, points, u, None));
    m.func(
        "set_camera_on_path",
        "`set_camera_on_path` with opts: `look_at` (a point, an entity or a second path at the same u), `look_ahead` (metres along the path), `bank` (lean into turns; 5 rolls 45 degrees in a 5 m turn), `max_bank` (degrees, 30), `closed`.",
    )
    .arg("points", Array)
    .arg("u", Num)
    .arg("opts", MapKind)
    .run(|call, (points, u, opts)| on_path(call, points, u, Some(&opts)));

    m.func(
        "shake_view",
        "Shake the 3D camera across its own view: an amplitude in metres (stacking on a shake under way), about how many swings a second, and a decay rate.",
    )
    .arg("amplitude", Num)
    .arg("frequency", Num)
    .arg("decay", Num)
    .run(|call, (amplitude, frequency, decay)| shake(call, amplitude, frequency, decay, None));
    m.func(
        "shake_view",
        "`shake_view` with opts: `degrees` (its turn at the start; 10 a metre of amplitude unless given) and `seed`.",
    )
    .arg("amplitude", Num)
    .arg("frequency", Num)
    .arg("decay", Num)
    .arg("opts", MapKind)
    .run(|call, (amplitude, frequency, decay, opts)| {
        shake(call, amplitude, frequency, decay, Some(&opts))
    });

    m.func("stop_view_shake", "Stop the view shake at once.")
        .run(|call, ()| {
            call.ctx().camera_rig.shake = rig::ViewShake::default();
        });

    m.func(
        "set_hand_held",
        "Hold the 3D camera by hand from now on: a slow drift at a strength (1 a person holding it, 0 off).",
    )
    .arg("strength", Num)
    .run(|call, (strength,)| hand(call, strength, None));
    m.func(
        "set_hand_held",
        "`set_hand_held` with opts: `steps` (footfalls a second, for a walk's bob and sway; 0 stands) and `seed`.",
    )
    .arg("strength", Num)
    .arg("opts", MapKind)
    .run(|call, (strength, opts)| hand(call, strength, Some(&opts)));

    m.func(
        "shot",
        "Which shot of a list game time t is in: `starts` is each shot's start, then the end of the last.",
    )
    .arg("t", Num)
    .arg("starts", Array)
    .returns(
        MapKind,
        "#{index, time, length, u, first, done}: index -1 before the first; first is true on the frame a shot began",
    )
    .run(|call, (t, starts)| shot(call, t, starts));
}
