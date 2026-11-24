# ADR 0134: The character controller stays in physics, with one player and fixed action names

Status: accepted, 2026-11-23 (#1212). Follows #728, whose audit named
these four questions and left them open.

## Context

The first-person character controller is `flint_physics::character`. Since
#728 its numbers are `character_controller` fields, its look and jump press
are read once a frame, and its walk runs once a fixed step. The #728 audit
left four things alone:

- **One player, chosen by name, silently.** `PhysicsSystem::initialize`
  walks the world's names in order (`FlintWorld::all_entities`, a
  `BTreeMap`) and takes the first entity with a `character_controller` as
  the player. `is_near_player` in flint-script picks the same way. A
  second controller was ignored and nothing said so.
- **Two fields nothing read.** `radius` (the capsule is the entity's
  `collider`) and `camera_mode` (`first_person` / `orbit`; no orbit camera
  exists for a character). The schema said "not read yet", and the `player`
  archetype and seven of the repository's scenes still set both.
- **Fixed action names.** `move_forward`, `move_backward`, `move_left`,
  `move_right`, `sprint` and `jump` are spelled in `character.rs`, and again
  in the player's controls line (`console.rs`, `CHARACTER_ACTIONS`).
- **Controller as a script.** #728's larger alternative was to make the
  controller a default script or extension, leaving physics only
  `move_character_shape`.

## Decision

1. **One player, the first by name, and a second controller is a warning.**
   The rule doesn't change. `flint_scene::diagnose` raises `second-player`
   on every entity after the first by name that carries a
   `character_controller`, written or from its archetype. The warning names
   the player. Because the loaders run the same pass, the load summary,
   `flint validate` and `flint check` all say it. It is not raised for a
   chunk: a chunk's entities are never the player, and the chunk pass has
   no whole scene to count.
2. **`radius` and `camera_mode` are removed, and the schema says why.** A
   component's schema can now hold `[component.<name>.retired]`, a field
   name and the reason it went. A scene that still writes a retired field
   gets the usual `unknown-field` warning, but the message ends "any more"
   and the hint is that reason, not the nearest field's name. Games and the
   examples repository are told what to do without anyone keeping a list
   by hand. The repository's own scenes, the `walk` template and the
   `player` archetype no longer write the two fields.
3. **The action names stay fixed.** They are the controller's interface, as
   a function's name is. A game already controls everything a player
   presses: its input config binds any key, pad button, axis or touch
   control to these six actions, layered under the user's overrides. A
   per-game rename would split one intent across names in the controls
   line, the touch layouts and every game's input config, and nothing has
   asked for one. If a game ever needs one, a `character_controller.actions`
   table (intent to action name) read in `character::Settings` and in
   `console::CHARACTER_ACTIONS` is the shape: two readers, one table.
4. **The controller stays in Rust, in physics, as the built-in first-person
   controller.** A game that wants another controller already has what it
   needs: leave `character_controller` off, move a collider with
   `move_character(id, dx, dy, dz)` from a script (or a `PlayerExtension`
   in Rust), and place the camera with `set_camera_position` /
   `set_camera_target`. Moving the built-in one into a script is refused
   for now, for three reasons:
   - **The fixed step.** Scripts have no fixed-step callback. Only
     `on_update` exists, and it runs once a frame at a variable `dt`. A
     script walk would bring back the frame-rate dependence #728 removed,
     unless scripts first gained one.
   - **"The player" is read by the host.** The first-person camera, the
     audio listener, cursor capture, the controls line, the debug keys'
     mouse note and `is_near_player` all ask
     `PhysicsSystem::has_player_entity`. A script controller would need a
     way to say "this entity is the player" to each of them.
   - **Cost.** The walk is a Rapier shape sweep. In Rust it runs every step
     for nothing, but in Rhai it would pay the call overhead on every step.

## Alternatives considered

- **Refuse a scene with two controllers.** Too strong: the scene plays
  correctly with one player, and a prefab or a work-in-progress level can
  carry a second one for a while. A warning that names the player says
  what will happen.
- **A `player = true` field to choose the player.** Adds a second rule to
  learn and a new failure (two marked, none marked) to fix the first. A
  name is already a stable, visible choice (`hero` before `zed`). Revisit
  if a scene really needs a player whose name sorts after another
  character's.
- **Read `radius` to size the capsule.** That gives two sources of truth
  for one shape, the collider's `size` and the controller's `radius`, and
  one of them has to lose. The collider is what Rapier sweeps.
- **Implement `camera_mode = "orbit"`.** A third-person camera is a real
  feature, not the reading of a field. When one is wanted, the controller
  becomes a choice of kinds (first person one of them), and a field naming
  the kind comes back with a reader.
- **Leave the two fields, still marked "not read yet".** Every scene that
  set them believed they did something.

## Consequences

- A scene, prefab or game that writes `radius` or `camera_mode` in a
  `character_controller` gets one `unknown-field` warning per field at load
  and in `flint check`. Its hint says to remove the field and why. The
  scene plays as before, since nothing read them. The changelog fragment
  says so.
- `[component.<name>.retired]` is open to any component or scene table,
  in a game's layer as well as the engine's. An extension's retired fields
  add to the declaring layer's. `COMPONENT_KEYS` includes `retired`.
- `codes::ALL` has 56 codes with `second-player`.
- The six names are constants in `flint_physics::character`
  (`MOVE_FORWARD` ... `SPRINT`, and `ACTIONS`), and the player's
  `console::CHARACTER_ACTIONS` is that list, so the controls line can't
  drift from what the controller reads.
