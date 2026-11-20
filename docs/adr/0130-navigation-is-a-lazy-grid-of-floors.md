# 0130: Navigation is a grid of floors baked lazily from the fixed colliders

Date: 2026-11-20. Issue: #969 (epic #676).

## Context

Flint had no navigation. An NPC moved in straight lines and around what a
ray told it, so a guard couldn't come round a wall to the player, and a
creature couldn't find the one ramp up to a terrace. The roadmap rated it
High, and two retired plans asked for it (`docs/archive/TODO_REMAINING_PLAN.md`:
"enemy pathing baseline ... suitable for Doom-style arena movement";
`docs/TERRAIN_ROADMAP.md`: nav mesh from terrain and static geometry).

The issue asked first for a decision on the representation (a grid or a
nav mesh, a crate or Flint's own), with what each game in the family
needs, and then for the baseline: a path between two points a script asks
for and follows.

What the games need, from their repositories and the Vault's notes on them:

| Game | What moves | What its NPCs need |
|---|---|---|
| Doom-style FPS | imps, zombiemen chasing the player through rooms | round walls and pillars, up stairs and lifts, through doors; arena-sized (tens of metres) |
| The Overgrown Ruins (`showcase_game/`), the tavern | a bartender, patrons, a stranger | walk a room between tables and to the bar |
| Last Light | (none yet) a walker on an island at dusk | over terrain, round rocks and trees; an island of a few hundred metres |
| FlintLeap | a clockwork platformer's enemies | patrol and chase on platforms at several heights; jumps are the player's |
| FlintKart, Trike | AI racers | none: they follow the track's spline (`spline_closest_point`) |
| Float, Starchild | the whale; a rhythm game | none |

So: several floors over one spot (a platform over ground, a deck over a
road), slopes and steps, ground made of boxes, trimeshes and terrain, scenes
from a room to a few hundred metres, and the web build (wasm32, no threads).
Nobody needs crowds yet, and nothing needs off-mesh links (jumps, ladders).

## Decision

1. **A grid of columns, each holding its floors** (`flint_physics::nav`,
   `NavGrid`). A column is `cell` metres square (0.25 by default), aligned to
   the world origin. Its floors are the up-facing tops of fixed colliders no
   steeper than `max_slope_deg` with room above them for the agent: an
   upright cylinder of its `radius` from `lift` (the step, or the slope's
   rise within the radius) to its `height`, plus a thin core from just over
   the floor (which finds a floor met from inside a solid, and a beam lower
   than the step). Several floors in one column are several levels, so a
   bridge and the road under it are both walkable.
2. **Read from Rapier, not from meshes.** A column is read by casting down
   through the fixed colliders and testing shapes against them, with the
   grid's own `QueryPipeline` built from the collider set. What an agent can
   stand on is what a character collides with: boxes, trimeshes, terrain,
   spline roads, mesh colliders. A visual with no collider isn't there.
   Kinematic and dynamic bodies are left out: a door, a lift and another
   agent are not on the grid.
3. **Baked lazily, a 16 x 16 tile at a time**, when a search first reaches
   it, and thrown away when the fixed colliders change
   (`PhysicsWorld::static_generation`, a process-wide counter bumped when a
   collider on a fixed body comes, goes or turns sensor, so a new scene's
   world never repeats an old one's number). A scene pays for the ground its
   agents ask about, and a scene with no agents pays nothing.
4. **A\* over the floors**, eight neighbours, a move allowed where the two
   floors are within `step` (or the slope's rise over the move) of each
   other, a diagonal only where both sides allow it; octile estimate; ties
   broken by the floor's coordinates, so a path is the same every run.
   `max_nodes` (40 000) bounds a search; past it the answer is no path.
5. **The path is pulled straight** in two passes (each point walks to the
   last of its run of visible cells; then over the points left, to the
   furthest visible). A straight leg must keep to the floor within a step
   at each column, and its height within `FOLLOW` (0.1 m) of the floor, so
   a path bends at the foot and the top of a ramp and an agent walking it
   from point to point never floats or sinks.
6. **Two script functions** in a `nav` module: `find_path(from, to[, opts])`
   returns `[#{x, y, z}, ...]` on the floor (or `[]`), with opts for the
   agent (`radius`, `height`, `step`, `max_slope`, `cell`, `max_nodes`), one
   grid kept for each set of settings asked for (eight at most);
   `move_along_path(entity, path, speed)` moves the entity along it this
   frame, keeping the height above the path it started at, and returns
   where it is, its direction, the point it walks to, the metres left and
   whether it has arrived. It sets the position; it doesn't collide.

## Alternatives considered

- **Recast/Detour through bindings** (`recastnavigation-sys` and the like):
  the standard nav mesh, with crowds. It is C++, so the web build would need
  a C++ toolchain targeting `wasm32-unknown-unknown` beside Rust's, which
  Flint doesn't have; it voxelises triangles, so terrain, splines and
  primitives would all have to be turned into a triangle soup first; and its
  bake is a whole-scene step. Too much for a baseline no game is waiting on.
- **`oxidized_navigation`**: a Rust Recast port, but built on Bevy's ECS and
  its own parry/rapier versions. Flint has neither Bevy nor that Rapier.
- **A nav mesh of Flint's own** (polygons from the same column data): fewer
  nodes and straighter paths over big open ground. It is the step after
  this if a game's ground outgrows a grid; the column bake would be its
  input, so nothing here is thrown away.
- **The `pathfinding` crate's A\***: generic and good, but the search is
  sixty lines here, and tying it to lazily baked tiles and levels is most of
  the work either way. No new dependency.
- **A grid baked whole at scene load**: simpler to reason about, but a
  300 m island at 0.25 m is 1.4 million columns, most never asked about.

## Consequences

- A script can ask for a way and walk it: `flint-examples/nav_lamplighter.scene.toml`
  (a lamplighter round two hedges and up a ramp to four lanterns) passes
  `flint playtest` with `test_pass` after the fourth.
- A path goes through a closed door and agents don't avoid each other:
  dynamic obstacles and avoidance are the next step (#969's "after").
- No jumps, ladders or teleports between floors: a gap the agent can't step
  is no way. FlintLeap's enemies would patrol within a platform.
- The first search over new ground bakes it, in the frame that asks: a
  script can ask once in `on_init` to bake at load instead.
- `NavGrid` is in `flint-physics` and reads its private sets; a second
  user (an editor overlay drawing the floors) would go through the same
  crate.

## Measured

Release, this machine (`cargo test -p flint-physics --release --lib nav_cost
-- --ignored --nocapture`), default settings (0.25 m cells, 0.4 m radius):

| Search | Cold (bakes) | Warm | Points | Length | Tiles |
|---|---|---|---|---|---|
| Garden leg 1 (straight) | 3.5 ms | <0.01 ms | 2 | 9.0 m | 4 |
| Garden leg 3 (round both hedges) | 10.8 ms | 0.65 ms | 4 | 24.4 m | 18 |
| Garden leg 4 (up the ramp) | 8.7 ms | 0.37 ms | 4 | 19.0 m | 26 |
| 200 m field, open 120 m | 16.6 ms | | 2 | 120 m | 36 |
| 200 m field, round a 150 m wall | 115 ms, then no path | 23 ms | | | 221 |
| the same, 0.5 m cells, `max_nodes` 400 000 | 153 ms | 30 ms | 4 | 193 m | 259 |

A tile costs 0.4 to 0.9 ms to bake (16.6 ms for the open field's 36; 8.2 ms
more for leg 3's 11 than its warm search), most of it Rapier's shape tests. A warm
search in a room-sized scene is well under a millisecond. A detour of a few
hundred metres round one long wall is past the default budget: a coarser
`cell` and a larger `max_nodes` find it, at 30 ms a search, which a game
would make once and follow, not every frame. In the debug build the
garden's long leg costs about 0.93 s cold (`flint play`'s hitch report: the
debug build doesn't optimise Rapier), so a debug run hitches once where a
release run doesn't.
