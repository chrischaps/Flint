//! The engine's script callbacks: one table (#700).
//!
//! Each row of [`callbacks!`] below is one callback: its [`Callback`]
//! variant, its name constant (`ON_UPDATE`), the Rhai function name the
//! engine dispatches to, its parameter names and when it runs. Everything
//! else is built from it: a loaded script's set of callbacks
//! (`ScriptInstance::defines`), the load-time check of each `fn on_*` (a
//! name not in the table, or a count the engine doesn't call with), `flint
//! script check`, `flint script api` and the book's callback table. Adding
//! a callback is one row here and its dispatch call in flint-script's
//! `engine/callbacks.rs`.
//!
//! The book lists the callbacks in table order, so a new row goes where a
//! reader would look for it.

/// One callback the engine calls on a script that defines it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallbackSpec {
    /// The Rhai function name: `on_update`.
    pub name: &'static str,
    /// The parameters the engine calls it with, by name; their count is the
    /// one arity it is called at.
    pub params: &'static [&'static str],
    /// When it runs, in a line.
    pub doc: &'static str,
}

impl CallbackSpec {
    /// How many arguments the engine passes.
    pub const fn arity(&self) -> usize {
        self.params.len()
    }

    /// `fn on_event(name, data)`: how a script declares it.
    pub fn signature(&self) -> String {
        format!("fn {}({})", self.name, self.params.join(", "))
    }
}

macro_rules! callbacks {
    ($(
        $(#[$meta:meta])*
        $variant:ident, $konst:ident = $name:literal ($($param:literal),*) $doc:literal;
    )*) => {
        $(
            $(#[$meta])*
            pub const $konst: &str = $name;
        )*

        /// A callback, as an index into [`CALLBACKS`].
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[repr(u8)]
        pub enum Callback {
            $($variant,)*
        }

        /// Every callback, in [`Callback`] order.
        pub const CALLBACKS: [CallbackSpec; Callback::COUNT] = [
            $(CallbackSpec { name: $name, params: &[$($param),*], doc: $doc },)*
        ];

        impl Callback {
            /// Every callback, in table order.
            pub const ALL: [Callback; Callback::COUNT] = [$(Callback::$variant,)*];
            /// How many callbacks there are.
            pub const COUNT: usize = [$($name),*].len();
        }
    };
}

callbacks! {
    Init, ON_INIT = "on_init"() "Once, when the script is first loaded.";
    Update, ON_UPDATE = "on_update"()
        "Every frame the game isn't paused; delta_time() is the frame's step.";
    DrawUi, ON_DRAW_UI = "on_draw_ui"()
        "Every frame after on_update, paused or not: the HUD's draw calls.";
    Ui, ON_UI = "on_ui"("element_id", "event")
        "Something happened to an element of a UI document this script loaded: focus, blur, activate, change or back; paused or not, before on_draw_ui.";
    Collision, ON_COLLISION = "on_collision"("other_id")
        "This entity collided with another.";
    CollisionExit, ON_COLLISION_EXIT = "on_collision_exit"("other_id")
        "A contact with another entity ended.";
    TriggerEnter, ON_TRIGGER_ENTER = "on_trigger_enter"("other_id")
        "Another entity entered this trigger volume.";
    TriggerExit, ON_TRIGGER_EXIT = "on_trigger_exit"("other_id")
        "Another entity left this trigger volume.";
    Action, ON_ACTION = "on_action"("action_name")
        "An input action fired (\"jump\", \"interact\").";
    Interact, ON_INTERACT = "on_interact"()
        "The player pressed Interact within this entity's interactable range.";
    SceneExit, ON_SCENE_EXIT = "on_scene_exit"() "The scene is about to be left.";
    SceneEnter, ON_SCENE_ENTER = "on_scene_enter"() "A scene was entered.";
    AnimationEnd, ON_ANIMATION_END = "on_animation_end"("clip")
        "A once sprite clip finished.";
    SequenceCue, ON_SEQUENCE_CUE = "on_sequence_cue"("sequence", "cue")
        "An animation sequence passed a cue event.";
    Event, ON_EVENT = "on_event"("name", "data")
        "A game event fired by a script, the engine or a player extension.";
    /// `on_expression(state)`: a new state from outside the process (Flint#518).
    Expression, ON_EXPRESSION = "on_expression"("state")
        "A new state arrived from outside the process (a timeline or a socket).";
    /// `on_persist_migrate(from)`: the save data loaded is at version `from`,
    /// lower than the game's `[project] save_version` (#726).
    PersistMigrate, ON_PERSIST_MIGRATE = "on_persist_migrate"("from")
        "Save data loaded at a version below save_version; runs before on_init.";
}

impl Callback {
    /// Its row of the table.
    pub const fn spec(self) -> &'static CallbackSpec {
        &CALLBACKS[self as usize]
    }

    /// The Rhai function name: `on_update`.
    pub const fn name(self) -> &'static str {
        self.spec().name
    }

    /// The callback named `name`, if the engine has one.
    pub fn named(name: &str) -> Option<Callback> {
        Callback::ALL.into_iter().find(|c| c.name() == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_and_the_enum_agree() {
        for (i, c) in Callback::ALL.into_iter().enumerate() {
            assert_eq!(c as usize, i);
            assert_eq!(c.spec(), &CALLBACKS[i]);
            assert_eq!(Callback::named(c.name()), Some(c));
        }
        let mut names: Vec<_> = CALLBACKS.iter().map(|c| c.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), Callback::COUNT, "a name is in the table twice");
        assert!(CALLBACKS.iter().all(|c| c.name.starts_with("on_")));
        assert_eq!(Callback::named("on_updte"), None);
    }

    #[test]
    fn a_sequence_cue_takes_two() {
        assert_eq!(Callback::SequenceCue.spec().arity(), 2);
        assert_eq!(
            Callback::SequenceCue.spec().signature(),
            "fn on_sequence_cue(sequence, cue)"
        );
        assert_eq!(ON_SEQUENCE_CUE, Callback::SequenceCue.name());
    }
}
