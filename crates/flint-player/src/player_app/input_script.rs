//! Timed input script (`--input-script <file>`): key presses, gamepad
//! buttons and stick values played into `InputState` at set game times.
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
//! ```

use flint_runtime::{parse_key_code, InputState};
use winit::keyboard::KeyCode;

/// Gamepad slot the script drives, away from slot 0 so a real pad plugged in
/// at the same time does not overwrite its values.
const SCRIPT_PAD: u32 = 9;

#[derive(Debug, Clone)]
enum Action {
    KeyDown(KeyCode),
    KeyUp(KeyCode),
    Axis(String, f32),
    ButtonDown(String),
    ButtonUp(String),
}

#[derive(Debug, Default, Clone)]
pub struct InputScript {
    events: Vec<(f64, Action)>,
    next: usize,
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
            let key = |name: &str| parse_key_code(name).ok_or_else(|| bad("unknown key"));
            match f[1] {
                "down" => events.push((t, Action::KeyDown(key(f[2])?))),
                "up" => events.push((t, Action::KeyUp(key(f[2])?))),
                "tap" => {
                    let k = key(f[2])?;
                    let hold: f64 = match f.get(3) {
                        Some(h) => h.parse().map_err(|_| bad("bad hold"))?,
                        None => 0.06,
                    };
                    events.push((t, Action::KeyDown(k)));
                    events.push((t + hold, Action::KeyUp(k)));
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
                _ => return Err(bad("unknown verb")),
            }
        }
        // Stable: events at the same time keep file order.
        events.sort_by(|a, b| a.0.total_cmp(&b.0));
        Ok(Self { events, next: 0 })
    }

    /// Apply every event due at `now`. A release of something pressed this
    /// same frame waits for the next frame, so scripts always see the press.
    pub fn apply(&mut self, now: f64, input: &mut InputState) {
        let mut pressed_keys: Vec<KeyCode> = Vec::new();
        let mut pressed_buttons: Vec<String> = Vec::new();
        while let Some((t, action)) = self.events.get(self.next) {
            if *t > now {
                break;
            }
            match action {
                Action::KeyDown(k) => {
                    input.process_key_down(*k);
                    pressed_keys.push(*k);
                }
                Action::KeyUp(k) => {
                    if pressed_keys.contains(k) {
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
            }
            self.next += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn tap_release_waits_a_frame() {
        let mut s = InputScript::parse("0 tap KeyF 0.001").unwrap();
        let mut input = InputState::new();
        s.apply(0.5, &mut input);
        assert!(input.is_key_down(KeyCode::KeyF));
        input.end_frame();
        s.apply(0.6, &mut input);
        assert!(!input.is_key_down(KeyCode::KeyF));
    }
}
