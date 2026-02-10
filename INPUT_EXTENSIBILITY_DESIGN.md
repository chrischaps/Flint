# Input Extensibility Design (Flint)

## 1. Current Input Handling (Exploration Summary)

Input is currently centralized and functional, but mostly hardcoded:

- Hardware events enter in `PlayerApp`:
  - Keyboard in `crates/flint-player/src/player_app.rs:485`
  - Mouse buttons in `crates/flint-player/src/player_app.rs:541`
  - Raw mouse motion in `crates/flint-player/src/player_app.rs:571`
- Those events feed `InputState` (`crates/flint-runtime/src/input.rs:7`).
- Default action bindings are hardcoded in code (`crates/flint-runtime/src/input.rs:56` and `crates/flint-runtime/src/input.rs:71`).
- Gameplay reads logical actions (`is_action_pressed`) in character movement (`crates/flint-physics/src/character.rs:128` to `crates/flint-physics/src/character.rs:173`).
- `PlayerApp` emits only `ActionPressed` events from `actions_just_pressed()` (`crates/flint-player/src/player_app.rs:279`), even though `ActionReleased` exists (`crates/flint-runtime/src/event.rs:31`).
- Script input snapshot uses a hardcoded action-name list (`crates/flint-script/src/lib.rs:157`), which blocks arbitrary custom actions from `is_action_pressed("...")`.
- Scenes currently have no input config metadata field (`crates/flint-scene/src/format.rs:16`).
- UI scripts hardcode prompt keys like `[E]` (for example `demo/scripts/hud_interact.rhai` and `games/doom_fps/scripts/hud.rhai`).

## 2. Gaps vs Goal

To support per-game config + remapping + multi-device input cleanly, current gaps are:

- No file-backed action map.
- Keyboard and mouse are split into separate maps instead of one binding model.
- No gamepad ingestion path.
- No user-remap persistence.
- Script query path is partly hardcoded to known actions.
- No action release event flow.

## 3. Proposed Target Architecture

### Core model

Add a config-backed, device-agnostic binding layer in `flint-runtime`:

```rust
struct InputConfig {
    version: u32,
    game_id: String,
    actions: BTreeMap<String, ActionConfig>,
}

struct ActionConfig {
    kind: ActionKind, // Button | Axis1D
    bindings: Vec<Binding>,
}

enum Binding {
    Key { code: String },
    MouseButton { button: String },
    MouseDelta { axis: String, scale: f32 },
    MouseWheel { axis: String, scale: f32 },
    GamepadButton { button: String, gamepad: GamepadSelector },
    GamepadAxis {
        axis: String,
        gamepad: GamepadSelector,
        deadzone: f32,
        scale: f32,
        invert: bool,
        threshold: Option<f32>,
    },
}
```

### Runtime API (`InputState`)

Keep existing APIs for compatibility, add:

- `load_bindings(config)`
- `rebind_action(action, binding, mode)`
- `clear_action_bindings(action)`
- `actions_pressed()` and `actions_just_released()`
- `action_value(action) -> f32` for analog support

## 4. Config File Layout

### Per-game default config (versioned in repo)

Recommended path: `<game_root>/config/input.toml`.

Example:

```toml
version = 1
game_id = "doom_fps"

[actions.move_forward]
kind = "button"
[[actions.move_forward.bindings]]
type = "key"
code = "KeyW"
[[actions.move_forward.bindings]]
type = "gamepad_axis"
axis = "LeftStickY"
direction = "negative"
threshold = 0.35
gamepad = "any"

[actions.fire]
kind = "button"
[[actions.fire.bindings]]
type = "mouse_button"
button = "Left"
[[actions.fire.bindings]]
type = "gamepad_button"
button = "RightTrigger"
gamepad = "any"

[actions.look_x]
kind = "axis1d"
[[actions.look_x.bindings]]
type = "mouse_delta"
axis = "x"
scale = 1.0
[[actions.look_x.bindings]]
type = "gamepad_axis"
axis = "RightStickX"
deadzone = 0.15
scale = 2.0
invert = false
gamepad = "any"
```

### Per-user remap override (written at runtime)

Recommended path: `<game_root>/.flint/input.user.toml` (or global fallback under user home if game root is read-only).

Merge rule: action-level override replaces that action's binding list.

## 5. Load/Merge Order

Use deterministic precedence:

1. Engine built-in defaults (today's hardcoded map).
2. Game defaults file (`config/input.toml`).
3. User override file (`.flint/input.user.toml`).
4. Optional CLI override path (`--input-config`).

This matches existing Flint layering style already used elsewhere (see `crates/flint-asset-gen/src/config.rs`).

## 6. Remapping Flow

Runtime remap flow:

1. UI asks to rebind action.
2. Engine enters "capture next control" mode.
3. First valid hardware event becomes new binding.
4. Conflict policy applied (`replace`, `add`, or `swap`).
5. Binding applied immediately.
6. User override file saved.

Important guardrails:

- Keep `Escape`, `F1`, `F4`, `F11` engine-reserved initially.
- Expose binding-label API to scripts/UI, so prompts stop hardcoding `[E]`.

## 7. Multi-Device (Keyboard/Mouse + Gamepad)

`winit` does not provide full gamepad support directly, so integrate a gamepad crate in `flint-player` (pragmatic choice: `gilrs`).

Gamepad integration plan:

- Drain gamepad events each frame in `PlayerApp`.
- Convert to same internal binding events as keyboard/mouse.
- Feed `InputState` through one unified path.
- Respect deadzones and thresholds for stick/trigger mappings.

## 8. Migration Plan (Low Risk)

1. Phase 1: Config-backed keyboard/mouse bindings, no behavior change by default.
2. Phase 2: Runtime remap + user override persistence.
3. Phase 3: Gamepad bindings + axis values.
4. Phase 4: Emit and consume `ActionReleased`; remove hardcoded script action list in `snapshot_actions()`.

## 9. Code Touch Points

- `crates/flint-runtime/src/input.rs`: replace hardcoded maps with config-driven bindings.
- `crates/flint-player/src/player_app.rs`: load configs near startup and route additional hardware events.
- `crates/flint-script/src/lib.rs`: remove hardcoded action list and snapshot dynamically.
- `crates/flint-script/src/api.rs`: add optional helpers for binding labels and axis value access.
- `crates/flint-cli/src/commands/play.rs`: optional `--input-config` flag.
- `docs/book/src/concepts/physics-and-runtime.md`: update controls docs to reference config files.

## 10. Test Coverage to Add

- Config parse/validate tests (bad key names, unknown buttons, invalid thresholds).
- Merge precedence tests (engine < game < user < CLI).
- Rebind persistence tests.
- Multi-binding same action tests (keyboard + mouse + gamepad).
- Script compatibility tests (`is_action_pressed` works for custom actions).
- Gamepad deadzone/threshold behavior tests.
