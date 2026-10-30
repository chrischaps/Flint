//! Timed input script (`--input-script <file>`): key presses, gamepad
//! buttons, stick values, the mouse wheel and the pointer, played into the
//! player at set game times.
//!
//! With `--frame-step` the run is deterministic, so a scripted lap or trick
//! line plays the same way every time: the capture path for showcase video
//! and the regression path for script-driven gameplay.
//!
//! One event per line, `#` starts a comment, times are game seconds:
//!
//! ```text
//! 0.0   down   KeyW               # hold a key (names as in input configs)
//! 2.5   up     KeyW
//! 3.0   tap    KeyF 0.4           # down now, up 0.4 s later (default 0.06)
//! 3.0   axis   LeftStickX -0.35   # stick value on the script pad
//! 4.0   press  South              # gamepad button down
//! 4.2   release South
//! 4.5   pull   RightTrigger2 0.6  # a trigger pulled part-way (0 lets go)
//! 5.0   wheel  1                  # wheel lines this frame: up is +, in
//! 5.5   wheel  -0.5 2             # a line is a notch; then an x amount
//! 6.0   mouse  640 360            # pointer to window pixels (as screenshots)
//! 6.1   down   MouseLeft          # mouse buttons: MouseLeft, MouseRight,
//! 6.2   mouse  900 500 0.5        #   MouseMiddle, MouseBack, MouseForward;
//! 6.8   up     MouseLeft          #   a glide to (900, 500) over 0.5 s drags
//! 7.0   click  320 200            # pointer there, then tap MouseLeft
//! 7.5   click  320 200 MouseRight
//! 8.0   type   200 planks         # text into the focused egui field
//! 9.0   look   40 -10             # raw mouse motion, as mouse-look reads
//! 9.5   look   300 0 1.5          #   it; spread over 1.5 s
//! 10.0  touch  1 200 600          # finger 1 down at window pixels
//! 10.1  drag   1 200 480 0.4      # finger 1 slides there over 0.4 s
//! 10.2  touch  2 1150 620         # a second finger, at the same time
//! 11.0  lift   2                  # finger 2 up
//! ```
//!
//! Fingers (`touch`, `drag`, `lift`) arrive as winit's `WindowEvent::Touch`
//! and take a real finger's path: the on-screen touch controls first
//! (#133), then egui, then `InputState`. Each finger glides on its own, so
//! a thumb on a stick and another on a button can move at once.
//!
//! `pull` is an analog button's travel, as a real pad's trigger sends it
//! (gilrs' `ButtonChanged`): a binding to that button reads the value, not
//! the digital 0 or 1. It sets the value alone, so a script that also wants
//! the button's press edge adds a `press` and `release` of its own.
//!
//! The pad writes `InputState` directly. Keys, the pointer (`mouse`,
//! `click`, mouse buttons and the wheel) and typed text come back from
//! [`InputScript::apply`] as [`ScriptEvent`]s, which the player routes the
//! way it routes a real keyboard and mouse: egui first, then `InputState`
//! (ADR 0082 for the pointer, ADR 0097 for keys). So a scripted click can
//! press an egui button, one over a panel stays out of the world, a wheel
//! over a panel scrolls it rather than the camera behind it, a held Shift
//! reaches egui's modifiers, and a key typed into a focused field never
//! reaches the game. `look` is raw motion (`mouse_delta_x/y`), routed as a
//! real mouse's: it reaches the game only while the cursor is captured and
//! no text field has focus (ADR 0110).

use flint_runtime::{parse_key_code, InputState};
use winit::dpi::PhysicalPosition;
use winit::event::{
    DeviceId, ElementState, MouseButton, MouseScrollDelta, TouchPhase, WindowEvent,
};
use winit::keyboard::{KeyCode, ModifiersState};
use flint_core::vfs;

/// Gamepad slot the script drives, away from slot 0 so a real pad plugged in
/// at the same time does not overwrite its values.
const SCRIPT_PAD: u32 = 9;

/// A pointer event from the script, for the player to route as winit's own.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PointerEvent {
    /// The cursor is at (x, y), in physical window pixels.
    Moved(f64, f64),
    Pressed(MouseButton),
    Released(MouseButton),
    /// The wheel turned (x, y) lines: up and right are positive.
    Wheel(f32, f32),
}

impl PointerEvent {
    /// The winit event a real mouse would have sent.
    pub fn to_window_event(self) -> WindowEvent {
        let device_id = DeviceId::dummy();
        let button = |state, button| WindowEvent::MouseInput {
            device_id,
            state,
            button,
        };
        match self {
            PointerEvent::Moved(x, y) => WindowEvent::CursorMoved {
                device_id,
                position: PhysicalPosition::new(x, y),
            },
            PointerEvent::Pressed(b) => button(ElementState::Pressed, b),
            PointerEvent::Released(b) => button(ElementState::Released, b),
            PointerEvent::Wheel(x, y) => WindowEvent::MouseWheel {
                device_id,
                delta: MouseScrollDelta::LineDelta(x, y),
                phase: TouchPhase::Moved,
            },
        }
    }
}

/// One thing the script did, for the player to route as a real device's.
#[derive(Debug, Clone, PartialEq)]
pub enum ScriptEvent {
    Pointer(PointerEvent),
    /// A key went down or up.
    Key(KeyCode, ElementState),
    /// The modifiers held now, sent just before the event of the Shift,
    /// Control, Alt or Super key that changed them, as winit sends
    /// `ModifiersChanged`. A second Shift while one is held changes nothing.
    Modifiers(ModifiersState),
    /// Text typed (`type`), for a focused egui field.
    Text(String),
    /// Raw mouse motion (`look`), in the units of a real mouse's.
    Look(f64, f64),
    /// A finger (`touch`, `drag`, `lift`): id, phase and window pixels.
    Touch(u64, TouchPhase, f64, f64),
}

/// What one frame of the script did, beyond writing `InputState`.
#[derive(Debug, Default)]
pub struct Applied {
    /// Keys, pointer, modifiers and text, in script order, for the player
    /// to route.
    pub events: Vec<ScriptEvent>,
}

impl Applied {
    /// The pointer events, in order.
    pub fn pointer(&self) -> Vec<PointerEvent> {
        self.events
            .iter()
            .filter_map(|e| match e {
                ScriptEvent::Pointer(p) => Some(*p),
                _ => None,
            })
            .collect()
    }

    /// The keys pressed, in order.
    pub fn keys_pressed(&self) -> Vec<KeyCode> {
        self.events
            .iter()
            .filter_map(|e| match e {
                ScriptEvent::Key(k, ElementState::Pressed) => Some(*k),
                _ => None,
            })
            .collect()
    }
}

/// The modifier a key holds, if it's one.
fn modifier(key: KeyCode) -> Option<ModifiersState> {
    Some(match key {
        KeyCode::ShiftLeft | KeyCode::ShiftRight => ModifiersState::SHIFT,
        KeyCode::ControlLeft | KeyCode::ControlRight => ModifiersState::CONTROL,
        KeyCode::AltLeft | KeyCode::AltRight => ModifiersState::ALT,
        KeyCode::SuperLeft | KeyCode::SuperRight => ModifiersState::SUPER,
        _ => return None,
    })
}

/// The rest of a line after its first `n` words, from its first non-space.
fn after_words(line: &str, n: usize) -> &str {
    let mut rest = line;
    for _ in 0..n {
        rest = rest.trim_start();
        rest = &rest[rest.find(char::is_whitespace).unwrap_or(rest.len())..];
    }
    rest.trim_start()
}

#[derive(Debug, Clone)]
enum Action {
    KeyDown(KeyCode),
    KeyUp(KeyCode),
    Axis(String, f32),
    ButtonDown(String),
    ButtonUp(String),
    /// An analog button's travel (a trigger part-way), 0 to 1.
    Pull(String, f32),
    Wheel(f32, f32),
    /// Pointer to (x, y), at once or gliding over the seconds given.
    Pointer(f64, f64, f64),
    MouseDown(MouseButton),
    MouseUp(MouseButton),
    Text(String),
    /// Raw motion (dx, dy), at once or spread over the seconds given.
    Look(f64, f64, f64),
    FingerDown(u64, f64, f64),
    /// Finger to (x, y), at once or gliding over the seconds given.
    FingerMove(u64, f64, f64, f64),
    FingerUp(u64),
}

/// Raw motion spread over a span of game time.
#[derive(Debug, Clone, Copy)]
struct Sweep {
    total: (f64, f64),
    start: f64,
    seconds: f64,
    /// How much of `total` has been fed so far.
    fed: (f64, f64),
}

impl Sweep {
    /// The motion due by `now` and not yet fed, and whether it's all fed.
    fn step(&mut self, now: f64) -> ((f64, f64), bool) {
        let k = ((now - self.start) / self.seconds).clamp(0.0, 1.0);
        let due = (self.total.0 * k, self.total.1 * k);
        let step = (due.0 - self.fed.0, due.1 - self.fed.1);
        self.fed = due;
        (step, k >= 1.0)
    }
}

/// A pointer moving from one place to another over a span of game time.
#[derive(Debug, Clone, Copy)]
struct Glide {
    from: (f64, f64),
    to: (f64, f64),
    start: f64,
    seconds: f64,
}

impl Glide {
    /// Where the pointer is at `now`, and whether it has arrived.
    fn at(&self, now: f64) -> ((f64, f64), bool) {
        let k = ((now - self.start) / self.seconds).clamp(0.0, 1.0);
        let lerp = |a: f64, b: f64| a + (b - a) * k;
        (
            (lerp(self.from.0, self.to.0), lerp(self.from.1, self.to.1)),
            k >= 1.0,
        )
    }
}

#[derive(Debug, Default, Clone)]
pub struct InputScript {
    events: Vec<(f64, Action)>,
    next: usize,
    /// Where the script last put the pointer.
    pointer: (f64, f64),
    glide: Option<Glide>,
    /// Modifier keys held down, for `ModifiersChanged`.
    modifier_keys: Vec<KeyCode>,
    /// `look`s still spreading their motion over frames.
    sweeps: Vec<Sweep>,
    /// Fingers down, where the script last put each, and any glide.
    fingers: std::collections::BTreeMap<u64, ((f64, f64), Option<Glide>)>,
}

fn parse_mouse_button(name: &str) -> Option<MouseButton> {
    Some(match name {
        "MouseLeft" => MouseButton::Left,
        "MouseRight" => MouseButton::Right,
        "MouseMiddle" => MouseButton::Middle,
        "MouseBack" => MouseButton::Back,
        "MouseForward" => MouseButton::Forward,
        _ => return None,
    })
}

impl InputScript {
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let text = vfs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("input script {path}: {e}"))?;
        Self::parse(&text).map_err(|e| anyhow::anyhow!("input script {path}: {e}"))
    }

    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let mut events = Vec::new();
        for (n, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let bad = |what: &str| anyhow::anyhow!("line {}: {what}: {raw}", n + 1);
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() < 3 {
                return Err(bad("expected <time> <verb> <arg>"));
            }
            let t: f64 = f[0].parse().map_err(|_| bad("bad time"))?;
            // A key or a mouse button, for down, up and tap.
            let held = |name: &str| -> anyhow::Result<(Action, Action)> {
                if let Some(b) = parse_mouse_button(name) {
                    return Ok((Action::MouseDown(b), Action::MouseUp(b)));
                }
                let k = parse_key_code(name).ok_or_else(|| bad("unknown key"))?;
                Ok((Action::KeyDown(k), Action::KeyUp(k)))
            };
            let number = |i: usize, what: &str| -> anyhow::Result<f64> {
                f.get(i)
                    .ok_or_else(|| bad(what))?
                    .parse()
                    .map_err(|_| bad(what))
            };
            match f[1] {
                "down" => events.push((t, held(f[2])?.0)),
                "up" => events.push((t, held(f[2])?.1)),
                "tap" => {
                    let (down, up) = held(f[2])?;
                    let hold: f64 = match f.get(3) {
                        Some(h) => h.parse().map_err(|_| bad("bad hold"))?,
                        None => 0.06,
                    };
                    events.push((t, down));
                    events.push((t + hold, up));
                }
                "axis" => {
                    let v: f32 = f
                        .get(3)
                        .ok_or_else(|| bad("axis needs a value"))?
                        .parse()
                        .map_err(|_| bad("bad axis value"))?;
                    events.push((t, Action::Axis(f[2].to_string(), v)));
                }
                "pull" => {
                    let v: f32 = f
                        .get(3)
                        .ok_or_else(|| bad("pull needs a value"))?
                        .parse()
                        .map_err(|_| bad("bad pull value"))?;
                    events.push((t, Action::Pull(f[2].to_string(), v)));
                }
                "press" => events.push((t, Action::ButtonDown(f[2].to_string()))),
                "release" => events.push((t, Action::ButtonUp(f[2].to_string()))),
                "wheel" => {
                    let lines = |s: &str| s.parse::<f32>().map_err(|_| bad("bad wheel lines"));
                    let dy = lines(f[2])?;
                    let dx = match f.get(3) {
                        Some(x) => lines(x)?,
                        None => 0.0,
                    };
                    events.push((t, Action::Wheel(dx, dy)));
                }
                "mouse" => {
                    let x = number(2, "mouse needs x and y")?;
                    let y = number(3, "mouse needs x and y")?;
                    let over = match f.get(4) {
                        Some(_) => number(4, "bad glide seconds")?,
                        None => 0.0,
                    };
                    if over < 0.0 {
                        return Err(bad("bad glide seconds"));
                    }
                    events.push((t, Action::Pointer(x, y, over)));
                }
                "click" => {
                    let x = number(2, "click needs x and y")?;
                    let y = number(3, "click needs x and y")?;
                    let b = match f.get(4) {
                        Some(name) => {
                            parse_mouse_button(name).ok_or_else(|| bad("unknown mouse button"))?
                        }
                        None => MouseButton::Left,
                    };
                    events.push((t, Action::Pointer(x, y, 0.0)));
                    events.push((t, Action::MouseDown(b)));
                    events.push((t + 0.06, Action::MouseUp(b)));
                }
                // Everything after the verb, inner spaces kept (a `#` still
                // starts a comment).
                "type" => events.push((t, Action::Text(after_words(line, 2).to_string()))),
                "look" => {
                    let dx = number(2, "look needs dx and dy")?;
                    let dy = number(3, "look needs dx and dy")?;
                    let over = match f.get(4) {
                        Some(_) => number(4, "bad look seconds")?,
                        None => 0.0,
                    };
                    if over < 0.0 {
                        return Err(bad("bad look seconds"));
                    }
                    events.push((t, Action::Look(dx, dy, over)));
                }
                "touch" | "drag" | "lift" => {
                    let id: u64 = f[2].parse().map_err(|_| bad("bad finger id"))?;
                    if f[1] == "lift" {
                        events.push((t, Action::FingerUp(id)));
                        continue;
                    }
                    let x = number(3, "a finger needs x and y")?;
                    let y = number(4, "a finger needs x and y")?;
                    if f[1] == "touch" {
                        events.push((t, Action::FingerDown(id, x, y)));
                        continue;
                    }
                    let over = match f.get(5) {
                        Some(_) => number(5, "bad drag seconds")?,
                        None => 0.0,
                    };
                    if over < 0.0 {
                        return Err(bad("bad drag seconds"));
                    }
                    events.push((t, Action::FingerMove(id, x, y, over)));
                }
                _ => return Err(bad("unknown verb")),
            }
        }
        // Stable: events at the same time keep file order.
        events.sort_by(|a, b| a.0.total_cmp(&b.0));
        Ok(Self {
            events,
            ..Self::default()
        })
    }

    /// The modifiers the held modifier keys make.
    fn modifiers(&self) -> ModifiersState {
        self.modifier_keys
            .iter()
            .filter_map(|&k| modifier(k))
            .fold(ModifiersState::empty(), |a, b| a | b)
    }

    /// Apply every event due at `now`. Two things wait for the next frame,
    /// as they would with a real hand on the keys and mouse:
    /// - a release of something pressed this same frame, so scripts always
    ///   see the press;
    /// - a mouse button press or a wheel turn in the frame the pointer
    ///   jumped, so egui has seen the pointer over its panels before it
    ///   decides who gets the click or the scroll. A glide's steps don't
    ///   hold them back.
    pub fn apply(&mut self, now: f64, input: &mut InputState) -> Applied {
        let mut out = Applied::default();
        let mut pressed_keys: Vec<KeyCode> = Vec::new();
        let mut pressed_buttons: Vec<String> = Vec::new();
        let mut pressed_mouse: Vec<MouseButton> = Vec::new();
        let mut fingers_down: Vec<u64> = Vec::new();
        let mut jumped = false;
        while let Some((t, action)) = self.events.get(self.next) {
            if *t > now {
                break;
            }
            match action {
                Action::KeyDown(k) => {
                    let was = self.modifiers();
                    if modifier(*k).is_some() && !self.modifier_keys.contains(k) {
                        self.modifier_keys.push(*k);
                    }
                    if self.modifiers() != was {
                        out.events.push(ScriptEvent::Modifiers(self.modifiers()));
                    }
                    out.events.push(ScriptEvent::Key(*k, ElementState::Pressed));
                    pressed_keys.push(*k);
                }
                Action::KeyUp(k) => {
                    if pressed_keys.contains(k) {
                        break;
                    }
                    let was = self.modifiers();
                    self.modifier_keys.retain(|m| m != k);
                    if self.modifiers() != was {
                        out.events.push(ScriptEvent::Modifiers(self.modifiers()));
                    }
                    out.events
                        .push(ScriptEvent::Key(*k, ElementState::Released));
                }
                Action::Axis(name, v) => input.process_gamepad_axis(SCRIPT_PAD, name.clone(), *v),
                Action::ButtonDown(b) => {
                    input.process_gamepad_button_down(SCRIPT_PAD, b.clone());
                    pressed_buttons.push(b.clone());
                }
                Action::ButtonUp(b) => {
                    if pressed_buttons.contains(b) {
                        break;
                    }
                    input.process_gamepad_button_up(SCRIPT_PAD, b.clone());
                }
                Action::Pull(b, v) => {
                    input.process_gamepad_button_changed(SCRIPT_PAD, b.clone(), *v)
                }
                Action::Wheel(dx, dy) => {
                    if jumped {
                        break;
                    }
                    out.events
                        .push(ScriptEvent::Pointer(PointerEvent::Wheel(*dx, *dy)));
                }
                Action::Pointer(x, y, over) => {
                    if *over > 0.0 {
                        self.glide = Some(Glide {
                            from: self.pointer,
                            to: (*x, *y),
                            start: *t,
                            seconds: *over,
                        });
                    } else {
                        self.glide = None;
                        self.pointer = (*x, *y);
                        out.events
                            .push(ScriptEvent::Pointer(PointerEvent::Moved(*x, *y)));
                        jumped = true;
                    }
                }
                Action::MouseDown(b) => {
                    if jumped {
                        break;
                    }
                    out.events
                        .push(ScriptEvent::Pointer(PointerEvent::Pressed(*b)));
                    pressed_mouse.push(*b);
                }
                Action::MouseUp(b) => {
                    if pressed_mouse.contains(b) {
                        break;
                    }
                    out.events
                        .push(ScriptEvent::Pointer(PointerEvent::Released(*b)));
                }
                Action::Text(text) => out.events.push(ScriptEvent::Text(text.clone())),
                Action::Look(dx, dy, over) => {
                    if *over > 0.0 {
                        self.sweeps.push(Sweep {
                            total: (*dx, *dy),
                            start: *t,
                            seconds: *over,
                            fed: (0.0, 0.0),
                        });
                    } else {
                        out.events.push(ScriptEvent::Look(*dx, *dy));
                    }
                }
                Action::FingerDown(id, x, y) => {
                    self.fingers.insert(*id, ((*x, *y), None));
                    out.events
                        .push(ScriptEvent::Touch(*id, TouchPhase::Started, *x, *y));
                    fingers_down.push(*id);
                }
                Action::FingerMove(id, x, y, over) => {
                    if let Some((at, glide)) = self.fingers.get_mut(id) {
                        if *over > 0.0 {
                            *glide = Some(Glide {
                                from: *at,
                                to: (*x, *y),
                                start: *t,
                                seconds: *over,
                            });
                        } else {
                            *glide = None;
                            *at = (*x, *y);
                            out.events
                                .push(ScriptEvent::Touch(*id, TouchPhase::Moved, *x, *y));
                        }
                    }
                }
                Action::FingerUp(id) => {
                    // As a key: a finger down this frame lifts next frame.
                    if fingers_down.contains(id) {
                        break;
                    }
                    if let Some((at, _)) = self.fingers.remove(id) {
                        out.events
                            .push(ScriptEvent::Touch(*id, TouchPhase::Ended, at.0, at.1));
                    }
                }
            }
            self.next += 1;
        }
        // A glide moves the pointer every frame until it arrives.
        if let Some(glide) = self.glide {
            let (at, arrived) = glide.at(now);
            if at != self.pointer {
                self.pointer = at;
                out.events
                    .push(ScriptEvent::Pointer(PointerEvent::Moved(at.0, at.1)));
            }
            if arrived {
                self.glide = None;
            }
        }
        // Each gliding finger moves every frame until it arrives.
        for (id, (at, glide)) in self.fingers.iter_mut() {
            let Some(g) = *glide else { continue };
            let (now_at, arrived) = g.at(now);
            if now_at != *at {
                *at = now_at;
                out.events
                    .push(ScriptEvent::Touch(*id, TouchPhase::Moved, now_at.0, now_at.1));
            }
            if arrived {
                *glide = None;
            }
        }
        // A spread `look` feeds its share of the motion every frame.
        let mut look = (0.0, 0.0);
        self.sweeps.retain_mut(|sweep| {
            let ((dx, dy), done) = sweep.step(now);
            look.0 += dx;
            look.1 += dy;
            !done
        });
        if look != (0.0, 0.0) {
            out.events.push(ScriptEvent::Look(look.0, look.1));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use PointerEvent::{Moved, Pressed, Released};

    const LEFT: MouseButton = MouseButton::Left;

    #[test]
    fn parses_and_orders_events() {
        let s = InputScript::parse(
            "# lap\n1.0 down KeyW\n0.5 tap KeyF 0.2  # pogo\n2 axis LeftStickX -0.5\n",
        )
        .unwrap();
        let times: Vec<f64> = s.events.iter().map(|e| e.0).collect();
        assert_eq!(times, vec![0.5, 0.7, 1.0, 2.0]);
    }

    /// The finger events of one frame.
    fn touches(applied: &Applied) -> Vec<(u64, TouchPhase, f64, f64)> {
        applied
            .events
            .iter()
            .filter_map(|e| match e {
                ScriptEvent::Touch(id, phase, x, y) => Some((*id, *phase, *x, *y)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn two_fingers_glide_and_lift_apart() {
        let mut input = InputState::new();
        let mut s = InputScript::parse(
            "1.0 touch 1 100 600\n1.0 drag 1 100 400 1.0\n1.0 touch 2 900 600\n\
             1.0 lift 2\n2.5 lift 1\n",
        )
        .unwrap();
        // Both land; the lift of a finger that landed this frame waits.
        assert_eq!(
            touches(&s.apply(1.0, &mut input)),
            vec![
                (1, TouchPhase::Started, 100.0, 600.0),
                (2, TouchPhase::Started, 900.0, 600.0),
            ]
        );
        // Next frame finger 2 lifts while finger 1 is half-way.
        assert_eq!(
            touches(&s.apply(1.5, &mut input)),
            vec![
                (2, TouchPhase::Ended, 900.0, 600.0),
                (1, TouchPhase::Moved, 100.0, 500.0),
            ]
        );
        assert_eq!(
            touches(&s.apply(2.0, &mut input)),
            vec![(1, TouchPhase::Moved, 100.0, 400.0)]
        );
        assert!(touches(&s.apply(2.2, &mut input)).is_empty(), "arrived");
        assert_eq!(
            touches(&s.apply(2.5, &mut input)),
            vec![(1, TouchPhase::Ended, 100.0, 400.0)]
        );
        assert!(InputScript::parse("1 drag 1 2\n").is_err());
        assert!(InputScript::parse("1 touch x 2 3\n").is_err());
    }

    #[test]
    fn pull_is_a_trigger_part_way() {
        let mut input = InputState::new();
        input
            .load_bindings(
                flint_runtime::InputConfig::from_toml_str(
                    r#"
version = 1
[actions.zoom_in]
kind = "axis1d"
[[actions.zoom_in.bindings]]
type = "gamepad_button"
button = "RightTrigger2"
gamepad = "any"
"#,
                )
                .unwrap(),
            )
            .unwrap();
        let mut s =
            InputScript::parse("1.0 pull RightTrigger2 0.6
2.0 pull RightTrigger2 0
").unwrap();
        s.apply(1.0, &mut input);
        assert!((input.action_value("zoom_in") - 0.6).abs() < 1e-6, "a half pull");
        s.apply(2.0, &mut input);
        assert_eq!(input.action_value("zoom_in"), 0.0, "let go");
        assert!(InputScript::parse("0 pull RightTrigger2").is_err());
    }

    #[test]
    fn rejects_unknown_key() {
        assert!(InputScript::parse("0 down KeyWW").is_err());
    }

    #[test]
    fn wheel_is_a_pointer_event() {
        let mut s = InputScript::parse("1.0 wheel 2\n1.0 wheel -0.5 1.5\n").unwrap();
        let mut input = InputState::new();
        assert!(s.apply(0.5, &mut input).events.is_empty());
        assert_eq!(
            s.apply(1.0, &mut input).pointer(),
            vec![
                PointerEvent::Wheel(0.0, 2.0),
                PointerEvent::Wheel(1.5, -0.5)
            ]
        );
        // The player routes it: the script itself leaves `InputState` alone.
        assert_eq!(input.mouse_wheel_delta(), (0.0, 0.0));
        assert!(s.apply(1.1, &mut input).events.is_empty());
        assert!(InputScript::parse("0 wheel up").is_err());
    }

    #[test]
    fn a_wheel_waits_a_frame_after_a_jump() {
        let mut s = InputScript::parse("1.0 mouse 300 200\n1.0 wheel -10\n").unwrap();
        let mut input = InputState::new();
        assert_eq!(
            s.apply(1.0, &mut input).pointer(),
            vec![Moved(300.0, 200.0)]
        );
        assert_eq!(
            s.apply(1.1, &mut input).pointer(),
            vec![PointerEvent::Wheel(0.0, -10.0)]
        );
    }

    #[test]
    fn a_wheel_is_a_real_wheel_event() {
        match PointerEvent::Wheel(0.5, -2.0).to_window_event() {
            WindowEvent::MouseWheel {
                delta: MouseScrollDelta::LineDelta(x, y),
                ..
            } => assert_eq!((x, y), (0.5, -2.0)),
            other => panic!("not a wheel: {other:?}"),
        }
    }

    #[test]
    fn tap_release_waits_a_frame() {
        let mut s = InputScript::parse("0 tap KeyF 0.001").unwrap();
        let mut input = InputState::new();
        let first = s.apply(0.5, &mut input).events;
        assert_eq!(
            first,
            [ScriptEvent::Key(KeyCode::KeyF, ElementState::Pressed)]
        );
        let second = s.apply(0.6, &mut input).events;
        assert_eq!(
            second,
            [ScriptEvent::Key(KeyCode::KeyF, ElementState::Released)]
        );
        // The script never writes keys itself: the player routes them.
        assert!(!input.is_key_down(KeyCode::KeyF));
    }

    #[test]
    fn modifier_keys_say_what_is_held_before_their_own_event() {
        use ElementState::{Pressed as Down, Released as Up};
        let mut s = InputScript::parse(
            "0 down ShiftLeft\n0 down ControlRight\n0.1 down ShiftRight\n\
             0.2 up ShiftLeft\n0.3 tap KeyA\n0.4 up ShiftRight\n0.4 up ControlRight\n",
        )
        .unwrap();
        let mut input = InputState::new();
        let both = ModifiersState::SHIFT | ModifiersState::CONTROL;
        let mut frame = |t| s.apply(t, &mut input).events;
        assert_eq!(
            frame(0.0),
            [
                ScriptEvent::Modifiers(ModifiersState::SHIFT),
                ScriptEvent::Key(KeyCode::ShiftLeft, Down),
                ScriptEvent::Modifiers(both),
                ScriptEvent::Key(KeyCode::ControlRight, Down),
            ]
        );
        // The other Shift, and one Shift up while the other is held, change
        // nothing that's held.
        assert_eq!(frame(0.1), [ScriptEvent::Key(KeyCode::ShiftRight, Down)]);
        assert_eq!(frame(0.2), [ScriptEvent::Key(KeyCode::ShiftLeft, Up)]);
        assert_eq!(frame(0.3), [ScriptEvent::Key(KeyCode::KeyA, Down)]);
        assert_eq!(
            frame(0.4),
            [
                ScriptEvent::Key(KeyCode::KeyA, Up),
                ScriptEvent::Modifiers(ModifiersState::CONTROL),
                ScriptEvent::Key(KeyCode::ShiftRight, Up),
                ScriptEvent::Modifiers(ModifiersState::empty()),
                ScriptEvent::Key(KeyCode::ControlRight, Up),
            ]
        );
    }

    #[test]
    fn type_keeps_the_rest_of_the_line() {
        let mut s = InputScript::parse("1.5   type   200  planks, oak  # a note\n").unwrap();
        let mut input = InputState::new();
        assert_eq!(
            s.apply(2.0, &mut input).events,
            [ScriptEvent::Text("200  planks, oak".into())]
        );
        assert!(InputScript::parse("1 type").is_err());
    }

    /// The pointer events of each frame, one frame per time.
    fn frames(src: &str, times: &[f64]) -> Vec<Vec<PointerEvent>> {
        let mut s = InputScript::parse(src).unwrap();
        let mut input = InputState::new();
        times
            .iter()
            .map(|&t| s.apply(t, &mut input).pointer())
            .collect()
    }

    #[test]
    fn a_click_moves_then_presses_then_releases_on_three_frames() {
        // egui must see the pointer before the press, and scripts the
        // press before the release, however coarse the frames.
        let got = frames("1.0 click 640 360\n", &[0.5, 1.0, 2.0, 3.0, 4.0]);
        assert_eq!(
            got,
            [
                vec![],
                vec![Moved(640.0, 360.0)],
                vec![Pressed(LEFT)],
                vec![Released(LEFT)],
                vec![],
            ]
        );
        let right = frames("0 click 1 2 MouseRight\n", &[0.0, 0.1, 0.2]);
        assert_eq!(right[1], [Pressed(MouseButton::Right)]);
        assert!(InputScript::parse("0 click 1 2 Left").is_err());
        assert!(InputScript::parse("0 click 1").is_err());
        assert!(InputScript::parse("0 mouse 1 2 -1").is_err());
    }

    #[test]
    fn mouse_buttons_share_down_up_and_tap_with_keys() {
        let got = frames(
            "0 mouse 10 20\n0.1 down MouseLeft\n0.2 up MouseLeft\n0.3 tap MouseMiddle 0.1\n",
            &[0.0, 0.1, 0.2, 0.3, 0.4],
        );
        assert_eq!(
            got,
            [
                vec![Moved(10.0, 20.0)],
                vec![Pressed(LEFT)],
                vec![Released(LEFT)],
                vec![Pressed(MouseButton::Middle)],
                vec![Released(MouseButton::Middle)],
            ]
        );
        // The script never writes mouse buttons itself: the player routes them.
        let mut s = InputScript::parse("0 down MouseLeft").unwrap();
        let mut input = InputState::new();
        s.apply(0.0, &mut input);
        assert!(!input.is_mouse_button_down(0));
    }

    #[test]
    fn a_glide_drags_a_held_button_across_frames() {
        let got = frames(
            "0 mouse 0 0\n0.5 down MouseLeft\n0.5 mouse 100 50 1\n2 up MouseLeft\n",
            &[0.0, 0.5, 1.0, 1.5, 2.0, 2.5],
        );
        assert_eq!(
            got,
            [
                vec![Moved(0.0, 0.0)],
                // The glide starts where the pointer is: no step yet.
                vec![Pressed(LEFT)],
                vec![Moved(50.0, 25.0)],
                vec![Moved(100.0, 50.0)],
                vec![Released(LEFT)],
                vec![],
            ]
        );
    }

    #[test]
    fn a_jump_cancels_a_glide() {
        let got = frames("0 mouse 100 0 1.0\n0.5 mouse 7 7\n", &[0.0, 0.25, 0.5, 1.0]);
        assert_eq!(
            got,
            [
                vec![],
                vec![Moved(25.0, 0.0)],
                vec![Moved(7.0, 7.0)],
                vec![]
            ]
        );
    }

    /// The raw motion of each frame, one frame per time.
    fn looks(src: &str, times: &[f64]) -> Vec<Vec<(f64, f64)>> {
        let mut s = InputScript::parse(src).unwrap();
        let mut input = InputState::new();
        times
            .iter()
            .map(|&t| {
                s.apply(t, &mut input)
                    .events
                    .into_iter()
                    .filter_map(|e| match e {
                        ScriptEvent::Look(dx, dy) => Some((dx, dy)),
                        _ => None,
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn look_is_raw_motion_at_once_or_spread() {
        assert_eq!(
            looks("1 look 40 -10
", &[0.5, 1.0, 1.5]),
            [vec![], vec![(40.0, -10.0)], vec![]]
        );
        // Spread over a second: a share each frame, adding up to it all.
        assert_eq!(
            looks("0 look 100 0 1
", &[0.0, 0.25, 0.5, 1.0, 2.0]),
            [vec![], vec![(25.0, 0.0)], vec![(25.0, 0.0)], vec![(50.0, 0.0)], vec![]]
        );
        // The script never writes it itself: the player routes it.
        let mut s = InputScript::parse("0 look 5 5").unwrap();
        let mut input = InputState::new();
        s.apply(0.0, &mut input);
        assert_eq!(input.raw_mouse_delta(), (0.0, 0.0));
        assert!(InputScript::parse("0 look 5").is_err());
        assert!(InputScript::parse("0 look 5 5 -1").is_err());
    }

    #[test]
    fn pointer_events_become_winit_events() {
        match Moved(3.0, 4.0).to_window_event() {
            WindowEvent::CursorMoved { position, .. } => {
                assert_eq!((position.x, position.y), (3.0, 4.0))
            }
            e => panic!("{e:?}"),
        }
        match Released(LEFT).to_window_event() {
            WindowEvent::MouseInput { state, button, .. } => {
                assert_eq!((state, button), (ElementState::Released, LEFT))
            }
            e => panic!("{e:?}"),
        }
    }
}
