//! Reading an entity's style back (#372): what `get_entity_style`,
//! `get_entity_style_variant` and `get_style_param` ask.
//!
//! flint-script doesn't link the renderer, so it can't see the stylize
//! registry or the scene's `[post_process]` styles. The host hands the
//! script engine a [`StyleReader`] that can (the player's
//! `flint_player::SceneStyles`, over `flint_render::entity_style`), and
//! replaces it when the scene's post-process settings change. Without one,
//! every entity reads as unstyled.
//!
//! The reads see the world as it is when the script calls them: a style
//! set this frame with `set_entity_style` is a queued command, and reads
//! back from the next frame on.

use flint_core::EntityId;
use flint_ecs::FlintWorld;

/// The style an entity is drawn in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityStyle {
    /// The style's name: `"ascii"`, `"pencil"`, `"kuwahara"`.
    pub style: String,
    /// Its variant's name, the one the style mask's id stands for
    /// (`"ascii{ramp=blocks}"`); a whole-frame style's is its own name.
    pub variant: String,
}

/// Answers style questions about the world for scripts. Installed with
/// `ScriptEngine::set_style_reader` / `ScriptSystem::set_style_reader`.
pub trait StyleReader: Send + Sync {
    /// The style `id` is drawn in: its own, one inherited from an
    /// ancestor, or the scene's whole-frame style. `None` for none.
    fn entity_style(&self, world: &FlintWorld, id: EntityId) -> Option<EntityStyle>;

    /// The value setting `param` of that style has for `id`: the entity's,
    /// else the scene's, else the style's default. A number, a string, a
    /// colour as a 3-array, or a bool. `None` when unstyled or no such
    /// setting.
    fn style_param(&self, world: &FlintWorld, id: EntityId, param: &str) -> Option<toml::Value>;
}
