//! The `spline` module: queries of a spline entity's sampled path, its
//! `spline_data` component (positions and `t` values in parallel arrays,
//! and the gaps where the track breaks). ADR 0116.
//!
//! The sampling is the plain functions below, over the component's TOML
//! table; the registrations only find the table.

use super::builder::kind::{Bool, Entity, Map as MapKind, Num};
use super::builder::ScriptApi;
use flint_core::toml_util::toml_f64;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use rhai::{Dynamic, Map};

/// The entity's `spline_data` table, if it has one.
fn spline_table(world: &FlintWorld, id: EntityId) -> Option<&toml::Table> {
    world
        .get_components(id)?
        .get(flint_geometry::components::SPLINE_DATA)?
        .as_table()
}

/// One of the table's number arrays.
fn numbers<'t>(table: &'t toml::Table, key: &str) -> Option<&'t [toml::Value]> {
    table.get(key)?.as_array().map(Vec::as_slice)
}

fn num(v: &toml::Value) -> f64 {
    toml_f64(v).unwrap_or(0.0)
}

/// `t` wrapped into `[0, 1)`: a closed spline goes round.
fn wrap(t: f64) -> f64 {
    ((t % 1.0) + 1.0) % 1.0
}

/// The samples' parallel arrays, cut to the shortest and to
/// `sample_count`, which must be at least `min`.
struct Samples<'t> {
    x: &'t [toml::Value],
    y: &'t [toml::Value],
    z: &'t [toml::Value],
    t: &'t [toml::Value],
}

impl<'t> Samples<'t> {
    fn of(table: &'t toml::Table, min: usize) -> Option<Self> {
        let count = table.get("sample_count")?.as_integer()?;
        if count < min as i64 {
            return None;
        }
        let (x, y, z, t) = (
            numbers(table, "positions_x")?,
            numbers(table, "positions_y")?,
            numbers(table, "positions_z")?,
            numbers(table, "t_values")?,
        );
        let len = (count as usize)
            .min(x.len())
            .min(y.len())
            .min(z.len())
            .min(t.len());
        (len >= min.max(1)).then(|| Self {
            x: &x[..len],
            y: &y[..len],
            z: &z[..len],
            t: &t[..len],
        })
    }

    fn point(&self, i: usize) -> [f64; 3] {
        [num(&self.x[i]), num(&self.y[i]), num(&self.z[i])]
    }
}

/// The sample nearest a point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Closest {
    pub t: f64,
    pub at: [f64; 3],
    pub dist_sq: f64,
}

/// The sample of `table` nearest `q`; `None` with no samples.
pub(crate) fn closest_point(table: &toml::Table, q: [f64; 3]) -> Option<Closest> {
    let s = Samples::of(table, 1)?;
    let mut best = (f64::MAX, 0usize);
    for i in 0..s.t.len() {
        let p = s.point(i);
        let d = [q[0] - p[0], q[1] - p[1], q[2] - p[2]];
        let dist_sq = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
        if dist_sq < best.0 {
            best = (dist_sq, i);
        }
    }
    Some(Closest {
        t: num(&s.t[best.1]),
        at: s.point(best.1),
        dist_sq: best.0,
    })
}

/// The gap of `table` that `t` (wrapped) falls in, as `(start, end)`; a
/// gap whose start is after its end wraps through 1.
pub(crate) fn gap_at(table: &toml::Table, t: f64) -> Option<(f64, f64)> {
    let count = table
        .get("gap_count")
        .and_then(|v| v.as_integer())
        .unwrap_or(0);
    if count <= 0 {
        return None;
    }
    let starts = numbers(table, "gap_starts")?;
    let ends = numbers(table, "gap_ends")?;
    let t = wrap(t);
    starts
        .iter()
        .zip(ends)
        .take(count as usize)
        .map(|(s, e)| (num(s), num(e)))
        .find(|&(start, end)| {
            if start <= end {
                t >= start && t < end
            } else {
                t >= start || t < end
            }
        })
}

/// A point on the path, with its direction of travel and the level right.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Sample {
    pub at: [f64; 3],
    pub forward: [f64; 3],
    /// `forward × up`, flattened: no `y`.
    pub right: [f64; 3],
}

/// The path at `t` (wrapped), between the two samples whose `t` values
/// bracket it; `None` with fewer than two samples.
pub(crate) fn sample_at(table: &toml::Table, t: f64) -> Option<Sample> {
    let s = Samples::of(table, 2)?;
    let t = wrap(t);
    // The bracketing segment, by binary search over the t values
    let (mut lo, mut hi) = (0usize, s.t.len() - 1);
    while lo + 1 < hi {
        let mid = (lo + hi) / 2;
        if num(&s.t[mid]) <= t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let (t0, t1) = (num(&s.t[lo]), num(&s.t[hi]));
    let frac = if (t1 - t0).abs() > 1e-9 {
        (t - t0) / (t1 - t0)
    } else {
        0.0
    };
    let (a, b) = (s.point(lo), s.point(hi));
    let at = [
        a[0] + (b[0] - a[0]) * frac,
        a[1] + (b[1] - a[1]) * frac,
        a[2] + (b[2] - a[2]) * frac,
    ];
    let mut f = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let flen = (f[0] * f[0] + f[1] * f[1] + f[2] * f[2]).sqrt();
    if flen > 1e-9 {
        f = [f[0] / flen, f[1] / flen, f[2] / flen];
    }
    // Right: cross(forward, up), up = (0, 1, 0)
    let (mut rx, mut rz) = (f[2], -f[0]);
    let rlen = (rx * rx + rz * rz).sqrt();
    if rlen > 1e-9 {
        rx /= rlen;
        rz /= rlen;
    }
    Some(Sample {
        at,
        forward: f,
        right: [rx, 0.0, rz],
    })
}

fn put(map: &mut Map, key: &str, v: f64) {
    map.insert(key.into(), Dynamic::from(v));
}

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("spline");

    m.func(
        "spline_closest_point",
        "The spline's sample nearest a world point.",
    )
    .arg("spline", Entity)
    .arg("x", Num)
    .arg("y", Num)
    .arg("z", Num)
    .returns(
        MapKind,
        "#{t, x, y, z, dist_sq}; () when the entity has no spline_data samples",
    )
    .run(|call, (id, x, y, z)| {
        let world = call.world()?;
        Ok(
            match spline_table(world, id).and_then(|t| closest_point(t, [x, y, z])) {
                Some(c) => {
                    let mut map = Map::new();
                    put(&mut map, "t", c.t);
                    put(&mut map, "x", c.at[0]);
                    put(&mut map, "y", c.at[1]);
                    put(&mut map, "z", c.at[2]);
                    put(&mut map, "dist_sq", c.dist_sq);
                    Dynamic::from_map(map)
                }
                None => Dynamic::UNIT,
            },
        )
    });

    m.func(
        "spline_is_gap",
        "Whether t (wrapped into 0..1) falls in one of the spline's gaps.",
    )
    .arg("spline", Entity)
    .arg("t", Num)
    .returns(Bool, "false too when the entity has no spline_data")
    .run(|call, (id, t)| {
        let world = call.world()?;
        Ok(spline_table(world, id).and_then(|s| gap_at(s, t)).is_some())
    });

    m.func(
        "spline_gap_at",
        "The spline's gap that t (wrapped into 0..1) falls in.",
    )
    .arg("spline", Entity)
    .arg("t", Num)
    .returns(
        MapKind,
        "#{start_t, end_t}; () when t is in no gap or the entity has no spline_data",
    )
    .run(|call, (id, t)| {
        let world = call.world()?;
        Ok(match spline_table(world, id).and_then(|s| gap_at(s, t)) {
            Some((start, end)) => {
                let mut map = Map::new();
                put(&mut map, "start_t", start);
                put(&mut map, "end_t", end);
                Dynamic::from_map(map)
            }
            None => Dynamic::UNIT,
        })
    });

    m.func(
        "spline_sample_at",
        "The spline's point at t (wrapped into 0..1), with its forward and level right directions.",
    )
    .arg("spline", Entity)
    .arg("t", Num)
    .returns(
        MapKind,
        "#{x, y, z, fwd_x, fwd_y, fwd_z, right_x, right_y, right_z}; () with fewer than two samples",
    )
    .run(|call, (id, t)| {
        let world = call.world()?;
        Ok(match spline_table(world, id).and_then(|s| sample_at(s, t)) {
            Some(s) => {
                let mut map = Map::new();
                for (keys, v) in [
                    (["x", "y", "z"], s.at),
                    (["fwd_x", "fwd_y", "fwd_z"], s.forward),
                    (["right_x", "right_y", "right_z"], s.right),
                ] {
                    for (k, n) in keys.iter().zip(v) {
                        put(&mut map, k, n);
                    }
                }
                Dynamic::from_map(map)
            }
            None => Dynamic::UNIT,
        })
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A square track of side 10 in the xz plane, sampled at its corners
    /// and back to the start, with a gap from 0.875 round to 0.125 and one
    /// from 0.25 to 0.375 (values a float holds exactly: `t` is wrapped
    /// with `%`, so 0.4 comes back as 0.39999...).
    fn square() -> toml::Table {
        r#"
        sample_count = 5
        positions_x = [0.0, 10.0, 10.0, 0.0, 0.0]
        positions_y = [0.0, 0.0, 2, 0.0, 0.0]
        positions_z = [0.0, 0.0, 10.0, 10.0, 0.0]
        t_values = [0.0, 0.25, 0.5, 0.75, 1.0]
        gap_count = 2
        gap_starts = [0.875, 0.25]
        gap_ends = [0.125, 0.375]
        "#
        .parse()
        .unwrap()
    }

    #[test]
    fn the_closest_sample_is_found() {
        let c = closest_point(&square(), [9.0, 0.0, 1.0]).unwrap();
        assert_eq!(c.t, 0.25);
        assert_eq!(c.at, [10.0, 0.0, 0.0]);
        assert_eq!(c.dist_sq, 2.0);
        // An int in a float array reads as its number
        let c = closest_point(&square(), [10.0, 2.0, 10.0]).unwrap();
        assert_eq!((c.t, c.dist_sq), (0.5, 0.0));
    }

    #[test]
    fn samples_interpolate_between_the_bracketing_pair() {
        let s = sample_at(&square(), 0.125).unwrap();
        assert_eq!(s.at, [5.0, 0.0, 0.0]);
        assert_eq!(s.forward, [1.0, 0.0, 0.0]);
        assert_eq!(s.right, [0.0, 0.0, -1.0]);
        // t wraps: -0.875 and 1.125 are 0.125
        assert_eq!(sample_at(&square(), -0.875), Some(s));
        assert_eq!(sample_at(&square(), 1.125), Some(s));
        // Halfway up the second side, which climbs
        let s = sample_at(&square(), 0.375).unwrap();
        assert_eq!(s.at, [10.0, 1.0, 5.0]);
        let len = (s.forward.iter().map(|v| v * v).sum::<f64>()).sqrt();
        assert!((len - 1.0).abs() < 1e-12);
        assert_eq!(s.right[1], 0.0, "right is level");
    }

    #[test]
    fn gaps_include_their_start_and_wrap_through_one() {
        let sq = square();
        assert_eq!(gap_at(&sq, 0.3), Some((0.25, 0.375)));
        assert_eq!(gap_at(&sq, 0.25), Some((0.25, 0.375)));
        assert_eq!(gap_at(&sq, 0.375), None, "the end is outside");
        assert_eq!(gap_at(&sq, 0.95), Some((0.875, 0.125)));
        assert_eq!(gap_at(&sq, 0.05), Some((0.875, 0.125)));
        assert_eq!(gap_at(&sq, 1.05), Some((0.875, 0.125)));
        assert_eq!(gap_at(&sq, -0.05), Some((0.875, 0.125)));
        assert_eq!(gap_at(&sq, 0.5), None);
    }

    #[test]
    fn too_little_data_is_none_not_a_panic() {
        let mut one = square();
        one.insert("sample_count".into(), toml::Value::Integer(1));
        assert!(closest_point(&one, [0.0; 3]).is_some());
        assert_eq!(sample_at(&one, 0.5), None, "a segment needs two");
        let mut short = square();
        short.insert("positions_y".into(), toml::Value::Array(vec![]));
        assert_eq!(closest_point(&short, [0.0; 3]), None);
        assert_eq!(sample_at(&short, 0.5), None);
        let mut gapless = square();
        gapless.insert("gap_count".into(), toml::Value::Integer(0));
        assert_eq!(gap_at(&gapless, 0.95), None);
        assert_eq!(gap_at(&toml::Table::new(), 0.95), None);
    }
}
