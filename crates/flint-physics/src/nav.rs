//! Navigation (#969, ADR 0130): where an agent can stand, baked from the
//! scene's fixed colliders, and the way between two points of it.
//!
//! The representation is a grid of columns `cell` metres square, aligned to
//! the world's origin. Each column holds the heights of the floors an agent
//! fits on there, so a bridge over a road, or a room on top of another, is
//! two floors of one column. A floor is the top of a fixed collider (a
//! static body, terrain, a spline's road) facing up no steeper than
//! `max_slope_deg`, with room above it for an upright cylinder of the
//! agent's `radius` and `height`; the cylinder starts `lift` above the floor
//! (the step, or the rise of the steepest slope within the radius), so the
//! floor's own collider never blocks it. Columns are baked a tile of 16 x 16
//! at a time, when a search first reaches them: a scene pays only for the
//! ground its agents ask about.
//!
//! A search is A\* over the floors, eight neighbours a column, a move
//! allowed where the two floors are within `step` (or the slope's rise over
//! the move) of each other and, on a diagonal, where both columns beside it
//! allow it too. The cells' path is then pulled straight: a point is kept
//! only where the straight line past it would leave the floor, or leave its
//! height by more than [`FOLLOW`] (the foot and the top of a ramp), so an
//! agent walking from point to point keeps its feet on the ground.
//!
//! Only fixed bodies are read. A door, a platform or another agent is a
//! kinematic or dynamic body and is not on the grid: a path goes through a
//! closed door, and agents don't avoid each other. That is the next step.

use crate::world::PhysicsWorld;
use crate::PhysicsSystem;
use rapier3d::parry::shape::Cylinder;
use rapier3d::prelude::*;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

/// Columns a side of a tile, the unit a grid bakes in.
pub const TILE: i32 = 16;
const TILE_COLUMNS: usize = (TILE * TILE) as usize;
/// The most floors one column is read for, top down.
const MAX_FLOORS: usize = 16;
/// How far, metres, the floor under a straight leg of a path may be from
/// the leg's own height: a walk along it never floats or sinks more.
pub const FOLLOW: f32 = 0.1;

/// What the agent a grid is for can do. Two agents with the same settings
/// share a grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavSettings {
    /// A column's side, metres. Under twice `radius`, so a thin wall can't
    /// fall between two columns.
    pub cell: f32,
    /// The agent's radius, metres: how far it keeps from a wall.
    pub radius: f32,
    /// The agent's height, metres: the room it needs above a floor.
    pub height: f32,
    /// The highest step it climbs or drops, metres.
    pub step: f32,
    /// The steepest floor it stands on, degrees from flat.
    pub max_slope_deg: f32,
    /// The most columns one search visits before it gives up.
    pub max_nodes: usize,
}

impl Default for NavSettings {
    fn default() -> Self {
        Self {
            cell: 0.25,
            radius: 0.4,
            height: 1.8,
            step: 0.35,
            max_slope_deg: 45.0,
            max_nodes: 40_000,
        }
    }
}

impl NavSettings {
    /// Why these settings can't make a grid, or `Ok`.
    pub fn validate(&self) -> Result<(), String> {
        let positive = |name: &str, v: f32| {
            if v.is_finite() && v > 0.0 {
                Ok(())
            } else {
                Err(format!("`{name}` is a positive number of metres, not {v}"))
            }
        };
        positive("cell", self.cell)?;
        positive("radius", self.radius)?;
        positive("height", self.height)?;
        if !(self.step.is_finite() && self.step >= 0.0) {
            return Err(format!("`step` is metres, 0 or more, not {}", self.step));
        }
        if !(self.max_slope_deg.is_finite() && (0.0..=85.0).contains(&self.max_slope_deg)) {
            return Err(format!(
                "`max_slope` is degrees from 0 to 85, not {}",
                self.max_slope_deg
            ));
        }
        if self.cell >= 2.0 * self.radius {
            return Err(format!(
                "`cell` ({}) must be under the agent's diameter ({}), or a thin wall falls between two columns",
                self.cell,
                2.0 * self.radius
            ));
        }
        if self.max_nodes == 0 {
            return Err("`max_nodes` is 1 or more".into());
        }
        Ok(())
    }

    fn tan_slope(&self) -> f32 {
        self.max_slope_deg.to_radians().tan()
    }

    /// How far above a floor the room test starts: over the step and over
    /// the floor's own rise within the radius, so neither blocks it.
    fn lift(&self) -> f32 {
        (self.step.max(self.radius * self.tan_slope()) + 0.02).min(self.height * 0.5)
    }

    /// The most two floors `run` metres apart may differ to be one walk.
    fn climb(&self, run: f32) -> f32 {
        self.step.max(run * self.tan_slope() + 1e-3)
    }
}

/// One tile's columns: column `i`'s floors are `heights[start[i]..start[i + 1]]`,
/// lowest first.
struct Tile {
    heights: Vec<f32>,
    start: Vec<u16>,
}

impl Tile {
    fn floors(&self, i: usize) -> &[f32] {
        &self.heights[self.start[i] as usize..self.start[i + 1] as usize]
    }
}

/// A floor: its column and which of the column's floors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct Node {
    ix: i32,
    iz: i32,
    level: u8,
}

/// A navigation grid for one kind of agent over one physics world, baked a
/// tile at a time as searches reach it. It notices a change to the fixed
/// colliders ([`PhysicsWorld::static_generation`]) and starts again.
pub struct NavGrid {
    settings: NavSettings,
    generation: u64,
    pipeline: QueryPipeline,
    bounds: Option<Aabb>,
    tiles: HashMap<(i32, i32), Tile>,
}

/// What a grid has baked, for a test or a debug readout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NavStats {
    pub tiles: usize,
    pub floors: usize,
}

impl NavGrid {
    /// A grid for `settings`, which [`NavSettings::validate`] has passed.
    pub fn new(settings: NavSettings) -> Self {
        Self {
            settings,
            generation: 0,
            pipeline: QueryPipeline::new(),
            bounds: None,
            tiles: HashMap::new(),
        }
    }

    pub fn settings(&self) -> &NavSettings {
        &self.settings
    }

    pub fn stats(&self) -> NavStats {
        NavStats {
            tiles: self.tiles.len(),
            floors: self.tiles.values().map(|t| t.heights.len()).sum(),
        }
    }

    /// The way from `from` to `to` over the floors: points on the floor,
    /// the first where `from` stands and the last where `to` does, each
    /// in a straight walk of the next. `None` when either point is on no
    /// floor, no way joins them, or the search ran past `max_nodes`.
    ///
    /// A point stands on the highest floor no more than `step` above it and
    /// no more than `height` below it: an agent's feet, its middle or its
    /// eyes all find the floor under it. A point no floor is under (inside
    /// a wall's margin, say) takes the nearest floor within a metre or so.
    pub fn find_path(
        &mut self,
        physics: &PhysicsSystem,
        from: [f32; 3],
        to: [f32; 3],
    ) -> Option<Vec<[f32; 3]>> {
        flint_core::scope!("nav::find_path");
        let world = &physics.physics_world;
        self.refresh(world);
        let start = self.snap(world, from)?;
        let goal = self.snap(world, to)?;
        let a = self.endpoint(start, from);
        let b = self.endpoint(goal, to);
        if start == goal || self.walkable_line(world, a, b) {
            return Some(vec![a, b]);
        }
        let cells = self.search(world, start, goal)?;
        let mut points: Vec<[f32; 3]> = cells.iter().map(|&n| self.center(n)).collect();
        let last = points.len() - 1;
        points[0] = a;
        points[last] = b;
        Some(self.pull_straight(world, &points))
    }

    /// The floor nearest below a point, as [`find_path`](Self::find_path)
    /// stands one: `[x, floor, z]` of the point itself when a floor is under
    /// it, else of the nearest column's middle.
    pub fn nearest_floor(&mut self, physics: &PhysicsSystem, p: [f32; 3]) -> Option<[f32; 3]> {
        let world = &physics.physics_world;
        self.refresh(world);
        let node = self.snap(world, p)?;
        Some(self.endpoint(node, p))
    }

    // ------------------------------------------------------------------
    // Baking
    // ------------------------------------------------------------------

    /// Start again when the fixed colliders changed since the last bake.
    fn refresh(&mut self, world: &PhysicsWorld) {
        if self.generation == world.static_generation {
            return;
        }
        flint_core::scope!("nav::refresh");
        self.generation = world.static_generation;
        self.tiles.clear();
        self.pipeline.update(&world.collider_set);
        let mut bounds: Option<Aabb> = None;
        for (handle, collider) in world.collider_set.iter() {
            if collider.is_sensor() || !world.is_fixed_collider(handle) {
                continue;
            }
            let aabb = collider.compute_aabb();
            bounds = Some(match bounds {
                Some(b) => b.merged(&aabb),
                None => aabb,
            });
        }
        self.bounds = bounds;
    }

    fn filter() -> QueryFilter<'static> {
        QueryFilter::only_fixed().exclude_sensors()
    }

    fn ensure_tile(&mut self, world: &PhysicsWorld, tx: i32, tz: i32) {
        if self.tiles.contains_key(&(tx, tz)) {
            return;
        }
        flint_core::scope!("nav::bake_tile");
        let mut heights = Vec::new();
        let mut start = Vec::with_capacity(TILE_COLUMNS + 1);
        for lz in 0..TILE {
            for lx in 0..TILE {
                start.push(heights.len() as u16);
                let ix = tx * TILE + lx;
                let iz = tz * TILE + lz;
                let x = (ix as f32 + 0.5) * self.settings.cell;
                let z = (iz as f32 + 0.5) * self.settings.cell;
                self.bake_column(world, x, z, &mut heights);
            }
        }
        start.push(heights.len() as u16);
        self.tiles.insert((tx, tz), Tile { heights, start });
    }

    /// The floors of the column at `(x, z)`, lowest first, onto `out`.
    fn bake_column(&self, world: &PhysicsWorld, x: f32, z: f32, out: &mut Vec<f32>) {
        let Some(b) = self.bounds else { return };
        let margin = self.settings.cell;
        if x < b.mins.x - margin || x > b.maxs.x + margin {
            return;
        }
        if z < b.mins.z - margin || z > b.maxs.z + margin {
            return;
        }
        let s = &self.settings;
        let cos_slope = s.max_slope_deg.to_radians().cos() - 1e-4;
        let bottom = b.mins.y - 0.5;
        let mut y = b.maxs.y + 0.5;
        let first = out.len();
        let filter = Self::filter();
        // Top down: each hit's surface, then on from just under it. A
        // ray that starts inside a solid meets its underside, which faces
        // down and is no floor.
        for _ in 0..MAX_FLOORS * 2 {
            if y <= bottom {
                break;
            }
            let ray = Ray::new(point![x, y, z], vector![0.0, -1.0, 0.0]);
            let Some((_, hit)) = self.pipeline.cast_ray_and_get_normal(
                &world.rigid_body_set,
                &world.collider_set,
                &ray,
                y - bottom,
                false,
                filter,
            ) else {
                break;
            };
            let h = y - hit.time_of_impact;
            if hit.normal.y >= cos_slope && self.has_room(world, x, h, z) {
                out.push(h);
                if out.len() - first >= MAX_FLOORS {
                    break;
                }
            }
            y = h - 0.01;
        }
        out[first..].reverse();
    }

    /// Whether the agent fits standing on a floor at height `h`: its body
    /// (a cylinder of its radius from `lift` up) meets nothing, and nor
    /// does a thin core straight up from just over the floor. The core
    /// finds what the body starts too high to: a floor met from inside a
    /// solid (the ground's top under a kerb), or a beam lower than the
    /// step over the agent's middle.
    fn has_room(&self, world: &PhysicsWorld, x: f32, h: f32, z: f32) -> bool {
        let s = &self.settings;
        let clear = |shape: &Cylinder, bottom: f32| {
            let at = Isometry::translation(x, bottom + shape.half_height, z);
            self.pipeline
                .intersection_with_shape(
                    &world.rigid_body_set,
                    &world.collider_set,
                    &at,
                    shape,
                    Self::filter(),
                )
                .is_none()
        };
        let lift = s.lift();
        let body = Cylinder::new((s.height - lift) * 0.5, s.radius);
        // Started over the floor's own rise within the core's radius
        let core_radius = (s.radius * 0.25).min(0.05);
        let core_bottom = 0.02 + core_radius * s.tan_slope();
        let core = Cylinder::new((s.height - core_bottom) * 0.5, core_radius);
        clear(&body, h + lift) && clear(&core, h + core_bottom)
    }

    fn floors(&mut self, world: &PhysicsWorld, ix: i32, iz: i32) -> &[f32] {
        let (tx, tz) = (ix.div_euclid(TILE), iz.div_euclid(TILE));
        self.ensure_tile(world, tx, tz);
        let i = (iz.rem_euclid(TILE) * TILE + ix.rem_euclid(TILE)) as usize;
        self.tiles[&(tx, tz)].floors(i)
    }

    /// The column's floor nearest `h` within `climb` of it.
    fn floor_near(
        &mut self,
        world: &PhysicsWorld,
        ix: i32,
        iz: i32,
        h: f32,
        climb: f32,
    ) -> Option<(u8, f32)> {
        let mut best: Option<(u8, f32)> = None;
        for (level, &f) in self.floors(world, ix, iz).iter().enumerate() {
            let d = (f - h).abs();
            if d <= climb && best.is_none_or(|(_, b)| d < (b - h).abs()) {
                best = Some((level as u8, f));
            }
        }
        best
    }

    // ------------------------------------------------------------------
    // Points and columns
    // ------------------------------------------------------------------

    fn column_of(&self, x: f32, z: f32) -> (i32, i32) {
        let c = self.settings.cell;
        ((x / c).floor() as i32, (z / c).floor() as i32)
    }

    fn height(&mut self, world: &PhysicsWorld, n: Node) -> f32 {
        self.floors(world, n.ix, n.iz)[n.level as usize]
    }

    fn center(&self, n: Node) -> [f32; 3] {
        let c = self.settings.cell;
        let tile = &self.tiles[&(n.ix.div_euclid(TILE), n.iz.div_euclid(TILE))];
        let i = (n.iz.rem_euclid(TILE) * TILE + n.ix.rem_euclid(TILE)) as usize;
        [
            (n.ix as f32 + 0.5) * c,
            tile.floors(i)[n.level as usize],
            (n.iz as f32 + 0.5) * c,
        ]
    }

    /// Where a path starts or ends for a point snapped to `n`: the point
    /// itself on the floor when `n` is its own column, else `n`'s middle.
    fn endpoint(&self, n: Node, p: [f32; 3]) -> [f32; 3] {
        let center = self.center(n);
        if self.column_of(p[0], p[2]) == (n.ix, n.iz) {
            [p[0], center[1], p[2]]
        } else {
            center
        }
    }

    /// The floor a point stands on: its own column's, else the nearest
    /// column's within a ring a little wider than the agent.
    fn snap(&mut self, world: &PhysicsWorld, p: [f32; 3]) -> Option<Node> {
        let s = self.settings;
        let (cx, cz) = self.column_of(p[0], p[2]);
        let (lo, hi) = (p[1] - s.height - s.step, p[1] + s.step);
        let ring = ((s.radius + s.cell) / s.cell).ceil() as i32 + 2;
        let mut best: Option<(i64, f32, Node)> = None;
        for dz in -ring..=ring {
            for dx in -ring..=ring {
                let d2 = (dx * dx + dz * dz) as i64;
                if best.is_some_and(|(bd, _, _)| d2 > bd) {
                    continue;
                }
                let (ix, iz) = (cx + dx, cz + dz);
                let floors = self.floors(world, ix, iz);
                // The highest floor in the window
                let Some((level, &h)) = floors
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(_, &h)| h >= lo && h <= hi)
                else {
                    continue;
                };
                let node = Node {
                    ix,
                    iz,
                    level: level as u8,
                };
                let better = match best {
                    None => true,
                    Some((bd, bh, bn)) => {
                        d2 < bd || (d2 == bd && (h > bh || (h == bh && node < bn)))
                    }
                };
                if better {
                    best = Some((d2, h, node));
                }
            }
            if best.is_some_and(|(bd, _, _)| bd == 0) {
                break;
            }
        }
        best.map(|(_, _, n)| n)
    }

    // ------------------------------------------------------------------
    // Search
    // ------------------------------------------------------------------

    /// The floors a move from `n` reaches, with each move's length.
    fn neighbours(&mut self, world: &PhysicsWorld, n: Node, out: &mut Vec<(Node, f32)>) {
        out.clear();
        let s = self.settings;
        let h = self.height(world, n);
        let straight = s.climb(s.cell);
        let diagonal = s.climb(s.cell * std::f32::consts::SQRT_2);
        let mut side = [None; 4];
        const SIDES: [(i32, i32); 4] = [(1, 0), (0, 1), (-1, 0), (0, -1)];
        for (k, (dx, dz)) in SIDES.iter().enumerate() {
            side[k] = self.floor_near(world, n.ix + dx, n.iz + dz, h, straight);
            if let Some((level, f)) = side[k] {
                let to = Node {
                    ix: n.ix + dx,
                    iz: n.iz + dz,
                    level,
                };
                out.push((to, (s.cell * s.cell + (f - h) * (f - h)).sqrt()));
            }
        }
        // A diagonal needs both sides it passes between
        for k in 0..4 {
            let (a, b) = (SIDES[k], SIDES[(k + 1) % 4]);
            if side[k].is_none() || side[(k + 1) % 4].is_none() {
                continue;
            }
            let (dx, dz) = (a.0 + b.0, a.1 + b.1);
            if let Some((level, f)) = self.floor_near(world, n.ix + dx, n.iz + dz, h, diagonal) {
                let to = Node {
                    ix: n.ix + dx,
                    iz: n.iz + dz,
                    level,
                };
                let run = s.cell * std::f32::consts::SQRT_2;
                out.push((to, (run * run + (f - h) * (f - h)).sqrt()));
            }
        }
    }

    /// The shortest way over the floors, as A* finds it: octile distance
    /// as the estimate, ties broken by the floor so a search is the same
    /// every run.
    fn search(&mut self, world: &PhysicsWorld, start: Node, goal: Node) -> Option<Vec<Node>> {
        flint_core::scope!("nav::search");
        let cell = self.settings.cell;
        let estimate = |n: Node| {
            let dx = (n.ix - goal.ix).abs() as f32;
            let dz = (n.iz - goal.iz).abs() as f32;
            (dx.max(dz) + (std::f32::consts::SQRT_2 - 1.0) * dx.min(dz)) * cell
        };
        let mut best: HashMap<Node, (f32, Node)> = HashMap::new();
        let mut open = BinaryHeap::new();
        best.insert(start, (0.0, start));
        open.push(Open {
            f: estimate(start),
            g: 0.0,
            node: start,
        });
        let mut visited = 0usize;
        let mut around = Vec::with_capacity(8);
        while let Some(Open { g, node, .. }) = open.pop() {
            if node == goal {
                let mut cells = vec![goal];
                let mut at = goal;
                while at != start {
                    at = best[&at].1;
                    cells.push(at);
                }
                cells.reverse();
                return Some(cells);
            }
            if g > best[&node].0 {
                continue; // a shorter way here was found after this was queued
            }
            visited += 1;
            if visited > self.settings.max_nodes {
                return None;
            }
            self.neighbours(world, node, &mut around);
            for &(next, cost) in &around {
                let ng = g + cost;
                if best.get(&next).is_some_and(|&(bg, _)| bg <= ng) {
                    continue;
                }
                best.insert(next, (ng, node));
                open.push(Open {
                    f: ng + estimate(next),
                    g: ng,
                    node: next,
                });
            }
        }
        None
    }

    // ------------------------------------------------------------------
    // Pulling the path straight
    // ------------------------------------------------------------------

    /// Whether an agent walks the straight line from `a` to `b` on the
    /// floor: every column it crosses has a floor within a step of the
    /// last and within [`FOLLOW`] of the line, and a corner it cuts has a
    /// floor on one side.
    fn walkable_line(&mut self, world: &PhysicsWorld, a: [f32; 3], b: [f32; 3]) -> bool {
        let s = self.settings;
        let (dx, dz) = (b[0] - a[0], b[2] - a[2]);
        let run = (dx * dx + dz * dz).sqrt();
        let samples = ((run / (s.cell * 0.25)).ceil() as usize).max(1);
        let (mut px, mut pz) = self.column_of(a[0], a[2]);
        let mut h = a[1];
        let straight = s.climb(s.cell);
        for i in 1..=samples {
            let t = i as f32 / samples as f32;
            let (ix, iz) = self.column_of(a[0] + dx * t, a[2] + dz * t);
            if (ix, iz) == (px, pz) {
                continue;
            }
            // Both indices changed within a quarter cell: the line passed
            // through one of the two columns beside the corner, so one of
            // them must hold it
            if ix != px
                && iz != pz
                && self.floor_near(world, ix, pz, h, straight).is_none()
                && self.floor_near(world, px, iz, h, straight).is_none()
            {
                return false;
            }
            let crossing = if ix != px && iz != pz {
                s.cell * std::f32::consts::SQRT_2
            } else {
                s.cell
            };
            match self.floor_near(world, ix, iz, h, s.climb(crossing)) {
                Some((_, f)) => h = f,
                None => return false,
            }
            // A walk keeps to the line's height: where the floor bends
            // (the foot and the top of a ramp) the line must bend too, or
            // an agent following it would float or sink
            let (cx, cz) = ((ix as f32 + 0.5) * s.cell, (iz as f32 + 0.5) * s.cell);
            let along = ((cx - a[0]) * dx + (cz - a[2]) * dz) / (run * run).max(1e-12);
            let line = a[1] + (b[1] - a[1]) * along.clamp(0.0, 1.0);
            if (h - line).abs() > FOLLOW {
                return false;
            }
            (px, pz) = (ix, iz);
        }
        (h - b[1]).abs() <= s.step.max(1e-3)
    }

    /// Keep a point of `points` only where the straight line past it
    /// would leave the floor. Two passes: along the cells, each point
    /// walks to the last of the run it sees, which leaves the corners and
    /// a few cells hugging a wall; then over those few, each walks to the
    /// furthest it sees, which drops the huggers.
    fn pull_straight(&mut self, world: &PhysicsWorld, points: &[[f32; 3]]) -> Vec<[f32; 3]> {
        let mut kept = vec![points[0]];
        let mut anchor = 0;
        while anchor < points.len() - 1 {
            let mut reach = anchor + 1;
            while reach + 1 < points.len()
                && self.walkable_line(world, points[anchor], points[reach + 1])
            {
                reach += 1;
            }
            kept.push(points[reach]);
            anchor = reach;
        }
        let mut pulled = vec![kept[0]];
        let mut anchor = 0;
        while anchor < kept.len() - 1 {
            let reach = (anchor + 2..kept.len())
                .rev()
                .find(|&j| self.walkable_line(world, kept[anchor], kept[j]))
                .unwrap_or(anchor + 1);
            pulled.push(kept[reach]);
            anchor = reach;
        }
        pulled
    }
}

/// An open floor in the search, cheapest first.
struct Open {
    f: f32,
    g: f32,
    node: Node,
}

impl PartialEq for Open {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Open {}
impl PartialOrd for Open {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Open {
    fn cmp(&self, other: &Self) -> Ordering {
        // A max-heap: the lowest f first, then the furthest along (highest
        // g), then the floor, so equal costs pop in one order every run
        other
            .f
            .total_cmp(&self.f)
            .then(self.g.total_cmp(&other.g))
            .then(other.node.cmp(&self.node))
    }
}
