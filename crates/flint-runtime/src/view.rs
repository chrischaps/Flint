//! Who the game is played as, and where it is seen and heard from (ADR 0138
//! decision 4, #1614).
//!
//! The kernel used to ask physics: whether its character controller had a
//! player, and where that player's eye was. A kernel that asks physics can't
//! build without it, and every other controller would have to pretend to be
//! physics' one. [`PlayerView`] is the answer as plain data: a controller
//! writes it (the `first-person` package's walker since #1618), and the
//! player, the cursor, the controls line, the listener and the scripts'
//! `is_near_player` read it.

use flint_core::{EntityId, Vec3};

/// Where an eye is and which way it looks: the camera's pose, and the
/// listener's.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Eye {
    /// Where the eye is, in the world
    pub position: Vec3,
    /// A point it looks at
    pub target: Vec3,
    /// Radians about +Y: 0 looks down +Z, a quarter turn down +X
    pub yaw: f32,
    /// Radians from level. A character's eye counts up from level;
    /// [`Eye::looking`] counts down, as the script camera's listener always
    /// has (#1666): kept so the listener moves as it did.
    pub pitch: f32,
    /// The camera's vertical field of view, degrees; `None` leaves it as
    /// the scene and the scripts set it (a first-person eye). A chase
    /// camera widens it with speed (#1631).
    pub fov: Option<f32>,
    /// Radians the camera turns about the way it looks (as
    /// `set_camera_roll`); 0 is level. A script's roll that frame wins.
    pub roll: f32,
}

impl Eye {
    /// An eye at `position` looking at `target`, its yaw and pitch read
    /// from the direction between them (a script's camera, which says only
    /// where it is and what it looks at). Its pitch counts down from level
    /// (see [`Eye::pitch`]).
    pub fn looking(position: Vec3, target: Vec3) -> Self {
        let dir = Vec3::new(
            target.x - position.x,
            target.y - position.y,
            target.z - position.z,
        );
        let yaw = dir.x.atan2(dir.z);
        let horiz = (dir.x * dir.x + dir.z * dir.z).sqrt();
        let pitch = (-dir.y).atan2(horiz);
        Self {
            position,
            target,
            yaw,
            pitch,
            fov: None,
            roll: 0.0,
        }
    }
}

/// Who the game is played as, and where it is seen and heard from (ADR
/// 0138). A run resource, reset at each scene's entry: a controller says
/// who its player is before the scene's first frame, and places the eye
/// each frame after the fixed steps.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlayerView {
    /// The entity the game is played as. `None`: no player (a menu, a 2D
    /// scene, a script's own controller that never says).
    pub player: Option<EntityId>,
    /// Whether the player looks with the mouse: the cursor is captured at
    /// the scene's entry, on a click while physics runs, and when the
    /// panels close.
    pub mouse_look: bool,
    /// The actions the controls line lists for the player whether the
    /// game's layers name them or not (the character controller's six).
    pub controls: &'static [&'static str],
    /// The eye a controller placed this frame; `None` leaves the camera
    /// where the scene and the scripts put it.
    pub eye: Option<Eye>,
    /// Where the listener went this frame, set by the kernel from the eye
    /// or the script's camera; `None` leaves it where it was.
    pub listener: Option<Eye>,
}

impl PlayerView {
    /// No player, no eye: what a scene starts with before its controller
    /// says, and what a host with no view reads.
    pub const NONE: PlayerView = PlayerView {
        player: None,
        mouse_look: false,
        controls: &[],
        eye: None,
        listener: None,
    };

    /// The frame's eye and listener forgotten, before a controller places
    /// them again; who the player is stays.
    pub fn new_frame(&mut self) {
        self.eye = None;
        self.listener = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_eye_looking_down_plus_x_has_a_quarter_turn_of_yaw() {
        let eye = Eye::looking(Vec3::new(1.0, 2.0, 3.0), Vec3::new(5.0, 2.0, 3.0));
        assert!((eye.yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert_eq!(eye.pitch, 0.0);
        // Looking down is a positive pitch here (#1666)
        let down = Eye::looking(Vec3::ZERO, Vec3::new(0.0, -1.0, 1.0));
        assert!((down.pitch - std::f32::consts::FRAC_PI_4).abs() < 1e-6);
    }

    #[test]
    fn a_new_frame_forgets_the_eye_and_keeps_the_player() {
        let at = Eye::looking(Vec3::ZERO, Vec3::new(0.0, 0.0, 1.0));
        let mut view = PlayerView {
            player: Some(EntityId::from_raw(7)),
            mouse_look: true,
            controls: &["jump"],
            eye: Some(at),
            listener: Some(at),
        };
        view.new_frame();
        assert_eq!(view.eye, None);
        assert_eq!(view.listener, None);
        assert_eq!(view.player, Some(EntityId::from_raw(7)));
        assert!(view.mouse_look);
        assert_eq!(PlayerView::default(), PlayerView::NONE);
    }
}
