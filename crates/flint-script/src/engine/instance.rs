//! One loaded script (`ScriptInstance`): its compiled AST, its scope, and
//! which engine callbacks it defines, checked against the one table
//! (`flint_core::callbacks::CALLBACKS`) at load.

use flint_core::callbacks::{Callback, CallbackSpec};
use flint_core::diagnostic;
use rhai::{ASTFlags, ASTNode, Expr, Ident, ImmutableString, Position, Scope, Stmt, AST};

/// Per-entity script instance
pub struct ScriptInstance {
    /// The whole script, top-level statements and all: what runs once, at
    /// the first lending of the world after it is added
    /// ([`ScriptEngine::lend`](super::ScriptEngine::lend)), and at a reload.
    pub ast: AST,
    /// What a callback is called on (#692): the script's functions, its
    /// top-level `import`s and the constants it reads as `global::NAME`,
    /// nothing else ([`callable`]). Rhai runs an AST's statements before
    /// every `call_fn`, so calling on `ast` ran a top-level `log` or
    /// `get_entity` again before each callback.
    pub(crate) calls: AST,
    /// The names the script reads as `global::NAME` (#1012), in its
    /// functions or at its top level ([`global_reads`]).
    pub(crate) global_reads: Vec<ImmutableString>,
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
    /// Each callback's profiler site (`<source>:<callback>`, #1097), made
    /// the first time the callback runs while the recorder records.
    trace_sites: Vec<(String, &'static flint_core::profile::Site)>,
    /// [`Self::calls`] with each function timed (#1100,
    /// `fn_times::profiled`): made the first time a callback is called
    /// while profiling, and again after `calls` is rebuilt. `Some(None)`:
    /// made, and nothing to time (or it didn't compile).
    pub(crate) profiled: Option<Option<AST>>,
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

    /// `callback`'s scope for the recorder: `scripts/hud.rhai:on_draw_ui`,
    /// the name its `--perf` time goes under. Asked only while the
    /// recorder records; after a callback's first time it is a lookup.
    pub(crate) fn trace_site(&mut self, callback: &str) -> &'static flint_core::profile::Site {
        if let Some((_, site)) = self.trace_sites.iter().find(|(c, _)| c == callback) {
            return site;
        }
        let site = flint_core::profile::site_named(&format!("{}:{callback}", self.source_path));
        self.trace_sites.push((callback.to_string(), site));
        site
    }

    pub fn new(ast: AST, source_path: String) -> Self {
        let defines = scan(&ast, &source_path);
        let completes_transition = may_complete_transition(&ast);
        let file = source_path.clone();
        let scope = Scope::new();
        let global_reads = global_reads(&ast);

        Self {
            calls: callable(&ast, &global_reads, &scope),
            global_reads,
            ast,
            scope,
            top_level_pending: true,
            source_path,
            defines,
            completes_transition,
            init_called: false,
            entity_name: String::new(),
            file,
            speaker: None,
            trace_sites: Vec::new(),
            profiled: None,
        }
    }

    /// Make [`Self::calls`]' timed copy, if it isn't made yet, with `engine`.
    pub(crate) fn make_profiled(&mut self, engine: &rhai::Engine) {
        if self.profiled.is_none() {
            self.profiled = Some(crate::fn_times::profiled(
                engine,
                &self.calls,
                &self.source_path,
            ));
        }
    }

    /// Recompile with a new AST but preserve the scope (persistent state)
    pub fn hot_reload(&mut self, ast: AST) {
        self.defines = scan(&ast, &self.source_path);
        self.completes_transition = may_complete_transition(&ast);
        self.global_reads = global_reads(&ast);
        self.ast = ast;
        self.top_level_ran();
        // Don't reset init_called — hot-reload preserves state
    }

    /// The scope now holds the constants the top level made (it ran at a
    /// lending, or a reload copied new ones in): call on their values.
    pub(crate) fn top_level_ran(&mut self) {
        self.calls = callable(&self.ast, &self.global_reads, &self.scope);
        self.profiled = None;
    }
}

/// What a callback is called on (#692): `ast`'s functions and two kinds of
/// top-level statement, which Rhai keeps in a call's global state rather
/// than in the scope, so each has to run again before every call:
///
/// - its `import`s, for `alias::f()` to resolve in the callback. An
///   imported module is the resolver's cached one, so its own top level
///   runs once (`VfsModuleResolver`). An import inside a block was scoped
///   to that block and never reached a callback.
/// - a `const NAME = <value>` for each name in `reads` (the script's
///   `global::NAME` reads) that `scope` holds as a constant (#1012), with
///   the value the one top-level run made, so the initializer never runs
///   again. Rhai records a top-level constant for `global::` only as the
///   statement runs. A script with no `global::` read gets none.
///
/// The rest of the top level (`let`s, `log`, `get_entity`, `play_sound`)
/// ran once, into the scope the callbacks are handed.
pub(crate) fn callable(ast: &AST, reads: &[ImmutableString], scope: &Scope) -> AST {
    let mut statements: Vec<Stmt> = ast
        .statements()
        .iter()
        .filter(|s| matches!(s, Stmt::Import(..)))
        .cloned()
        .collect();
    for name in reads {
        // The last declaration of a name is the one the callbacks see
        let constant = scope.iter_raw().filter(|(n, ..)| n == name).last();
        if let Some((_, true, value)) = constant {
            let ident = Ident {
                name: name.clone(),
                pos: Position::NONE,
            };
            let value = Expr::DynamicConstant(Box::new(value.clone()), Position::NONE);
            statements.push(Stmt::Var(
                Box::new((ident, value, None)),
                ASTFlags::CONSTANT,
                Position::NONE,
            ));
        }
    }
    let mut calls = AST::new(statements, ast.shared_lib().clone());
    if let Some(source) = ast.source() {
        calls.set_source(source);
    }
    calls
}

/// The names `ast` reads as `global::NAME`, anywhere: its functions, the
/// closures in them, its top level, a method call's arguments (#998). Each
/// once, in the order first read.
pub(crate) fn global_reads(ast: &AST) -> Vec<ImmutableString> {
    let mut names: Vec<ImmutableString> = Vec::new();
    any_node(ast, |node| {
        if let ASTNode::Expr(Expr::Variable(v, ..)) = node {
            let (_, name, namespace, _) = &**v;
            if namespace.path.len() == 1 && namespace.root() == "global" && !names.contains(name) {
                names.push(name.clone());
            }
        }
        false
    });
    names
}

/// Whether `ast` may call `complete_transition()` (#362): a call to it, or
/// the name in a string (`call("complete_transition")`, `Fn(..)`), anywhere
/// in the script, function bodies and closures included; or an `import`,
/// since a module is resolved only when the script runs and could make the
/// call. A walk of the AST ([`any_node`]), stopping at the first match; once
/// a load or reload, never a frame. `engine::tests` holds it to the shapes
/// above.
pub(crate) fn may_complete_transition(ast: &AST) -> bool {
    const NAME: &str = "complete_transition";
    any_node(ast, |node| match node {
        ASTNode::Stmt(Stmt::FnCall(call, _))
        | ASTNode::Expr(Expr::FnCall(call, _) | Expr::MethodCall(call, _)) => call.name == NAME,
        ASTNode::Expr(Expr::StringConstant(s, _)) => s.as_str() == NAME,
        // What the optimizer folded: `Fn("complete_transition")`, a constant
        // array holding the name
        ASTNode::Expr(Expr::DynamicConstant(value, _)) => names(value, NAME),
        ASTNode::Stmt(Stmt::Import(..)) => true,
        _ => false,
    })
}

/// Whether `value`, a constant the optimizer folded, is `name` as a string
/// or a function pointer, or holds it in an array or a map.
fn names(value: &rhai::Dynamic, name: &str) -> bool {
    if let Some(s) = value.read_lock::<ImmutableString>() {
        return s.as_str() == name;
    }
    if let Some(f) = value.read_lock::<rhai::FnPtr>() {
        return f.fn_name() == name;
    }
    if let Some(items) = value.read_lock::<rhai::Array>() {
        return items.iter().any(|v| names(v, name));
    }
    if let Some(map) = value.read_lock::<rhai::Map>() {
        return map.values().any(|v| names(v, name));
    }
    false
}

/// Whether `ast` spells `name` as a string literal anywhere
/// (`is_action_pressed("jump")`), function bodies and closures included; a
/// comment, a variable or a function of that name doesn't. Once a load.
pub(crate) fn names_string(ast: &AST, name: &str) -> bool {
    any_node(
        ast,
        |node| matches!(node, ASTNode::Expr(Expr::StringConstant(s, _)) if s.as_str() == name),
    )
}

/// Whether `is_it` holds for any node of `ast`: its top level, its
/// functions' bodies and its closures' (Rhai keeps a closure as a function
/// of the AST too). Rhai's walk skips a method call's arguments (`check.rs`
/// says so too), so those are walked here by hand, nested ones included.
/// Stops at the first.
pub(crate) fn any_node(ast: &AST, mut is_it: impl FnMut(&ASTNode) -> bool) -> bool {
    fn hit(node: &ASTNode, is_it: &mut dyn FnMut(&ASTNode) -> bool) -> bool {
        if is_it(node) {
            return true;
        }
        let ASTNode::Expr(Expr::MethodCall(call, _)) = node else {
            return false;
        };
        call.args.iter().any(|arg| {
            let mut found = false;
            arg.walk(&mut Vec::new(), &mut |inner: &[ASTNode]| {
                found = inner.last().is_some_and(|n| hit(n, is_it));
                !found
            });
            found
        })
    }
    let mut found = false;
    ast.walk(&mut |nodes: &[ASTNode]| {
        found = nodes.last().is_some_and(|n| hit(n, &mut is_it));
        !found
    });
    found
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
