# Physics and Runtime

Flint's runtime layer transforms static scenes into interactive, playable experiences. The `flint-runtime` crate provides the game loop infrastructure, and `flint-physics` integrates the Rapier 3D physics engine for collision detection and character movement.

## The Game Loop

The game loop uses a **fixed-timestep accumulator** pattern. Physics simulation steps at a constant rate (1/60s by default) regardless of how fast or slow the rendering runs. This ensures deterministic behavior across different hardware.

The loop structure:

1. **Tick the clock** --- advance time, accumulate delta into the physics budget
2. **Process input** --- read keyboard and mouse state into `InputState`
3. **Fixed-step physics** --- while enough time has accumulated, step the physics simulation
4. **Character controller** --- apply player movement based on input and physics state
5. **Update audio** --- sync listener position to camera, process trigger events, update spatial tracks
6. **Advance animation** --- tick property tweens and skeletal playback, write updated transforms to ECS, upload bone matrices to GPU
7. **Render** --- draw the frame with the current entity positions

The `RuntimeSystem` trait provides a standard interface for systems that plug into this loop. Physics, audio, and animation each implement `RuntimeSystem` with `initialize()`, `fixed_update()`, `update()`, and `shutdown()` methods.

## Physics with Rapier 3D

The `flint-physics` crate wraps Rapier 3D and bridges it to Flint's TOML-based component system:

- **PhysicsWorld** --- manages Rapier's rigid body set, collider set, and simulation pipeline
- **PhysicsSync** --- reads `rigidbody` and `collider` components from entities and creates corresponding Rapier bodies. Static bodies for world geometry (walls, floors, furniture), kinematic bodies for the player.
- **CharacterController** --- kinematic first-person movement with gravity, jumping, ground detection, and sprint

## Physics Schemas

Three component schemas define physics properties:

**Rigidbody** (`rigidbody.toml`) --- determines how an entity participates in physics:
- `body_type`: `"static"` (immovable world geometry), `"dynamic"` (simulated), or `"kinematic"` (script-controlled)
- `mass`, `gravity_scale`

**Collider** (`collider.toml`) --- defines the collision shape:
- `shape`: `"box"`, `"sphere"`, or `"capsule"`
- `size`: dimensions of the collision volume
- `friction`: surface friction coefficient

**Character Controller** (`character_controller.toml`) --- first-person movement parameters:
- `move_speed`, `jump_force`, `height`, `radius`, `camera_mode`

The **player** archetype (`player.toml`) bundles these together with a `transform` for a ready-to-use player entity.

## Adding Physics to a Scene

To make a scene playable, add physics components to entities:

```toml
# The player entity
[entities.player]
archetype = "player"

[entities.player.transform]
position = [0, 1, 0]

[entities.player.character_controller]
move_speed = 6.0
jump_force = 7.0

# A wall with a static collider
[entities.north_wall]
archetype = "wall"

[entities.north_wall.transform]
position = [0, 2, -10]

[entities.north_wall.collider]
shape = "box"
size = [20, 4, 0.5]

[entities.north_wall.rigidbody]
body_type = "static"
```

Then play the scene:

```bash
flint play my_scene.scene.toml
```

## Input System

The `InputState` struct tracks keyboard and mouse state each frame:

- Keyboard keys are tracked as pressed/released
- Mouse provides raw delta movement (via `DeviceEvent::MouseMotion`) for smooth camera look
- Action bindings map keys to game actions (move forward, jump, sprint, etc.)

## Further Reading

- [Audio](audio.md) --- spatial audio with Kira
- [Animation](animation.md) --- property tweens and skeletal animation
- [Rendering](rendering.md) --- the PBR rendering pipeline
- [Schemas](schemas.md) --- component and archetype definitions including physics schemas
- [CLI Reference](../cli-reference/overview.md) --- the `play` command and player binary
