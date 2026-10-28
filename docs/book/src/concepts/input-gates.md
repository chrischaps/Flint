# Cursor Capture and Input Gates

Mouse-look and pad play need the player to be strict about who hears what. A hidden cursor mustn't press a button nobody can see. A pad shouldn't walk the character while someone types a name into a field, or while the window is in the background. ADR 0110 sets these rules, and the player enforces them for every game. Nothing needs turning on.

## A captured cursor

`set_cursor_captured(true)` from a script, or the first click in a scene with a character controller, captures the cursor. The cursor is hidden and held in the window. Some platforms (Windows among them) only confine it, so it keeps moving under the hood. The player makes that motion count for nothing:

- **It reads as the window centre.** `mouse_x()` / `mouse_y()` and `InputState::mouse_position` stay at the centre while the cursor is captured. They follow a resize too. Picking and hit-testing against them aim where a reticle would be.
- **Its moves reach no one.** Not egui, not extensions' `on_window_event`, not `InputState`. Mouse-look reads the raw motion: `mouse_delta_x()` / `mouse_delta_y()`, or a `mouse_delta` binding.
- **Its buttons and wheel skip egui.** A click while captured goes to extensions and the game: `is_mouse_pressed`, `is_mouse_just_pressed` and button bindings report it as usual. An egui button under the hidden cursor is never pressed. The one exception is a release whose press egui took before the capture. That release goes to egui, so nothing stays pressed there. When the capture begins, egui is told the pointer is gone, so it doesn't keep hovering whatever it was over.
- **Release puts the cursor at the centre.** That's where it read while captured, so nothing jumps under it.

Keys and touch don't change with the capture.

`is_cursor_captured()` says whether the cursor is captured now. Extensions read the same from `ExtensionCtx::is_cursor_captured()`. A `set_cursor_captured` request takes effect at the end of the frame it's made in, so the getter shows it from the next frame. It also shows a release the script didn't ask for: Escape, or F3/F4 opening a debug panel.

```rhai
fn on_update() {
    if is_cursor_captured() {
        turn_view(mouse_delta_x(), mouse_delta_y());
    }
}
```

## The text-focus gate

While an egui text field has keyboard focus, keys stop at egui (ADR 0073). Pads and raw mouse motion stop there too:

- **Pads are held.** Sticks and triggers read 0 and buttons read up, so a pad-bound action releases the frame focus arrives (`InputState::hold_gamepads`). The pads are still tracked underneath, so a stick held through it reads again the frame the field lets go. A button doesn't. One held when the field took focus, or pressed while it had it, reads as up until it's released. It never fires a press nobody made.
- **Raw mouse motion is dropped**, so typing with the mouse-look on doesn't turn the view.

## The window's focus

While the window is unfocused, real pads are ignored. Losing focus releases them, so a character walking on a stick stops. Getting focus back reads the sticks where they stand, but a held button needs a fresh press. Raw mouse motion is dropped while the window is unfocused, too.

The input script's pad isn't a real one. Captures often run with the window in the background, so its `axis`, `press` and `release` play on whatever the focus.

## During a capture

A run that shoots (`--screenshot`), stops on its own (`--exit-after`) or plays an input script (`--input-script`) is a capture, and it hears its script alone. Real keys, the mouse, the wheel, touch and real pads are dropped before egui or the game sees them, and so is raw mouse motion. A scroll or a key on a capture window that pops up under someone's cursor can't move its shot, so two runs of one capture compare. Lifecycle events still arrive: the window resizes, gains and loses focus, and closes from its close button. The script's own events take the path real ones would, with every gate above (`CaptureConfig::is_capture`).

## In input scripts

`look dx dy [secs]` feeds raw mouse motion, as a real mouse's device motion arrives: at once, or spread over `secs` of game time. It takes the same gates as a real mouse: it reaches the game only while the cursor is captured and no text field has focus, but it doesn't wait on the window's focus.

```text
0.0   look   40 -10          # turn right and up a little, this frame
0.5   look   300 0 1.5       # a slow pan right over 1.5 s
```

See [the CLI reference](../cli-reference/overview.md) for the rest of the input script.
