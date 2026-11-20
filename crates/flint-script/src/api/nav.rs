//! The `nav` module (#969, ADR 0130): a way between two points of the
//! scene, over the floors its fixed colliders make, and an entity walked
//! along one. The grid is `flint_physics::nav`; this module keeps one grid
//! for each set of agent settings asked for, and where each walking entity
//! is along its path, in the call context's module state.

use super::builder::kind::{Array, Entity, Map as MapKind, Num};
use super::builder::{Call, CallResult, ScriptApi};
use crate::context::ScriptCallContext;
use flint_core::{components as comp, EntityId};
use flint_physics::{NavGrid, NavSettings};
use rhai::{Dynamic, Map};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

/// The keys `find_path`'s opts take.
const KEYS: &[&str] = &["radius", "height", "step", "max_slope", "cell", "max_nodes"];
/// The most grids kept at once; the oldest goes first.
const MAX_GRIDS: usize = 8;

const PATH: &str = "[#{x, y, z}, ...] on the floor, the first where `from` stands and the last \
                    where `to` does; [] when either is on no floor, no way joins them, or there is \
                    no physics";
const STEP: &str = "#{x, y, z, dir_x, dir_z, next, remaining, arrived}: where it is now, the way it \
                    went this frame (0, 0 once arrived), the index of the point it walks to, metres \
                    of path left, and whether it is at the end";

#[derive(Default)]
struct NavState {
    grids: Vec<NavGrid>,
    walks: HashMap<EntityId, Walk>,
}

/// Where an entity is along the path it was last given.
struct Walk {
    /// The path's points, hashed, to tell a new path from the same one
    key: u64,
    /// The point it is walking to
    next: usize,
    /// How high above the path it stood when it started
    lift: f32,
}

impl NavState {
    fn grid(&mut self, settings: NavSettings) -> &mut NavGrid {
        let at = match self.grids.iter().position(|g| *g.settings() == settings) {
            Some(i) => i,
            None => {
                if self.grids.len() >= MAX_GRIDS {
                    self.grids.remove(0);
                }
                self.grids.push(NavGrid::new(settings));
                self.grids.len() - 1
            }
        };
        &mut self.grids[at]
    }
}

fn warn_once(c: &mut ScriptCallContext, key: &str, message: String) {
    if c.opts_warned.insert(("find_path", key.to_string())) {
        tracing::warn!("find_path: {message}; ignored. Warned once for this function and key");
    }
}

/// The agent an opts map describes: the defaults, with each key it names.
fn read_settings(call: &mut Call, opts: Option<&Map>) -> CallResult<NavSettings> {
    let mut s = NavSettings::default();
    let Some(opts) = opts else {
        return Ok(s);
    };
    for (key, value) in opts {
        let key = key.as_str();
        let number = value
            .as_float()
            .ok()
            .or_else(|| value.as_int().ok().map(|i| i as f64));
        let Some(v) = number else {
            if KEYS.contains(&key) {
                let message = format!("opts key \"{key}\" should be a number");
                warn_once(call.ctx(), key, message);
            } else {
                let message = format!(
                    "opts key \"{key}\" is not one it takes ({})",
                    KEYS.join(", ")
                );
                warn_once(call.ctx(), key, message);
            }
            continue;
        };
        let v32 = v as f32;
        match key {
            "radius" => s.radius = v32,
            "height" => s.height = v32,
            "step" => s.step = v32,
            "max_slope" => s.max_slope_deg = v32,
            "cell" => s.cell = v32,
            "max_nodes" => s.max_nodes = v.max(0.0) as usize,
            _ => {
                let message = format!(
                    "opts key \"{key}\" is not one it takes ({})",
                    KEYS.join(", ")
                );
                warn_once(call.ctx(), key, message);
            }
        }
    }
    // A cell left out follows a radius made small, so a small agent works
    // without naming one
    if !opts.contains_key("cell") && s.cell >= 2.0 * s.radius {
        s.cell = s.radius * 0.6;
    }
    s.validate().map_err(|e| call.error(e))?;
    Ok(s)
}

fn point(p: [f32; 3]) -> Dynamic {
    let mut m = Map::new();
    m.insert("x".into(), Dynamic::from(p[0] as f64));
    m.insert("y".into(), Dynamic::from(p[1] as f64));
    m.insert("z".into(), Dynamic::from(p[2] as f64));
    Dynamic::from(m)
}

fn find(
    call: &mut Call,
    from: (f64, f64, f64),
    to: (f64, f64, f64),
    opts: Option<&Map>,
) -> CallResult<rhai::Array> {
    let settings = read_settings(call, opts)?;
    if call.physics().is_none() {
        return Ok(rhai::Array::new());
    }
    let mut state = std::mem::take(call.ctx().module_state.get_mut::<NavState>());
    let path = match call.physics() {
        Some(physics) => state.grid(settings).find_path(
            physics,
            [from.0 as f32, from.1 as f32, from.2 as f32],
            [to.0 as f32, to.1 as f32, to.2 as f32],
        ),
        None => None,
    };
    *call.ctx().module_state.get_mut::<NavState>() = state;
    Ok(path
        .map(|p| p.into_iter().map(point).collect())
        .unwrap_or_default())
}

/// A path's points, or a script error naming what isn't one.
fn read_path(call: &Call, path: &rhai::Array) -> CallResult<Vec<[f32; 3]>> {
    let num = |v: Option<&Dynamic>| -> Option<f32> {
        let v = v?;
        v.as_float()
            .ok()
            .or_else(|| v.as_int().ok().map(|i| i as f64))
            .map(|f| f as f32)
    };
    path.iter()
        .enumerate()
        .map(|(i, p)| {
            let m = p.read_lock::<Map>().ok_or_else(|| {
                call.error(format!(
                    "a path is points #{{x, y, z}}; point {i} is a {}",
                    p.type_name()
                ))
            })?;
            match (num(m.get("x")), num(m.get("y")), num(m.get("z"))) {
                (Some(x), Some(y), Some(z)) => Ok([x, y, z]),
                _ => Err(call.error(format!(
                    "a path is points #{{x, y, z}} of numbers; point {i} isn't"
                ))),
            }
        })
        .collect()
}

fn key_of(points: &[[f32; 3]]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for p in points {
        for v in p {
            v.to_bits().hash(&mut h);
        }
    }
    h.finish()
}

fn xz_dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((b[0] - a[0]).powi(2) + (b[2] - a[2]).powi(2)).sqrt()
}

/// One frame of an entity's walk: `speed * dt` metres along the path from
/// where it stands, keeping the height above the path it started at.
fn walk(call: &mut Call, id: EntityId, path: rhai::Array, speed: f64) -> CallResult<Map> {
    let points = read_path(call, &path)?;
    let dt = call.ctx_ref().delta_time as f32;
    let Some(t) = call.world()?.get_transform(id) else {
        return Ok(Map::new());
    };
    let mut at = [t.position.x, t.position.y, t.position.z];
    let mut out = Map::new();
    let mut report = |at: [f32; 3], dir: (f32, f32), next: usize, remaining: f32, arrived: bool| {
        out.insert("x".into(), Dynamic::from(at[0] as f64));
        out.insert("y".into(), Dynamic::from(at[1] as f64));
        out.insert("z".into(), Dynamic::from(at[2] as f64));
        out.insert("dir_x".into(), Dynamic::from(dir.0 as f64));
        out.insert("dir_z".into(), Dynamic::from(dir.1 as f64));
        out.insert("next".into(), Dynamic::from(next as i64));
        out.insert("remaining".into(), Dynamic::from(remaining as f64));
        out.insert("arrived".into(), Dynamic::from(arrived));
    };
    if points.is_empty() {
        report(at, (0.0, 0.0), 0, 0.0, true);
        return Ok(out);
    }

    let key = key_of(&points);
    let state = call.ctx().module_state.get_mut::<NavState>();
    let w = state.walks.entry(id).or_insert(Walk {
        key: !key,
        next: 0,
        lift: 0.0,
    });
    if w.key != key {
        // A new path: it starts where the entity stands, so walk to the
        // second point
        *w = Walk {
            key,
            next: 1.min(points.len() - 1),
            lift: at[1] - points[0][1],
        };
    }
    let lift = w.lift;
    let mut next = w.next;
    let mut budget = (speed.max(0.0) as f32) * dt;
    let start = at;
    while budget > 0.0 {
        let p = points[next];
        let target = [p[0], p[1] + lift, p[2]];
        let d = xz_dist(at, target);
        if d <= budget {
            at = target;
            budget -= d;
            if next + 1 >= points.len() {
                break;
            }
            next += 1;
        } else {
            let f = budget / d;
            for k in 0..3 {
                at[k] += (target[k] - at[k]) * f;
            }
            budget = 0.0;
        }
    }
    w_store(call.ctx(), id, next);

    let last = points[points.len() - 1];
    let arrived = next + 1 >= points.len() && xz_dist(at, last) < 1e-4;
    let mut remaining = xz_dist(at, points[next]);
    for pair in points[next..].windows(2) {
        remaining += xz_dist(pair[0], pair[1]);
    }
    let moved = xz_dist(start, at);
    let dir = if moved > 1e-6 {
        ((at[0] - start[0]) / moved, (at[2] - start[2]) / moved)
    } else {
        (0.0, 0.0)
    };
    if let Some(mut comps) = call.world_mut()?.get_components_mut(id) {
        let position =
            toml::Value::Array(at.iter().map(|&v| toml::Value::Float(v as f64)).collect());
        comps.set_field(comp::TRANSFORM, "position", position);
    }
    report(at, dir, next, remaining, arrived);
    Ok(out)
}

fn w_store(c: &mut ScriptCallContext, id: EntityId, next: usize) {
    if let Some(w) = c.module_state.get_mut::<NavState>().walks.get_mut(&id) {
        w.next = next;
    }
}

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("nav");

    m.func(
        "find_path",
        "The way from one point to another over the floors the scene's fixed colliders make, for an agent 0.8 m across and 1.8 m tall that climbs 0.35 m steps and 45-degree slopes.",
    )
    .arg("from_x", Num)
    .arg("from_y", Num)
    .arg("from_z", Num)
    .arg("to_x", Num)
    .arg("to_y", Num)
    .arg("to_z", Num)
    .returns(Array, PATH)
    .run(|call, (fx, fy, fz, tx, ty, tz)| find(call, (fx, fy, fz), (tx, ty, tz), None));

    m.func(
        "find_path",
        "`find_path` for the agent `opts` describes: `radius`, `height`, `step` (metres), `max_slope` (degrees), `cell` (the grid's, metres, under the agent's diameter) and `max_nodes` (the most cells a search visits).",
    )
    .arg("from_x", Num)
    .arg("from_y", Num)
    .arg("from_z", Num)
    .arg("to_x", Num)
    .arg("to_y", Num)
    .arg("to_z", Num)
    .arg("opts", MapKind)
    .returns(Array, PATH)
    .run(|call, (fx, fy, fz, tx, ty, tz, opts)| {
        find(call, (fx, fy, fz), (tx, ty, tz), Some(&opts))
    });

    m.func(
        "move_along_path",
        "Walk the entity `speed` metres a second along a path from `find_path`, keeping the height above it it started at; a new path starts again from its second point.",
    )
    .arg("id", Entity)
    .arg("path", Array)
    .arg("speed", Num)
    .returns(MapKind, STEP)
    .run(|call, (id, path, speed)| walk(call, id, path, speed));
}
