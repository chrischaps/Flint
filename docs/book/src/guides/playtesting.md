# Playtesting

`flint playtest` answers the question an agent asks after every change: *does the game still play?* It plays a scene the way `flint play` does, with physics, the character controller, scripts, animation, particles and transitions, but with no window, no GPU and no sound device, on a fixed clock. Then it says what happened as one JSON report and exits 1 when anything failed.

```bash
flint playtest flint-examples/phase4_runtime.scene.toml \
    --input-script flint-examples/playtests/phase4_runtime.playtest.txt --exit-after 8
```

```text
PASS flint-examples/phase4_runtime.scene.toml: ended by exit_after at 8.000 s, 481 frames
  checks 10 of 10 held; assertions 1 places, 0 failed; script errors 0; warnings 0 at load, 0 while running
```

A screenshot shows what a frame looked like. A playtest shows what the game *did*: whether the player reached the bar, whether the bartender answered, whether the door opened. It says so in words a test can read, and the same run gives the same bytes every time.

## A playtest is an input script that asks questions

The input script `flint-player --input-script` plays (keys, pads, the mouse, touch; [the format](../cli-reference/overview.md#player-screenshots-hud-included)) takes two more verbs, `expect` and `wait`. Here is the tavern's, checked in beside the scene and run by `cargo test`:

```text
# Before anything moves
0.0  expect tavern_kitchen_door door.locked == true
0.0  expect bartender interactable.enabled == true

# To the bar: A strafes toward +X
0.2  down   KeyA
0.2  wait   player transform.position.x > 3.6 within 3
0.2  up     KeyA
0.3  expect bartender animator.speed > 1.3        # the bartender is alert
0.4  tap    KeyE                                  # "Welcome to the Rusty Flint!"
0.6  expect log bartender_greeted == true

# Up the hall to the front door
0.9  down   KeyW
0.9  wait   player transform.position.z > 8.6 within 6
0.9  up     KeyW
1.1  tap    KeyE
1.3  wait   event door_opened within 1
1.4  expect 1 entities where door.locked == true  # the kitchen's, still
```

An **`expect`** is asked once, on the first frame at or after its time. A **`wait`** is asked every frame from its time until it holds, and holds back every line after it. Those lines then play as much later as it waited, so `0.2 up KeyA` lets go of the key the frame the player reaches the bar, however long the walk took. A `wait` that doesn't hold `within` its seconds (10 by default) fails, and the lines after it go on from there. A script written with waits doesn't have to know how fast the character walks.

What a line asks is a [flint-query](../concepts/queries.md) condition, the same `field op value` that `flint query "entities where ..."` takes:

| Line | Holds when |
|------|------------|
| `expect <entity> <condition>` | the named entity meets the condition: `player transform.position.z > 1` |
| `expect entities where <condition>` | at least one entity does |
| `expect no entities where <condition>` | none does |
| `expect <n> entities where <condition>` | exactly *n* do |
| `expect persist <key> <op> <value>` | the `persist_*` store holds it: `persist coins >= 3` |
| `expect log <key> <op> <value>` | a script's `test_log(key, value)` last said it |
| `expect event <name>` | a `fire_event` (or the engine's own event) of that name has fired. For a `wait`, since the check before it settled, so `tap KeyE` then `wait event door_opened` sees the door the tap opened |

A field may name one axis of a vector: `transform.position.y`, `velocity.x` (`x`, `y`, `z`, `w`). The operators are `==`, `!=`, `<`, `<=`, `>`, `>=` and `contains`, and a value is a number, `true`/`false` or a quoted string.

The same lines work in `flint play --input-script`: there a failed check is printed with the exit report and the run exits 1.

## Scripts have a say too

Four functions let a game's own scripts take part (the [test module](../reference/script-api.md#test)):

```rhai
fn on_interact() {
    // played, these say nothing unless the assertion fails
    test_assert(distance(self_entity(), get_entity("player")) <= 3.5,
                "the bartender only talks within reach");
    test_log("bartender_greeted", true);
}

fn on_update() {
    if get_field(get_entity("boss"), "health", "value") <= 0 {
        test_pass();          // end the playtest here, passing
    }
}
```

- `test_assert(cond, message)` fails the run when `cond` is false. Each place (file, line, message) is one entry in the report, however many frames it ran in: how often it held, how often it failed, and when it first failed.
- `test_fail(message)` ends the run failing; `test_pass()` ends it passing (unless something already failed). The first of the two stands.
- `test_log(key, value)` puts a value in the report's `log`, where an `expect log` line can read it.

In `flint play` they cost almost nothing: `test_assert` logs its failure once per place, `test_fail` logs once, and neither ends the game.

## How a run ends and what fails it

A run ends at the first of these:

| `ended` | When |
|---------|------|
| `test_pass` / `test_fail` | a script called one |
| `quit_game` | a script called `quit_game()` |
| `scene_failed` | a scene transition failed |
| `exit_after` | game time reached `--exit-after` (10 s when neither it nor `--timeout` is given) |
| `timeout` | game time reached `--timeout`, a failure: the run should have ended itself by then |
| `start_failed` | the scene didn't load |

It **fails** on a failed `test_assert`, a `test_fail`, an `expect` or `wait` that didn't hold, **any** script error (load, compile or run time, each with its file and line), a failed transition, a timeout, and with `--strict`, any warning while the scene loaded. `--strict` also runs [`flint check`](../cli-reference/overview.md#the-check-command) on the scene first and fails on what it finds, which catches what a run with no renderer can't see, such as a model that isn't there.

## The report

`--json` puts it on stdout, `--report out.json` in a file (both at once is fine). Every list is in a fixed order and every time is game time, so two runs of the same scene, script and flags give byte-identical reports. Add `--timings` for the wall clock, which then makes each run differ.

```json
{
  "passed": false,
  "ended": "exit_after",
  "failures": [
    { "kind": "script_error", "file": "scripts/game.rhai", "line": 4,
      "message": "director in on_update: Array index 5 out of bounds: only 2 elements" }
  ],
  "checks": [
    { "line": 2, "verb": "wait", "text": "player transform.position.x > 3.6",
      "due": 0.2, "at": 0.967, "passed": true, "found": "transform.position.x = 3.6657" }
  ],
  "assertions": [], "verdict": null, "script_errors": [ ... ],
  "warnings": { "counted": true, "at_load": 0, "while_running": 0, "messages": [] },
  "log": {}, "persist": {}, "entities": {},
  "perf": { "frames": 61, "fixed_steps": 60, "frame_step": 0.0167, "game_time": 1.0 }
}
```

| Key | What |
|-----|------|
| `failures` | every reason it failed: `kind` (`assertion`, `test_fail`, `check`, `script_error`, `scene`, `timeout`, `start`, `load_warning`, `preflight`), the message, and the file and line where there is one |
| `checks` | each `expect` and `wait`: its line, when it was due and settled, whether it held, and what was found (`2 matches: door1, door2`, `no entity named \`dor\``) |
| `assertions`, `verdict`, `log` | what scripts said |
| `script_errors` | as `flint render --report json` lists them: file, line, column, entity, callback, message, count |
| `warnings` | the engine's warnings by message, counted while loading and while running |
| `persist` | the `persist_*` store at the end |
| `entities` | with `--dump <name>` (repeatable), that entity's components at the end |
| `perf` | frames, fixed physics steps and game time |

## What a playtest leaves out

A playtest is the player without the presentation. There's no renderer, so models aren't loaded and nothing a script reads off a drawn frame (`raycast_meshes`, `world_to_screen`'s last view) means anything. There's no sound device: sounds still load, so a missing file still warns, but none plays. Saves stay in memory: a playtest starts with no save file and writes none. Hot reload is off. `random()` is seeded as in any stepped run (`--seed` picks another), and scripts see a screen of `--width` x `--height` points. A script that reads `timestamp()` still sees the wall clock, and its run won't repeat.

## Flags

| Flag | |
|------|--|
| `--input-script <file>` | Keys, pads, the pointer, and `expect` / `wait` lines |
| `--exit-after <s>` | End here, passing if nothing failed |
| `--timeout <s>` | End here as a failure |
| `--frame-step <s>` | Game seconds a frame (default 1/60) |
| `--seed <n>` | `random()`'s seed |
| `--strict` | A load warning fails it; so does what `flint check` finds |
| `--dump <entity>` | That entity's components at the end (repeatable) |
| `--report <file>` | Write the JSON report there too |
| `--timings` | Wall-clock times in the report |
| `--width`, `--height` | The screen scripts see, in points (1280 x 720) |
| `--schemas <dir>` | As for every command |
