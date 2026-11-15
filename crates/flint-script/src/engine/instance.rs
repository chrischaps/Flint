//! One loaded script (`ScriptInstance`): its compiled AST, its scope, and
//! which engine callbacks it defines, checked against the one table
//! (`flint_core::callbacks::CALLBACKS`) at load.

use flint_core::callbacks::{Callback, CallbackSpec};
use flint_core::diagnostic;
use rhai::{ASTNode, Expr, Scope, Stmt, AST};

/// Per-entity script instance
pub struct ScriptInstance {
    /// The whole script, top-level statements and all: what runs once, at
    /// the first lending of the world after it is added
    /// ([`ScriptEngine::lend`](super::ScriptEngine::lend)), and at a reload.
    pub ast: AST,
    /// What a callback is called on (#692): the script's functions and its
    /// top-level `import`s, nothing else ([`callable`]). Rhai runs an AST's
    /// statements before every `call_fn`, so calling on `ast` ran a
    /// top-level `log` or `get_entity` again before each callback.
    pub(crate) calls: AST,
    pub scope: Scope<'static>,
    /// Whether the top-level statements are still to run. They run at the
    /// next lending of the world, before any callback, with `self_entity()`
    /// this script's entity.
    pub(crate) top_level_pending: bool,
    pub source_path: String,
    /// Which callbacks the engine calls on this script, by [`Callback`]:
    /// those it defines at the table's argument count ([`scan`]). One
    /// defined at another count is reported at load and left out, never
    /// called (#700).
    pub(crate) defines: [bool; Callback::COUNT],
    /// Whether the script may call `complete_transition()`: it names the
    /// function, or imports a module that might (#362). A scene transition
    /// no loaded script can complete moves on by itself.
    pub completes_transition: bool,
    pub init_called: bool,
    /// The name of the entity the script is on, which decides the order scripts
    /// run in (empty until [`ScriptEngine::name_script`]). Entity ids differ run
    /// to run, so a name is what makes the order the same every time (#8).
    pub entity_name: String,
    /// The script's file as found, for its errors (#693): the scene's
    /// `source` unless [`super::ScriptEngine::add_named_script`] said where.
    pub file: String,
    /// The file and entity its `print`s name (#703), made at its first call
    /// and again after it is renamed.
    pub(crate) speaker: Option<std::sync::Arc<super::output::Speaker>>,
}

impl ScriptInstance {
    /// Whether the engine calls `callback` on this script: it defines it,
    /// at the argument count the engine calls it with.
    pub fn defines(&self, callback: Callback) -> bool {
        self.defines[callback as usize]
    }

    /// Who this script is, on `entity`, in what it prints.
    pub(crate) fn speaker(
        &mut self,
        entity: flint_core::EntityId,
    ) -> std::sync::Arc<super::output::Speaker> {
        if self.speaker.is_none() {
            self.speaker = Some(std::sync::Arc::new(super::output::Speaker {
                file: self.file.clone(),
                entity: super::callbacks::name_of(self, entity),
            }));
        }
        self.speaker
            .clone()
            .unwrap_or_else(super::output::Speaker::nobody)
    }

    pub fn new(ast: AST, source_path: String) -> Self {
        let defines = scan(&ast, &source_path);
        let completes_transition = may_complete_transition(&ast);
        let file = source_path.clone();

        Self {
            calls: callable(&ast),
            ast,
            scope: Scope::new(),
            top_level_pending: true,
            source_path,
            defines,
            completes_transition,
            init_called: false,
            entity_name: String::new(),
            file,
            speaker: None,
        }
    }

    /// Recompile with a new AST but preserve the scope (persistent state)
    pub fn hot_reload(&mut self, ast: AST) {
        self.defines = scan(&ast, &self.source_path);
        self.completes_transition = may_complete_transition(&ast);
        self.calls = callable(&ast);
        self.ast = ast;
        // Don't reset init_called — hot-reload preserves state
    }
}

/// What a callback is called on (#692): `ast`'s functions and those of its
/// top-level statements that are `import`s, which Rhai has to run before a
/// call for `alias::f()` to resolve in the callback (an import is an entry
/// in the call's global state, not a variable in the scope). The rest of
/// the top level (`let`s, `log`, `get_entity`, `play_sound`) ran once, into
/// the scope the callbacks are handed. An imported module is the
/// resolver's cached one, so its own top level runs once too
/// (`VfsModuleResolver`). An import inside a block was scoped to that block
/// and never reached a callback.
pub(crate) fn callable(ast: &AST) -> AST {
    let imports: Vec<Stmt> = ast
        .statements()
        .iter()
        .filter(|s| matches!(s, Stmt::Import(..)))
        .cloned()
        .collect();
    let mut calls = AST::new(imports, ast.shared_lib().clone());
    if let Some(source) = ast.source() {
        calls.set_source(source);
    }
    calls
}

/// Whether `ast` may call `complete_transition()` (#362): a call to it, or
/// the name in a string (`call("complete_transition")`), anywhere in the
/// script, function bodies and closures included; or an `import`, since a
/// module is resolved only when the script runs and could make the call.
///
/// Written when Rhai's `internals` (its AST walk) was on for `flint script
/// check` alone; every build has it since #692, so this could become a
/// walk. It reads the AST's `Debug` form, which names every call,
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

/// What is wrong with a script's `fn`s as callbacks (#700), found the one
/// way for the load's warnings ([`scan`]) and `flint script check`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CallbackProblem<'a> {
    /// `fn on_sequence_cue(seq)`: a callback with no definition at the
    /// count the engine calls it with, so it is not called.
    Arity {
        name: &'a str,
        params: usize,
        spec: &'static CallbackSpec,
    },
    /// `fn on_updte()`: one edit from a callback, and nothing in the script
    /// calls it or names it, so nothing ever will.
    Unknown { name: &'a str, near: &'static str },
}

impl CallbackProblem<'_> {
    /// The `fn` it is about.
    pub(crate) fn name(&self) -> &str {
        match self {
            CallbackProblem::Arity { name, .. } | CallbackProblem::Unknown { name, .. } => name,
        }
    }

    /// What is wrong, in a line.
    pub(crate) fn message(&self) -> String {
        match self {
            CallbackProblem::Arity { name, params, spec } => format!(
                "`fn {name}` has {params} parameter{}; the engine calls it with {}, so it never runs",
                if *params == 1 { "" } else { "s" },
                spec.arity()
            ),
            CallbackProblem::Unknown { name, .. } => {
                format!("`fn {name}` is not a callback, and nothing in this script calls it")
            }
        }
    }

    /// What to write instead.
    pub(crate) fn hint(&self) -> String {
        match self {
            CallbackProblem::Arity { spec, .. } if spec.name == Callback::Update.name() => {
                format!("`{}`: delta_time() is the frame's step", spec.signature())
            }
            CallbackProblem::Arity { spec, .. } => format!("`{}`", spec.signature()),
            CallbackProblem::Unknown { near, .. } => format!("did you mean `{near}`?"),
        }
    }
}

/// Every [`CallbackProblem`] of `ast`'s functions, in definition order.
/// `named(f)` says whether the script calls or names `f` itself, which
/// makes an `fn on_hit` a helper rather than a misspelt callback; it is
/// asked only of a name one edit from a callback.
pub(crate) fn callback_problems<'a>(
    ast: &'a AST,
    mut named: impl FnMut(&str) -> bool,
) -> Vec<CallbackProblem<'a>> {
    let fold = |s: &str| -> String {
        s.chars()
            .filter(|c| *c != '_')
            .flat_map(char::to_lowercase)
            .collect()
    };
    let names = || Callback::ALL.into_iter().map(Callback::name);
    let mut out = Vec::new();
    for f in ast.iter_functions() {
        if let Some(callback) = Callback::named(f.name) {
            let spec = callback.spec();
            // Rhai tells functions apart by name and count: another `fn` of
            // the name at the right count is the one called
            let called = ast
                .iter_functions()
                .any(|g| g.name == f.name && g.params.len() == spec.arity());
            if !called {
                out.push(CallbackProblem::Arity {
                    name: f.name,
                    params: f.params.len(),
                    spec,
                });
            }
            continue;
        }
        let looks_like_one = f.name.starts_with("on_") || names().any(|n| fold(n) == fold(f.name));
        if !looks_like_one {
            continue;
        }
        if let Some(near) = diagnostic::closest(f.name, names()) {
            if !named(f.name) {
                out.push(CallbackProblem::Unknown { name: f.name, near });
            }
        }
    }
    out
}

/// Which callbacks `ast` defines at the engine's argument count, warning
/// (once a load or reload) for each [`CallbackProblem`].
fn scan(ast: &AST, source_path: &str) -> [bool; Callback::COUNT] {
    let mut defines = [false; Callback::COUNT];
    for f in ast.iter_functions() {
        if let Some(callback) = Callback::named(f.name) {
            if f.params.len() == callback.spec().arity() {
                defines[callback as usize] = true;
            }
        }
    }
    for problem in callback_problems(ast, |name| names_function(ast, name)) {
        tracing::warn!(
            "warning ({source_path}): {} ({})",
            problem.message(),
            problem.hint()
        );
    }
    defines
}

/// Whether anything in `ast` calls the function `name` or names it: a call
/// or method call, a string (`call("f")`, `Fn("f")`), or the function
/// pointer the optimizer makes of `Fn("f")`.
pub(crate) fn names_function(ast: &AST, name: &str) -> bool {
    let mut found = false;
    ast.walk(&mut |nodes: &[ASTNode]| {
        found = match nodes.last() {
            Some(ASTNode::Stmt(Stmt::FnCall(x, _)))
            | Some(ASTNode::Expr(Expr::FnCall(x, _)))
            | Some(ASTNode::Expr(Expr::MethodCall(x, _))) => x.name == name,
            Some(ASTNode::Expr(Expr::StringConstant(s, _))) => s.as_str() == name,
            Some(ASTNode::Expr(Expr::DynamicConstant(v, _))) => v
                .read_lock::<rhai::FnPtr>()
                .is_some_and(|f| f.fn_name() == name),
            _ => false,
        };
        !found
    });
    found
}
