//! The gamepad backend, as one path for every crate that reads pads (#221).
//!
//! With the `gamepad` feature this is gilrs itself. The feature is off by
//! default in this crate, which every runtime crate links for `InputState`;
//! flint-player's and flint-input-capture's `gamepad` turn it on. Without it,
//! it's a stand-in with the slice of gilrs's API Flint touches, so that a
//! headless CLI builds with no libudev and no gamepad code to cut around:
//! [`Gilrs`] is an enum with no values, `Gilrs::new()` always fails the way
//! it does on a machine with no backend, and everything that would hold one
//! compiles and is never reached.
//!
//! It lives here, beside [`InputState`](crate::InputState), so the player's
//! ordinary pad polling and a script's [`rumble`](crate::rumble) need no
//! rhythm crate (#748); flint-input-capture's 1 kHz thread reads pads through
//! it too. Read pads through `flint_runtime::pad`, never `gilrs` directly, or
//! the build without the feature breaks.

#[cfg(feature = "gamepad")]
pub use gilrs::*;

#[cfg(not(feature = "gamepad"))]
pub use stand_in::*;

#[cfg(not(feature = "gamepad"))]
mod stand_in {
    use std::fmt;
    use std::time::SystemTime;

    /// Why there is never a [`Gilrs`]: the build left gamepads out.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Error;

    impl fmt::Display for Error {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("built without gamepad support (the `gamepad` feature is off)")
        }
    }

    impl std::error::Error for Error {}

    /// No value of this type can exist, so nothing that holds one runs.
    pub enum Gilrs {}

    impl Gilrs {
        pub fn new() -> Result<Self, Error> {
            Err(Error)
        }

        pub fn next_event(&mut self) -> Option<Event> {
            match *self {}
        }

        pub fn gamepads(&self) -> std::iter::Empty<(GamepadId, Gamepad)> {
            match *self {}
        }
    }

    /// A pad, which never exists either.
    pub enum Gamepad {}

    impl Gamepad {
        pub fn name(&self) -> &str {
            match *self {}
        }

        pub fn power_info(&self) -> PowerInfo {
            match *self {}
        }

        pub fn value(&self, _axis: Axis) -> f32 {
            match *self {}
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct GamepadId(usize);

    impl From<GamepadId> for usize {
        fn from(id: GamepadId) -> usize {
            id.0
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum PowerInfo {
        Unknown,
    }

    /// A platform event code, carried as gilrs carries it.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Code;

    pub struct Event {
        pub id: GamepadId,
        pub event: EventType,
        pub time: SystemTime,
    }

    #[derive(Debug, Clone, Copy, PartialEq)]
    pub enum EventType {
        ButtonPressed(Button, Code),
        ButtonRepeated(Button, Code),
        ButtonReleased(Button, Code),
        ButtonChanged(Button, f32, Code),
        AxisChanged(Axis, f32, Code),
        Connected,
        Disconnected,
        Dropped,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum Button {
        South,
        East,
        North,
        West,
        C,
        Z,
        LeftTrigger,
        LeftTrigger2,
        RightTrigger,
        RightTrigger2,
        Select,
        Start,
        Mode,
        LeftThumb,
        RightThumb,
        DPadUp,
        DPadDown,
        DPadLeft,
        DPadRight,
        Unknown,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum Axis {
        LeftStickX,
        LeftStickY,
        LeftZ,
        RightStickX,
        RightStickY,
        RightZ,
        DPadX,
        DPadY,
        Unknown,
    }
}
