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
//! 3.0   axis   LeftStickX -0.35   # stick / trigger value on the script pad
//! 4.0   press  South              # gamepad button down
//! 4.2   release South
//! 5.0   wheel  1                  # wheel lines this frame: up is +, in
//! 5.5   wheel  -0.5 2             # a line is a notch; then an x amount
//! 6.0   mouse  640 360            # pointer to window pixels (as screenshots)
//! 6.1   down   MouseLeft          # mouse buttons: MouseLeft, MouseRight,
//! 6.2   mouse  900 500 0.5        #   MouseMiddle, MouseBack, MouseForward;
//! 6.8   up     MouseLeft          #   a glide to (900, 500) over 0.5 s drags
//! 7.0   click  320 200            # pointer there, then tap MouseLeft
//! 7.5   click  320 200 MouseRight
//! ```
//!
//! Keys, pad and wheel write `InputState` directly. The pointer (`mouse`,
//! `click` and mouse buttons) comes back from [`InputScript::apply`] as
//! [`PointerEvent`]s, which the player feeds through the same path as
//! winit's own events (ADR 0082): egui first, then extensions, then
//! `InputState`. So a scripted click can press an egui button, and one over
//! a panel stays out of the world.

use flint_runtime::{parse_key_code, InputState};
use winit::dpi::PhysicalPosition;
use winit::event::{DeviceId, ElementState, MouseButton, WindowEvent};
use winit::keyboard::KeyCode;

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
        }
    }
}

/// What one frame of the script did, beyond writing `InputState`.
#[derive(Debug, Default)]
pub struct Applied {
    /// Keys pressed, so the player can treat its debug keys (F2 and the
    /// rest) as it would a real press.
    pub keys: Vec<KeyCode>,
    /// Pointer events, in order, for the player to route.
    pub pointer: Vec<PointerEvent>,
}

#[derive(Debug, Clone)]
enum Action {
    KeyDown(KeyCode),
    KeyUp(KeyCode),
    Axis(String, f32),
    ButtonDown(String),
    ButtonUp(String),
    Wheel(f32, f32),
    /// Pointer to (x, y), at once or gliding over the seconds given.
    Pointer(f64, f64, f64),
    MouseDown(MouseButton),
    MouseUp(MouseButton),
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
        let text = std::fs::read_to_string(path)
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

    /// Apply every event due at `now`. Two things wait for the next frame,
    /// as they would with a real hand on the keys and mouse:
    /// - a release of something pressed this same frame, so scripts always
    ///   see the press;
    /// - a mouse button press in the frame the pointer jumped, so egui has
    ///   seen the pointer over its panels before it decides who gets the
    ///   click. A glide's steps don't hold a press back.
    pub fn apply(&mut self, now: f64, input: &mut InputState) -> Applied {
        let mut out = Applied::default();
        let mut pressed_buttons: Vec<String> = Vec::new();
        let mut pressed_mouse: Vec<MouseButton> = Vec::new();
        let mut jumped = false;
        while let Some((t, action)) = self.events.get(self.next) {
            if *t > now {
                break;
            }
            match action {
                Action::KeyDown(k) => {
                    input.process_key_down(*k);
                    out.keys.push(*k);
                }
                Action::KeyUp(k) => {
                    if out.keys.contains(k) {
                        break;
                    }
                    input.process_key_up(*k);
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
                Action::Wheel(dx, dy) => input.process_mouse_wheel(*dx, *dy),
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
                        out.pointer.push(PointerEvent::Moved(*x, *y));
                        jumped = true;
                    }
                }
                Action::MouseDown(b) => {
                    if jumped {
                        break;
                    }
                    out.pointer.push(PointerEvent::Pressed(*b));
                    pressed_mouse.push(*b);
                }
                Action::MouseUp(b) => {
                    if pressed_mouse.contains(b) {
                        break;
                    }
                    out.pointer.push(PointerEvent::Released(*b));
                }
            }
            self.next += 1;
        }
        // A glide moves the pointer every frame until it arrives.
        if let Some(glide) = self.glide {
            let (at, arrived) = glide.at(now);
            if at != self.pointer {
                self.pointer = at;
                out.pointer.push(PointerEvent::Moved(at.0, at.1));
            }
            if arrived {
                self.glide = None;
            }
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

    #[test]
    fn rejects_unknown_key() {
        assert!(InputScript::parse("0 down KeyWW").is_err());
    }

    #[test]
    fn wheel_lasts_one_frame() {
        let mut s = InputScript::parse("1.0 wheel 2\n1.0 wheel -0.5 1.5\n").unwrap();
        let mut input = InputState::new();
        s.apply(0.5, &mut input);
        assert_eq!(input.mouse_wheel_delta(), (0.0, 0.0));
        s.apply(1.0, &mut input);
        assert_eq!(input.mouse_wheel_delta(), (1.5, 1.5));
        input.end_frame();
        s.apply(1.1, &mut input);
        assert_eq!(input.mouse_wheel_delta(), (0.0, 0.0));
        assert!(InputScript::parse("0 wheel up").is_err());
    }

    #[test]
    fn tap_release_waits_a_frame() {
        let mut s = InputScript::parse("0 tap KeyF 0.001").unwrap();
        let mut input = InputState::new();
        assert_eq!(s.apply(0.5, &mut input).keys, [KeyCode::KeyF]);
        assert!(input.is_key_down(KeyCode::KeyF));
        input.end_frame();
        assert!(s.apply(0.6, &mut input).keys.is_empty());
        assert!(!input.is_key_down(KeyCode::KeyF));
    }

    /// The pointer events of each frame, one frame per time.
    fn frames(src: &str, times: &[f64]) -> Vec<Vec<PointerEvent>> {
        let mut s = InputScript::parse(src).unwrap();
        let mut input = InputState::new();
        times
            .iter()
            .map(|&t| s.apply(t, &mut input).pointer)
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
