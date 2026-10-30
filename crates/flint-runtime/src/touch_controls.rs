//! On-screen touch controls (#133): virtual sticks, buttons, a menu button
//! and a look-drag region, declared in the input config's
//! `[touch_controls]` block and drawn by the player.
//!
//! Why config and not a script: a stick is an input *device*, not game UI.
//! It has to be hit-tested before egui sees a finger, feed actions that are
//! bound only to keys, and answer within the frame the finger moves, none of
//! which a script's `on_draw_ui` can do. So the block layers with the rest
//! of `InputConfig` (built-in -> game -> user -> CLI), the logic lives here
//! beside `InputState`, and the player paints what [`TouchControls::widgets`]
//! reports. Scripts still compose the game's HUD; they can ask
//! `touch_controls_visible()` to make room for the controls.
//!
//! Geometry is authored in logical points (egui's, the `draw_*` space),
//! from an anchor at a screen edge or corner, inside the safe-area insets.
//! Touches arrive in physical surface pixels; [`TouchControls::set_metrics`]
//! gives the pixels-per-point and insets that join the two. The scene's
//! render scale never matters: the HUD is drawn at full resolution.
//!
//! The controls show only once a real finger has touched the screen, and
//! hide when a key or gamepad takes over, so a desktop player never sees
//! them. Each control holds the finger that grabbed it by touch id, so a
//! stick and a button work under two thumbs at once.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The `[touch_controls]` block of an input config.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TouchControlsConfig {
    /// Overall alpha of the drawn controls, 0 to 1.
    #[serde(default = "default_opacity")]
    pub opacity: f32,
    #[serde(default)]
    pub sticks: Vec<TouchStick>,
    #[serde(default)]
    pub buttons: Vec<TouchButton>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub look: Option<TouchLook>,
    /// A small button that stands in for Escape: it presses an action
    /// (`pause` by default) so a game's own pause menu opens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub menu: Option<TouchMenu>,
}

impl Default for TouchControlsConfig {
    fn default() -> Self {
        Self {
            opacity: default_opacity(),
            sticks: Vec::new(),
            buttons: Vec::new(),
            look: None,
            menu: None,
        }
    }
}

/// Where a control's offset is measured from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    TopLeft,
    Top,
    TopRight,
    Left,
    Center,
    Right,
    #[default]
    BottomLeft,
    Bottom,
    BottomRight,
}

impl Anchor {
    /// Horizontal and vertical sides: -1 left/top, 0 centre, 1 right/bottom.
    fn sides(self) -> (i8, i8) {
        match self {
            Anchor::TopLeft => (-1, -1),
            Anchor::Top => (0, -1),
            Anchor::TopRight => (1, -1),
            Anchor::Left => (-1, 0),
            Anchor::Center => (0, 0),
            Anchor::Right => (1, 0),
            Anchor::BottomLeft => (-1, 1),
            Anchor::Bottom => (0, 1),
            Anchor::BottomRight => (1, 1),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StickMode {
    /// The stick sits where it's drawn; a finger must land on it.
    Fixed,
    /// The stick's base jumps to wherever a finger lands in its region.
    #[default]
    Floating,
}

/// A virtual stick. Its deflection drives a two-axis action pair (`x`,
/// `y`), four direction actions, or both.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TouchStick {
    #[serde(default)]
    pub anchor: Anchor,
    /// Centre of the base at rest, in points inward from the anchor.
    #[serde(default = "default_stick_offset")]
    pub offset: [f32; 2],
    /// Base radius in points: a finger this far from the centre is full tilt.
    #[serde(default = "default_stick_radius")]
    pub radius: f32,
    #[serde(default)]
    pub mode: StickMode,
    /// Where a floating stick can be grabbed, as normalized `[x, y, w, h]`.
    /// Default: the anchor's side of the screen, below its top 40%.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<[f32; 4]>,
    /// Fraction of the radius that reads as centred.
    #[serde(default = "default_stick_deadzone")]
    pub deadzone: f32,
    /// Axis action for right (+) / left (-).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<String>,
    /// Axis action for up (+) / down (-).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub up: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub down: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right: Option<String>,
}

/// A round button that holds an action while a finger is on it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TouchButton {
    pub action: String,
    /// Short text drawn on the button (defaults to the action's name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default = "default_button_anchor")]
    pub anchor: Anchor,
    #[serde(default = "default_button_offset")]
    pub offset: [f32; 2],
    #[serde(default = "default_button_radius")]
    pub radius: f32,
    /// What the button adds to its action while held: 1 presses a button
    /// action, and -1 drives an axis action's other end, so two buttons can
    /// share one axis (#134).
    #[serde(default = "default_button_value")]
    pub value: f32,
}

/// Dragging here turns the view, as raw mouse motion does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TouchLook {
    /// Normalized `[x, y, w, h]`; default the right half.
    #[serde(default = "default_look_region")]
    pub region: [f32; 4],
    /// Mouse counts per point of finger travel.
    #[serde(default = "default_look_sensitivity")]
    pub sensitivity: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TouchMenu {
    #[serde(default = "default_menu_action")]
    pub action: String,
    #[serde(default = "default_menu_anchor")]
    pub anchor: Anchor,
    #[serde(default = "default_menu_offset")]
    pub offset: [f32; 2],
    #[serde(default = "default_menu_radius")]
    pub radius: f32,
}

fn default_opacity() -> f32 {
    0.85
}
fn default_stick_offset() -> [f32; 2] {
    [120.0, 120.0]
}
fn default_stick_radius() -> f32 {
    64.0
}
fn default_stick_deadzone() -> f32 {
    0.15
}
fn default_button_anchor() -> Anchor {
    Anchor::BottomRight
}
fn default_button_offset() -> [f32; 2] {
    [96.0, 110.0]
}
fn default_button_radius() -> f32 {
    38.0
}
fn default_button_value() -> f32 {
    1.0
}
fn default_look_region() -> [f32; 4] {
    [0.5, 0.0, 0.5, 1.0]
}
fn default_look_sensitivity() -> f32 {
    2.0
}
fn default_menu_action() -> String {
    "pause".into()
}
fn default_menu_anchor() -> Anchor {
    Anchor::TopRight
}
fn default_menu_offset() -> [f32; 2] {
    [36.0, 36.0]
}
fn default_menu_radius() -> f32 {
    22.0
}

impl TouchControlsConfig {
    pub fn validate(&self) -> Result<(), String> {
        let finite = |v: f32| v.is_finite();
        if !(0.0..=1.0).contains(&self.opacity) {
            return Err("touch_controls.opacity must be between 0 and 1".into());
        }
        for (i, s) in self.sticks.iter().enumerate() {
            if !(s.radius > 0.0 && finite(s.radius)) {
                return Err(format!("touch_controls.sticks[{i}].radius must be > 0"));
            }
            if !(0.0..1.0).contains(&s.deadzone) {
                return Err(format!(
                    "touch_controls.sticks[{i}].deadzone must be in [0, 1)"
                ));
            }
            let drives = [&s.x, &s.y, &s.up, &s.down, &s.left, &s.right];
            if drives.iter().all(|a| a.is_none()) {
                return Err(format!(
                    "touch_controls.sticks[{i}] drives no action: give x/y or up/down/left/right"
                ));
            }
        }
        for (i, b) in self.buttons.iter().enumerate() {
            if b.action.trim().is_empty() {
                return Err(format!("touch_controls.buttons[{i}].action is empty"));
            }
            if !(b.radius > 0.0 && finite(b.radius)) {
                return Err(format!("touch_controls.buttons[{i}].radius must be > 0"));
            }
        }
        if let Some(look) = &self.look {
            if !finite(look.sensitivity) {
                return Err("touch_controls.look.sensitivity must be finite".into());
            }
        }
        Ok(())
    }
}

/// When the controls are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TouchControlsMode {
    /// After a real finger touches, until a key or pad is used (the default).
    #[default]
    Auto,
    /// Always, and the mouse's emulated finger drives them: for testing a
    /// touch layout on a desktop (`flint-player --touch-controls always`).
    Always,
    Never,
}

impl TouchControlsMode {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "auto" => Self::Auto,
            "always" | "on" => Self::Always,
            "never" | "off" => Self::Never,
            _ => return None,
        })
    }
}

/// What a finger is holding.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Grip {
    Stick(usize),
    Button(usize),
    Menu,
    /// Look drag, from the last position seen (pixels).
    Look((f64, f64)),
}

impl Grip {
    /// Sticks and buttons keep their finger to themselves: neither egui
    /// nor the game's own touch reading sees it. A look drag shares.
    fn exclusive(self) -> bool {
        !matches!(self, Grip::Look(_))
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct StickState {
    /// Base centre while held (pixels); `None` at rest.
    base: Option<(f64, f64)>,
    /// The finger's position while held (pixels).
    finger: (f64, f64),
}

/// A control as the player should draw it, in logical points.
#[derive(Debug, Clone, PartialEq)]
pub enum TouchWidget {
    Stick {
        base: [f32; 2],
        knob: [f32; 2],
        radius: f32,
        /// Deflection, x right and y up, each in [-1, 1].
        value: [f32; 2],
        held: bool,
    },
    Button {
        center: [f32; 2],
        radius: f32,
        label: String,
        held: bool,
    },
    Menu {
        center: [f32; 2],
        radius: f32,
        held: bool,
    },
}

/// The live controls: config, layout metrics, and which finger holds what.
#[derive(Debug, Default)]
pub struct TouchControls {
    config: Option<TouchControlsConfig>,
    mode: TouchControlsMode,
    /// A real finger has touched since the last key or pad input.
    touch_seen: bool,
    /// Put away by the game (`set_touch_controls(false)`), e.g. while a menu
    /// wants every tap. Survives scene changes, as cursor capture does.
    suspended: bool,
    screen: (f64, f64),
    pixels_per_point: f64,
    /// Safe-area insets in points: left, top, right, bottom.
    insets: [f32; 4],
    grips: HashMap<u64, Grip>,
    sticks: Vec<StickState>,
}

impl TouchControls {
    pub fn new() -> Self {
        Self {
            screen: (1280.0, 720.0),
            pixels_per_point: 1.0,
            ..Self::default()
        }
    }

    pub fn config(&self) -> Option<&TouchControlsConfig> {
        self.config.as_ref()
    }

    pub fn set_config(&mut self, config: Option<TouchControlsConfig>) {
        self.release_all();
        self.sticks = vec![StickState::default(); config.as_ref().map_or(0, |c| c.sticks.len())];
        self.config = config;
    }

    pub fn mode(&self) -> TouchControlsMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: TouchControlsMode) {
        self.mode = mode;
        if !self.visible() {
            self.release_all();
        }
    }

    /// Screen size in physical pixels, pixels per logical point, and the
    /// safe-area insets in points (left, top, right, bottom).
    pub fn set_metrics(&mut self, screen: (f64, f64), pixels_per_point: f64, insets: [f32; 4]) {
        self.screen = (screen.0.max(1.0), screen.1.max(1.0));
        self.pixels_per_point = pixels_per_point.max(0.01);
        self.insets = insets.map(|v| if v.is_finite() { v.max(0.0) } else { 0.0 });
    }

    /// Put the controls away (`false`) or bring them back (`true`). While
    /// away they hold no fingers and every touch reaches the game, but a
    /// finger still counts as seen, so they return already showing.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.suspended = !enabled;
        if !self.visible() {
            self.release_all();
        }
    }

    /// Whether the game has the controls out (see [`Self::set_enabled`]).
    pub fn enabled(&self) -> bool {
        !self.suspended
    }

    /// Whether the controls are on screen now.
    pub fn visible(&self) -> bool {
        self.config.is_some()
            && !self.suspended
            && match self.mode {
                TouchControlsMode::Auto => self.touch_seen,
                TouchControlsMode::Always => true,
                TouchControlsMode::Never => false,
            }
    }

    /// A real finger touched: the controls appear (in auto mode).
    pub fn note_touch(&mut self) {
        self.touch_seen = true;
    }

    /// A key or pad took over: the controls go, letting go of every finger.
    pub fn note_other_device(&mut self) {
        if self.touch_seen {
            self.touch_seen = false;
            if !self.visible() {
                self.release_all();
            }
        }
    }

    fn release_all(&mut self) {
        self.grips.clear();
        for s in &mut self.sticks {
            *s = StickState::default();
        }
    }

    /// Whether a finger landing at `pos` (pixels) would be taken by a stick,
    /// button or the menu: the player keeps such a touch from egui.
    pub fn would_grab(&self, pos: (f64, f64)) -> bool {
        self.visible() && self.hit(pos, false).is_some_and(Grip::exclusive)
    }

    /// Whether finger `id` is held by a stick, button or the menu.
    pub fn holds(&self, id: u64) -> bool {
        self.grips.get(&id).is_some_and(|g| g.exclusive())
    }

    /// A finger landed. Returns true if a stick, button or the menu took it
    /// (it should then stay out of the game's own touch list).
    pub fn touch_start(&mut self, id: u64, pos: (f64, f64)) -> bool {
        if !self.visible() {
            return false;
        }
        // A reused id: whatever it held is over.
        self.touch_end(id);
        let Some(grip) = self.hit(pos, true) else {
            return false;
        };
        if let Grip::Stick(i) = grip {
            let base = match self.config.as_ref().unwrap().sticks[i].mode {
                StickMode::Fixed => self.stick_rest(i),
                StickMode::Floating => pos,
            };
            self.sticks[i] = StickState {
                base: Some(base),
                finger: pos,
            };
        }
        self.grips.insert(id, grip);
        grip.exclusive()
    }

    /// A finger moved. Returns (claimed, look delta in mouse counts).
    pub fn touch_move(&mut self, id: u64, pos: (f64, f64)) -> (bool, (f64, f64)) {
        let Some(grip) = self.grips.get_mut(&id) else {
            return (false, (0.0, 0.0));
        };
        match grip {
            Grip::Stick(i) => {
                self.sticks[*i].finger = pos;
                (true, (0.0, 0.0))
            }
            Grip::Look(last) => {
                let sens = self
                    .config
                    .as_ref()
                    .and_then(|c| c.look.as_ref())
                    .map_or(default_look_sensitivity(), |l| l.sensitivity)
                    as f64;
                let k = sens / self.pixels_per_point;
                let delta = ((pos.0 - last.0) * k, (pos.1 - last.1) * k);
                *last = pos;
                (false, delta)
            }
            Grip::Button(_) | Grip::Menu => (true, (0.0, 0.0)),
        }
    }

    /// A finger lifted or was cancelled. Returns true if a stick, button or
    /// the menu held it.
    pub fn touch_end(&mut self, id: u64) -> bool {
        match self.grips.remove(&id) {
            Some(Grip::Stick(i)) => {
                self.sticks[i] = StickState::default();
                true
            }
            Some(g) => g.exclusive(),
            None => false,
        }
    }

    /// What the controls add to `action` now, if anything drives it: a held
    /// button reads 1, a stick direction its deflection that way, a stick
    /// axis its signed deflection.
    pub fn action_value(&self, action: &str) -> Option<f32> {
        let config = self.config.as_ref()?;
        let mut out: Option<f32> = None;
        let mut add = |v: f32| *out.get_or_insert(0.0) += v;
        for (i, stick) in config.sticks.iter().enumerate() {
            let [x, y] = self.stick_value(i);
            let is = |a: &Option<String>| a.as_deref() == Some(action);
            if is(&stick.x) {
                add(x);
            }
            if is(&stick.y) {
                add(y);
            }
            if is(&stick.right) {
                add(x.max(0.0));
            }
            if is(&stick.left) {
                add((-x).max(0.0));
            }
            if is(&stick.up) {
                add(y.max(0.0));
            }
            if is(&stick.down) {
                add((-y).max(0.0));
            }
        }
        for grip in self.grips.values() {
            match grip {
                Grip::Button(i) if config.buttons[*i].action == action => {
                    add(config.buttons[*i].value)
                }
                Grip::Menu if config.menu.as_ref().is_some_and(|m| m.action == action) => add(1.0),
                _ => {}
            }
        }
        out
    }

    /// Stick `i`'s deflection, x right and y up.
    pub fn stick_value(&self, i: usize) -> [f32; 2] {
        let (Some(config), Some(state)) = (self.config.as_ref(), self.sticks.get(i)) else {
            return [0.0, 0.0];
        };
        let Some(base) = state.base else {
            return [0.0, 0.0];
        };
        let stick = &config.sticks[i];
        stick_vector(
            base,
            state.finger,
            stick.radius as f64 * self.pixels_per_point,
            stick.deadzone,
        )
    }

    /// Every control, laid out in points, for the player to draw. Empty
    /// while hidden.
    pub fn widgets(&self) -> Vec<TouchWidget> {
        let Some(config) = self.config.as_ref().filter(|_| self.visible()) else {
            return Vec::new();
        };
        let ppp = self.pixels_per_point;
        let pt = |p: (f64, f64)| [(p.0 / ppp) as f32, (p.1 / ppp) as f32];
        let held = |want: Grip| self.grips.values().any(|g| *g == want);
        let mut out = Vec::new();
        for (i, stick) in config.sticks.iter().enumerate() {
            let state = self.sticks[i];
            let base = state.base.unwrap_or_else(|| self.stick_rest(i));
            let value = self.stick_value(i);
            // The knob follows the finger, held inside the base.
            let r = stick.radius as f64 * ppp;
            let (dx, dy) = (state.finger.0 - base.0, state.finger.1 - base.1);
            let len = (dx * dx + dy * dy).sqrt();
            let knob = if state.base.is_none() || len < 1e-9 {
                base
            } else {
                let k = len.min(r) / len;
                (base.0 + dx * k, base.1 + dy * k)
            };
            out.push(TouchWidget::Stick {
                base: pt(base),
                knob: pt(knob),
                radius: stick.radius,
                value,
                held: state.base.is_some(),
            });
        }
        for (i, b) in config.buttons.iter().enumerate() {
            out.push(TouchWidget::Button {
                center: pt(self.place(b.anchor, b.offset)),
                radius: b.radius,
                label: b
                    .label
                    .clone()
                    .unwrap_or_else(|| b.action.replace('_', " ")),
                held: held(Grip::Button(i)),
            });
        }
        if let Some(m) = &config.menu {
            out.push(TouchWidget::Menu {
                center: pt(self.place(m.anchor, m.offset)),
                radius: m.radius,
                held: held(Grip::Menu),
            });
        }
        out
    }

    /// Opacity the player should draw at.
    pub fn opacity(&self) -> f32 {
        self.config.as_ref().map_or(1.0, |c| c.opacity)
    }

    /// A point `offset` points inward from `anchor`, inside the insets, in
    /// pixels.
    fn place(&self, anchor: Anchor, offset: [f32; 2]) -> (f64, f64) {
        let ppp = self.pixels_per_point;
        let (w, h) = (self.screen.0 / ppp, self.screen.1 / ppp);
        let [il, it, ir, ib] = self.insets.map(|v| v as f64);
        let (sx, sy) = anchor.sides();
        let axis = |side: i8, off: f32, lo: f64, hi: f64, size: f64| match side {
            -1 => lo + off as f64,
            1 => size - hi - off as f64,
            _ => (lo + size - hi) / 2.0 + off as f64,
        };
        (
            axis(sx, offset[0], il, ir, w) * ppp,
            axis(sy, offset[1], it, ib, h) * ppp,
        )
    }

    fn stick_rest(&self, i: usize) -> (f64, f64) {
        let s = &self.config.as_ref().unwrap().sticks[i];
        self.place(s.anchor, s.offset)
    }

    /// The control under `pos` (pixels): buttons and the menu first, then
    /// sticks, then the look region. `grabbing` skips controls already held
    /// (a held stick can't be grabbed by a second finger).
    fn hit(&self, pos: (f64, f64), grabbing: bool) -> Option<Grip> {
        let config = self.config.as_ref()?;
        let ppp = self.pixels_per_point;
        let within = |c: (f64, f64), r: f32| {
            let r = r as f64 * ppp;
            (pos.0 - c.0).powi(2) + (pos.1 - c.1).powi(2) <= r * r
        };
        if let Some(m) = &config.menu {
            // A little slack: the menu button is small and in a corner.
            if within(self.place(m.anchor, m.offset), m.radius * 1.4) {
                return Some(Grip::Menu);
            }
        }
        for (i, b) in config.buttons.iter().enumerate() {
            if within(self.place(b.anchor, b.offset), b.radius * 1.15) {
                return Some(Grip::Button(i));
            }
        }
        let norm = (pos.0 / self.screen.0, pos.1 / self.screen.1);
        let in_rect = |r: [f32; 4]| {
            let [x, y, w, h] = r.map(|v| v as f64);
            norm.0 >= x && norm.0 < x + w && norm.1 >= y && norm.1 < y + h
        };
        for (i, s) in config.sticks.iter().enumerate() {
            if grabbing && self.sticks[i].base.is_some() {
                continue;
            }
            let hit = match s.mode {
                StickMode::Fixed => within(self.stick_rest(i), s.radius * 1.5),
                StickMode::Floating => {
                    in_rect(s.region.unwrap_or_else(|| default_stick_region(s.anchor)))
                }
            };
            if hit {
                return Some(Grip::Stick(i));
            }
        }
        if let Some(look) = &config.look {
            if in_rect(look.region) {
                return Some(Grip::Look(pos));
            }
        }
        None
    }
}

/// Where a floating stick anchored at `anchor` can be grabbed by default:
/// its side of the screen, below the top 40%.
fn default_stick_region(anchor: Anchor) -> [f32; 4] {
    match anchor.sides().0 {
        1 => [0.5, 0.4, 0.5, 0.6],
        0 => [0.25, 0.4, 0.5, 0.6],
        _ => [0.0, 0.4, 0.5, 0.6],
    }
}

/// A stick's deflection for a finger at `finger` over a base at `base`
/// (both pixels) of `radius` pixels: x right and y *up*, each in [-1, 1],
/// with the length past `deadzone` (a fraction of the radius) rescaled to
/// start at 0, and clamped to the unit circle.
pub fn stick_vector(base: (f64, f64), finger: (f64, f64), radius: f64, deadzone: f32) -> [f32; 2] {
    let (dx, dy) = (finger.0 - base.0, finger.1 - base.1);
    let len = (dx * dx + dy * dy).sqrt();
    if radius <= 0.0 || len <= 1e-9 {
        return [0.0, 0.0];
    }
    let t = (len / radius).min(1.0);
    let dz = deadzone.clamp(0.0, 0.99) as f64;
    if t <= dz {
        return [0.0, 0.0];
    }
    let mag = (t - dz) / (1.0 - dz);
    [(dx / len * mag) as f32, (-dy / len * mag) as f32]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> TouchControlsConfig {
        toml::from_str(
            r#"
            [[sticks]]
            anchor = "bottom_left"
            offset = [100, 100]
            radius = 50
            deadzone = 0.1
            up = "move_forward"
            down = "move_backward"
            left = "move_left"
            right = "move_right"
            x = "strafe"

            [[buttons]]
            action = "jump"
            anchor = "bottom_right"
            offset = [80, 80]
            radius = 30

            [look]

            [menu]
            "#,
        )
        .unwrap()
    }

    fn controls() -> TouchControls {
        let mut c = TouchControls::new();
        c.set_config(Some(config()));
        c.set_metrics((1000.0, 500.0), 1.0, [0.0; 4]);
        c.note_touch();
        c
    }

    #[test]
    fn stick_vector_maps_deflection_to_axes() {
        let b = (100.0, 100.0);
        // Up the screen is +y; right is +x; past the radius clamps to 1.
        assert_eq!(stick_vector(b, (100.0, 0.0), 50.0, 0.0), [0.0, 1.0]);
        assert_eq!(stick_vector(b, (150.0, 100.0), 50.0, 0.0), [1.0, 0.0]);
        let half = stick_vector(b, (100.0, 125.0), 50.0, 0.0);
        assert!((half[1] + 0.5).abs() < 1e-6, "half down: {half:?}");
        // Inside the deadzone is still; just past it starts from 0.
        assert_eq!(stick_vector(b, (104.0, 100.0), 50.0, 0.1), [0.0, 0.0]);
        let past = stick_vector(b, (127.5, 100.0), 50.0, 0.1);
        assert!((past[0] - 0.5).abs() < 1e-6, "rescaled: {past:?}");
        // A diagonal stays on the unit circle.
        let d = stick_vector(b, (200.0, 0.0), 50.0, 0.0);
        assert!(((d[0] * d[0] + d[1] * d[1]).sqrt() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_floating_stick_drives_direction_and_axis_actions() {
        let mut c = controls();
        // Lands bottom-left: the base jumps there.
        assert!(c.touch_start(1, (200.0, 400.0)));
        assert_eq!(c.action_value("move_forward"), Some(0.0));
        c.touch_move(1, (200.0, 340.0)); // 60 px up, past the 50 px radius
        assert_eq!(c.action_value("move_forward"), Some(1.0));
        assert_eq!(c.action_value("move_backward"), Some(0.0));
        c.touch_move(1, (250.0, 400.0));
        assert_eq!(c.action_value("move_right"), Some(1.0));
        assert_eq!(c.action_value("strafe"), Some(1.0));
        c.touch_move(1, (150.0, 400.0));
        assert_eq!(c.action_value("strafe"), Some(-1.0));
        assert_eq!(c.action_value("move_left"), Some(1.0));
        assert!(c.touch_end(1));
        assert_eq!(c.action_value("move_left"), Some(0.0));
        assert_eq!(c.action_value("not_driven"), None);
    }

    #[test]
    fn a_stick_and_a_button_hold_two_fingers_apart() {
        let mut c = controls();
        assert!(c.touch_start(3, (200.0, 400.0)));
        assert!(c.touch_start(4, (920.0, 420.0)), "on the jump button");
        c.touch_move(3, (200.0, 330.0));
        assert_eq!(c.action_value("jump"), Some(1.0));
        assert_eq!(c.action_value("move_forward"), Some(1.0));
        // The button finger lifts; the stick holds.
        assert!(c.touch_end(4));
        assert_eq!(c.action_value("jump"), None);
        assert_eq!(c.action_value("move_forward"), Some(1.0));
        // A second finger can't steal a held stick: it lands on nothing.
        assert!(!c.touch_start(5, (100.0, 450.0)));
        assert!(!c.holds(5));
        c.touch_move(3, (200.0, 470.0));
        assert_eq!(c.action_value("move_backward"), Some(1.0));
    }

    #[test]
    fn look_drag_feeds_mouse_counts_in_points() {
        let mut c = controls();
        c.set_metrics((2000.0, 1000.0), 2.0, [0.0; 4]);
        assert!(!c.touch_start(7, (1500.0, 200.0)), "look shares its finger");
        let (claimed, delta) = c.touch_move(7, (1540.0, 180.0));
        assert!(!claimed);
        // 40 px is 20 points, times the default sensitivity of 2.
        assert_eq!(delta, (40.0, -20.0));
    }

    #[test]
    fn controls_show_for_a_finger_and_hide_for_keys() {
        let mut c = TouchControls::new();
        c.set_config(Some(config()));
        assert!(!c.visible());
        assert!(
            !c.touch_start(1, (200.0, 400.0)),
            "hidden controls take nothing"
        );
        c.note_touch();
        assert!(c.visible());
        assert!(c.touch_start(1, (200.0, 400.0)));
        c.touch_move(1, (200.0, 300.0));
        c.note_other_device();
        assert!(!c.visible());
        assert!(c.widgets().is_empty());
        assert_eq!(c.action_value("move_forward"), Some(0.0), "let go on hide");
        c.note_touch();
        assert!(c.visible());

        c.set_mode(TouchControlsMode::Never);
        assert!(!c.visible());
        c.set_mode(TouchControlsMode::Always);
        c.note_other_device();
        assert!(c.visible(), "always stays up");
    }

    #[test]
    fn anchors_respect_safe_area_and_points() {
        let mut c = controls();
        c.set_metrics((2000.0, 1000.0), 2.0, [10.0, 0.0, 30.0, 20.0]);
        let w = c.widgets();
        // Stick: bottom-left, 100 pt in from the left inset (10) and up from
        // the bottom inset (20): screen is 1000 x 500 points.
        let TouchWidget::Stick { base, .. } = &w[0] else {
            panic!()
        };
        assert_eq!(*base, [110.0, 380.0]);
        let TouchWidget::Button { center, label, .. } = &w[1] else {
            panic!()
        };
        assert_eq!(*center, [1000.0 - 30.0 - 80.0, 500.0 - 20.0 - 80.0]);
        assert_eq!(label, "jump");
        let TouchWidget::Menu { center, .. } = &w[2] else {
            panic!()
        };
        assert_eq!(*center, [1000.0 - 30.0 - 36.0, 36.0]);
        // A finger on the button in pixels hits it.
        assert!(c.would_grab((890.0 * 2.0, 400.0 * 2.0)));
    }

    #[test]
    fn the_menu_presses_pause() {
        let mut c = controls();
        assert!(c.touch_start(2, (1000.0 - 36.0, 36.0)));
        assert_eq!(c.action_value("pause"), Some(1.0));
        c.touch_end(2);
        assert_eq!(c.action_value("pause"), None);
    }

    #[test]
    fn two_buttons_drive_one_axis_from_either_end() {
        let mut c = TouchControls::new();
        c.set_config(Some(
            toml::from_str(
                r#"
                [[buttons]]
                action = "scoot"
                anchor = "bottom_left"
                offset = [80, 80]
                [[buttons]]
                action = "scoot"
                value = -1.0
                anchor = "bottom_right"
                offset = [80, 80]
                "#,
            )
            .unwrap(),
        ));
        c.set_metrics((1000.0, 500.0), 1.0, [0.0; 4]);
        c.note_touch();
        assert!(c.touch_start(1, (80.0, 420.0)));
        assert_eq!(c.action_value("scoot"), Some(1.0));
        assert!(c.touch_start(2, (920.0, 420.0)));
        assert_eq!(c.action_value("scoot"), Some(0.0), "both held cancel");
        c.touch_end(1);
        assert_eq!(c.action_value("scoot"), Some(-1.0));
    }

    #[test]
    fn put_away_controls_let_every_touch_through() {
        let mut c = controls();
        assert!(c.touch_start(1, (100.0, 400.0)));
        c.set_enabled(false);
        assert!(!c.visible());
        assert!(c.widgets().is_empty());
        assert_eq!(
            c.action_value("move_forward"),
            Some(0.0),
            "the stick let go"
        );
        assert!(!c.would_grab((920.0, 420.0)));
        assert!(!c.touch_start(2, (920.0, 420.0)));
        c.set_enabled(true);
        assert!(c.visible(), "back already showing: a finger was seen");
        assert!(c.touch_start(3, (920.0, 420.0)));
    }

    #[test]
    fn validation_catches_a_stick_with_no_action() {
        let bad: TouchControlsConfig = toml::from_str("[[sticks]]\n").unwrap();
        assert!(bad.validate().is_err());
        assert!(config().validate().is_ok());
    }
}
