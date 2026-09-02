//! Script callback name constants.
//!
//! These match the Rhai function names that the engine dispatches to.

pub const ON_INIT: &str = "on_init";
pub const ON_UPDATE: &str = "on_update";
pub const ON_COLLISION: &str = "on_collision";
pub const ON_COLLISION_EXIT: &str = "on_collision_exit";
pub const ON_TRIGGER_ENTER: &str = "on_trigger_enter";
pub const ON_TRIGGER_EXIT: &str = "on_trigger_exit";
pub const ON_ACTION: &str = "on_action";
pub const ON_INTERACT: &str = "on_interact";
pub const ON_DRAW_UI: &str = "on_draw_ui";
pub const ON_SCENE_EXIT: &str = "on_scene_exit";
pub const ON_SCENE_ENTER: &str = "on_scene_enter";
pub const ON_ANIMATION_END: &str = "on_animation_end";
