//! One loaded script (`ScriptInstance`): its compiled AST, its scope, and
//! which engine callbacks it defines, checked against `CALLBACK_ARITIES` at
//! load.

use flint_core::callbacks as cb;
use rhai::{Scope, AST};

/// Per-entity script instance
pub struct ScriptInstance {
    pub ast: AST,
    pub scope: Scope<'static>,
    pub source_path: String,
    pub has_on_init: bool,
    pub has_on_update: bool,
    pub has_on_collision: bool,
    pub has_on_collision_exit: bool,
    pub has_on_trigger_enter: bool,
    pub has_on_trigger_exit: bool,
    pub has_on_action: bool,
    pub has_on_interact: bool,
    pub has_on_draw_ui: bool,
    pub has_on_scene_exit: bool,
    pub has_on_scene_enter: bool,
    pub has_on_animation_end: bool,
    pub has_on_sequence_cue: bool,
    pub has_on_event: bool,
    pub has_on_expression: bool,
    pub has_on_persist_migrate: bool,
    /// Whether the script may call `complete_transition()`: it names the
    /// function, or imports a module that might (#362). A scene transition
    /// no loaded script can complete moves on by itself.
    pub completes_transition: bool,
    pub init_called: bool,
    /// The name of the entity the script is on, which decides the order scripts
    /// run in (empty until [`ScriptEngine::name_script`]). Entity ids differ run
    /// to run, so a name is what makes the order the same every time (#8).
    pub entity_name: String,
}

impl ScriptInstance {
    pub fn new(ast: AST, source_path: String) -> Self {
        validate_callbacks(&ast, &source_path);
        let has_on_init = has_function(&ast, cb::ON_INIT);
        let has_on_update = has_function(&ast, cb::ON_UPDATE);
        let has_on_collision = has_function(&ast, cb::ON_COLLISION);
        let has_on_collision_exit = has_function(&ast, cb::ON_COLLISION_EXIT);
        let has_on_trigger_enter = has_function(&ast, cb::ON_TRIGGER_ENTER);
        let has_on_trigger_exit = has_function(&ast, cb::ON_TRIGGER_EXIT);
        let has_on_action = has_function(&ast, cb::ON_ACTION);
        let has_on_interact = has_function(&ast, cb::ON_INTERACT);
        let has_on_draw_ui = has_function(&ast, cb::ON_DRAW_UI);
        let has_on_scene_exit = has_function(&ast, cb::ON_SCENE_EXIT);
        let has_on_scene_enter = has_function(&ast, cb::ON_SCENE_ENTER);
        let has_on_animation_end = has_function(&ast, cb::ON_ANIMATION_END);
        let has_on_sequence_cue = has_function(&ast, cb::ON_SEQUENCE_CUE);
        let has_on_event = has_function(&ast, cb::ON_EVENT);
        let has_on_expression = has_function(&ast, cb::ON_EXPRESSION);
        let has_on_persist_migrate = has_function(&ast, cb::ON_PERSIST_MIGRATE);
        let completes_transition = may_complete_transition(&ast);

        Self {
            ast,
            scope: Scope::new(),
            source_path,
            has_on_init,
            has_on_update,
            has_on_collision,
            has_on_collision_exit,
            has_on_trigger_enter,
            has_on_trigger_exit,
            has_on_action,
            has_on_interact,
            has_on_draw_ui,
            has_on_scene_exit,
            has_on_scene_enter,
            has_on_animation_end,
            has_on_sequence_cue,
            has_on_event,
            has_on_expression,
            has_on_persist_migrate,
            completes_transition,
            init_called: false,
            entity_name: String::new(),
        }
    }

    /// Recompile with a new AST but preserve the scope (persistent state)
    pub fn hot_reload(&mut self, ast: AST) {
        validate_callbacks(&ast, &self.source_path);
        self.has_on_init = has_function(&ast, cb::ON_INIT);
        self.has_on_update = has_function(&ast, cb::ON_UPDATE);
        self.has_on_collision = has_function(&ast, cb::ON_COLLISION);
        self.has_on_collision_exit = has_function(&ast, cb::ON_COLLISION_EXIT);
        self.has_on_trigger_enter = has_function(&ast, cb::ON_TRIGGER_ENTER);
        self.has_on_trigger_exit = has_function(&ast, cb::ON_TRIGGER_EXIT);
        self.has_on_action = has_function(&ast, cb::ON_ACTION);
        self.has_on_interact = has_function(&ast, cb::ON_INTERACT);
        self.has_on_draw_ui = has_function(&ast, cb::ON_DRAW_UI);
        self.has_on_scene_exit = has_function(&ast, cb::ON_SCENE_EXIT);
        self.has_on_scene_enter = has_function(&ast, cb::ON_SCENE_ENTER);
        self.has_on_animation_end = has_function(&ast, cb::ON_ANIMATION_END);
        self.has_on_sequence_cue = has_function(&ast, cb::ON_SEQUENCE_CUE);
        self.has_on_event = has_function(&ast, cb::ON_EVENT);
        self.has_on_expression = has_function(&ast, cb::ON_EXPRESSION);
        self.has_on_persist_migrate = has_function(&ast, cb::ON_PERSIST_MIGRATE);
        self.completes_transition = may_complete_transition(&ast);
        self.ast = ast;
        // Don't reset init_called — hot-reload preserves state
    }
}

/// Check if an AST contains a function definition with the given name
fn has_function(ast: &AST, name: &str) -> bool {
    ast.iter_functions().any(|f| f.name == name)
}

/// Whether `ast` may call `complete_transition()` (#362): a call to it, or
/// the name in a string (`call("complete_transition")`), anywhere in the
/// script, function bodies and closures included; or an `import`, since a
/// module is resolved only when the script runs and could make the call.
///
/// Rhai walks an AST only under its `internals` feature, which the player
/// and the web build leave off (`check` turns it on for `flint script
/// check`), so this reads the AST's `Debug` form, which names every call,
/// string and statement, and stops at the first match. Once a load or
/// reload, never a frame. `engine::tests` holds it to the shapes above, so
/// a Rhai upgrade that changes the form fails there, not in a game.
pub(crate) fn may_complete_transition(ast: &AST) -> bool {
    use std::fmt::Write;
    let mut find = Finder::new(&["complete_transition", "Import("]);
    // An `Err` here is the finder stopping the walk at a match
    let _ = write!(find, "{ast:?}");
    find.found
}

/// A `fmt::Write` that looks for any of its needles across the pieces it
/// is handed, and refuses the rest once one is found.
pub(crate) struct Finder<'a> {
    needles: &'a [&'a str],
    /// The end of what came before, long enough to hold a needle split
    /// across two pieces.
    tail: String,
    pub(crate) found: bool,
}

impl<'a> Finder<'a> {
    pub(crate) fn new(needles: &'a [&'a str]) -> Self {
        Self {
            needles,
            tail: String::new(),
            found: false,
        }
    }
}

impl std::fmt::Write for Finder<'_> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        if self.found {
            return Err(std::fmt::Error);
        }
        self.tail.push_str(s);
        if self.needles.iter().any(|n| self.tail.contains(n)) {
            self.found = true;
            return Err(std::fmt::Error);
        }
        let keep = self.needles.iter().map(|n| n.len()).max().unwrap_or(1) - 1;
        if self.tail.len() > keep {
            let mut cut = self.tail.len() - keep;
            while !self.tail.is_char_boundary(cut) {
                cut -= 1;
            }
            self.tail.drain(..cut);
        }
        Ok(())
    }
}

/// Expected parameter counts for engine callbacks.
pub(crate) const CALLBACK_ARITIES: &[(&str, usize)] = &[
    (cb::ON_INIT, 0),
    (cb::ON_UPDATE, 0),
    (cb::ON_COLLISION, 1),
    (cb::ON_COLLISION_EXIT, 1),
    (cb::ON_TRIGGER_ENTER, 1),
    (cb::ON_TRIGGER_EXIT, 1),
    (cb::ON_ACTION, 1),
    (cb::ON_INTERACT, 0),
    (cb::ON_DRAW_UI, 0),
    (cb::ON_SCENE_EXIT, 0),
    (cb::ON_SCENE_ENTER, 0),
    (cb::ON_ANIMATION_END, 1),
    (cb::ON_EVENT, 2),
    (cb::ON_EXPRESSION, 1),
    (cb::ON_PERSIST_MIGRATE, 1),
];

/// Warn at load time if a script defines a callback with the wrong number of parameters.
fn validate_callbacks(ast: &AST, source_path: &str) {
    for func in ast.iter_functions() {
        for &(name, expected) in CALLBACK_ARITIES {
            if func.name == name && func.params.len() != expected {
                let hint = if name == cb::ON_UPDATE && func.params.len() == 1 {
                    " Did you mean `fn on_update()`? Use `delta_time()` to access frame delta."
                } else {
                    ""
                };
                tracing::warn!(
                    "warning ({}): `fn {}` expects {} parameter(s) but has {}.{}",
                    source_path,
                    name,
                    expected,
                    func.params.len(),
                    hint
                );
            }
        }
    }
}
