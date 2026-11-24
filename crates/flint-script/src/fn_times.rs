//! Time inside a script, by function (#1100).
//!
//! #703 timed each callback (`hud.rhai:on_draw_ui`), which names the file
//! but not the cause: a 12 ms `on_update` could be one loop or one engine
//! call made 400 times. While a callback is timed (`set_timing`, the
//! player's `--perf`) or the recorder records (ADR 0128), every function it
//! calls is counted here: calls, total and worst time. In the trace a
//! script function's call is a scope, nested under the callback's; an
//! engine function's calls are **summed** (#1288): one record a function in
//! each scope that called it, laid from the first call for their total,
//! with the count and the longest (`profile::record_calls`). A script
//! making 800 engine calls a frame made 800 records a frame, which filled
//! the ring's five seconds in under one. A call that recorded a scope
//! inside it (an engine function with a `scope!` of its own, or one that
//! called back into a script) is a scope of its own instead, so the tree
//! under it stays whole.
//!
//! - **Engine functions** are wrapped where Flint registers them, the
//!   declared API's builder (`api/builder.rs`): every overload of a name is
//!   one record, `raycast()`, so 400 calls show as one row with a count.
//!   Off, the wrapper is one thread-local flag read.
//! - **Script functions** have no hook in Rhai 1.24 short of its debugger,
//!   whose callback runs on every statement and expression of every script
//!   once registered (and its `debugging` feature puts a check on each of
//!   them even when it isn't). So a callback called while profiling runs on
//!   a *profiled* copy of its script's functions ([`profiled`]), made once
//!   a script: each `fn helper(a, b)` becomes a wrapper of the same name and
//!   parameters, compiled by Rhai from text,
//!   `{ let __flint_fn_timer = __flint_fn_enter(17); __flint_fn_17(a, b) }`,
//!   beside the original body renamed `__flint_fn_17`. Rhai drops the
//!   wrapper's guard when it rewinds the wrapper's scope, which it does on a
//!   return, an error and at the end alike, and the drop closes the timing.
//!   The wrapper is compiled, not spliced into the original body, because
//!   Rhai resolves a variable to its slot counted from the end of the scope
//!   at parse time: a `let` added to the top of a body would make every
//!   read of a parameter read the guard. Unprofiled, a script runs on its
//!   own AST, unchanged.
//!
//!   A wrapped function is two call levels deep, so a profiled call runs
//!   with twice Rhai's call-level limit (`ScriptEngine::call`): a recursion
//!   fails at the depth it would untimed. Not wrapped, so inside their
//!   caller's time: callbacks (the callback's own scope) and functions with
//!   a `this` type or reading `this` (a wrapper can't pass `this` on).
//! - **Closures** (#1281): Rhai 1.24 keeps a closure's definition in the
//!   expression that makes it, never looking it up by name, so the profiled
//!   copy rewrites those expressions: each closure a function body makes is
//!   a wrapper curried with the original ([`Closures`]), named by its line,
//!   `<source>:closure at line 12`.
//! - **An imported module's functions** (#1281): a module is evaluated once
//!   and shared by every script, so the resolver keeps a second, profiled
//!   form of it, made by [`profiled`] before `Module::eval_ast_as_new`, and
//!   an import gets that form while this thread [`counting`]
//!   (`VfsModuleResolver`). Named by the import: `menu::open()`.
//!
//! Names are interned once a run ([`id_of`]): an engine function as
//! `name()`, a script's as `<source>:<name>()`, a module's as
//! `<import>::<name>()`. Records go to a per-thread sink, which
//! [`begin`] switches on and [`end`] drains, so engines on other threads
//! (tests) never mix.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use flint_core::profile;
use rhai::AST;

/// A function's place in the name table.
pub(crate) type FnId = u32;

/// The name a profiled wrapper's guard has in the wrapper's scope.
const GUARD: &str = "__flint_fn_timer";
/// The function the wrapper opens its guard with.
pub(crate) const ENTER: &str = "__flint_fn_enter";
/// The original body's name in a profiled script: `__flint_fn_<id>`.
pub(crate) const INNER_PREFIX: &str = "__flint_fn_";

#[derive(Default)]
struct Names {
    list: Vec<&'static str>,
    ids: HashMap<&'static str, FnId>,
}

fn names() -> std::sync::MutexGuard<'static, Names> {
    static NAMES: OnceLock<Mutex<Names>> = OnceLock::new();
    NAMES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// The id of a function named `name`, the same for every engine of the run.
/// A name seen for the first time is kept for the run (bounded by the
/// engine's functions and the scripts' own).
pub(crate) fn id_of(name: &str) -> FnId {
    let mut names = names();
    if let Some(&id) = names.ids.get(name) {
        return id;
    }
    let text: &'static str = Box::leak(name.to_owned().into_boxed_str());
    let id = names.list.len() as FnId;
    names.list.push(text);
    names.ids.insert(text, id);
    id
}

/// The id of the engine function Flint declares as `name`: `name()`.
pub(crate) fn engine_fn(name: &str) -> FnId {
    id_of(&format!("{name}()"))
}

fn text_of(id: FnId) -> &'static str {
    names().list.get(id as usize).copied().unwrap_or("?")
}

/// One function's share of a callback: what [`end`] hands back.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Took {
    /// `raycast()`, `scripts/hud.rhai:draw_bar()`
    pub name: &'static str,
    /// Every call, recursive ones too.
    pub calls: u32,
    /// The outermost calls' time, ms: a recursive call is inside its caller.
    pub total_ms: f32,
    /// The longest outermost call, ms.
    pub worst_ms: f32,
}

#[derive(Default, Clone, Copy)]
struct Slot {
    calls: u32,
    /// Calls of it under way: only the outermost adds its time.
    open: u32,
    total_ns: u64,
    worst_ns: u64,
}

/// An engine function's calls inside one traced scope, folded (#1288): what
/// one [`profile::record_calls`] record says when that scope closes.
#[derive(Clone, Copy)]
struct Summed {
    id: FnId,
    calls: u32,
    /// The first call's start, clock ticks.
    first: u64,
    /// Ticks, summed and the longest.
    total: u64,
    worst: u64,
}

#[derive(Default)]
struct Sink {
    slots: Vec<Slot>,
    /// The ids with a record since [`begin`], in the order first called.
    touched: Vec<FnId>,
    /// Each id's profiler site, made the first time it is traced here.
    sites: Vec<Option<&'static profile::Site>>,
    /// Whether the recorder records: each script function's call is a
    /// scope too, and each engine function's calls one summed record a
    /// scope they were made in.
    trace: bool,
    /// While tracing: the engine calls folded so far in each traced scope
    /// open, the callback's first, then each script function's inside it.
    /// Flushed as records when the scope closes ([`Sink::flush`]).
    summed: Vec<Vec<Summed>>,
    /// Which [`begin`] the records are for: a guard from an earlier one
    /// (left in a scope by an error) closes into nothing.
    epoch: u32,
}

impl Sink {
    fn open(&mut self, id: FnId) -> (bool, Option<&'static profile::Site>) {
        let i = id as usize;
        if self.slots.len() <= i {
            self.slots.resize(i + 1, Slot::default());
        }
        let slot = &mut self.slots[i];
        if slot.calls == 0 && slot.open == 0 {
            self.touched.push(id);
        }
        slot.open += 1;
        let outer = slot.open == 1;
        let site = self.trace.then(|| self.site(id));
        (outer, site)
    }

    fn site(&mut self, id: FnId) -> &'static profile::Site {
        let i = id as usize;
        if self.sites.len() <= i {
            self.sites.resize(i + 1, None);
        }
        *self.sites[i].get_or_insert_with(|| profile::site_named(text_of(id)))
    }

    /// An engine call of `id` that recorded nothing inside it, folded into
    /// the innermost traced scope's sum.
    fn fold(&mut self, id: FnId, start: u64, ticks: u64) {
        let Some(frame) = self.summed.last_mut() else {
            return;
        };
        match frame.iter_mut().find(|s| s.id == id) {
            Some(s) => {
                s.calls += 1;
                s.total += ticks;
                s.worst = s.worst.max(ticks);
            }
            None => frame.push(Summed {
                id,
                calls: 1,
                first: start,
                total: ticks,
                worst: ticks,
            }),
        }
    }

    /// The innermost traced scope is closing: each engine function called
    /// in it is one record, one level inside it (it is still open).
    fn flush(&mut self) {
        let Some(frame) = self.summed.pop() else {
            return;
        };
        for s in frame {
            let site = self.site(s.id);
            profile::record_calls(site, s.first, s.total, s.calls, s.worst);
        }
    }

    fn close(&mut self, id: FnId, outer: bool, ns: u64) {
        let Some(slot) = self.slots.get_mut(id as usize) else {
            return;
        };
        slot.open = slot.open.saturating_sub(1);
        slot.calls += 1;
        if outer {
            slot.total_ns += ns;
            slot.worst_ns = slot.worst_ns.max(ns);
        }
    }
}

thread_local! {
    static ON: Cell<bool> = const { Cell::new(false) };
    static SINK: RefCell<Sink> = RefCell::new(Sink::default());
}

/// Count every function called on this thread from now until [`end`], each
/// a scope too when `trace`. Returns false, and changes nothing, when it is
/// already counting: a callback called from inside another's call is the
/// outer one's.
pub(crate) fn begin(trace: bool) -> bool {
    if ON.with(Cell::get) {
        return false;
    }
    SINK.with(|s| {
        let mut s = s.borrow_mut();
        s.trace = trace && profile::COMPILED_IN;
        s.epoch = s.epoch.wrapping_add(1);
        s.summed.clear();
        if s.trace {
            // The callback's own scope, which the caller holds open
            s.summed.push(Vec::new());
        }
    });
    ON.with(|on| on.set(true));
    true
}

/// Whether this thread is counting: a profiled call is under way, so an
/// `import` it runs is served the module's profiled form
/// (`VfsModuleResolver`, #1281).
#[inline]
pub(crate) fn counting() -> bool {
    ON.with(Cell::get)
}

/// Stop counting and hand what was counted since [`begin`] to `out`, in
/// the order each function was first called. Called while the callback's
/// scope is still open: the engine calls it made are recorded inside it.
pub(crate) fn end(out: &mut Vec<Took>) {
    ON.with(|on| on.set(false));
    SINK.with(|s| {
        let mut s = s.borrow_mut();
        let s = &mut *s;
        // What is still folded: the callback's (and any scope an error
        // left, innermost first, recorded at the callback's level)
        while !s.summed.is_empty() {
            s.flush();
        }
        // A guard still open (a closure kept from a profiled call) closes
        // into nothing from here
        s.epoch = s.epoch.wrapping_add(1);
        for id in s.touched.drain(..) {
            let slot = std::mem::take(&mut s.slots[id as usize]);
            if slot.calls > 0 {
                out.push(Took {
                    name: text_of(id),
                    calls: slot.calls,
                    total_ms: slot.total_ns as f32 / 1e6,
                    worst_ms: slot.worst_ns as f32 / 1e6,
                });
            }
        }
    });
}

/// What a traced call is in the recorder.
enum Traced {
    /// Not tracing.
    No,
    /// A script function's call: a scope, and a sum of the engine calls
    /// made directly inside it.
    Scope(profile::Scope),
    /// An engine function's call: folded into its scope's sum unless
    /// something inside it was recorded (#1288, [`profile::Call`]).
    Call(profile::Call, &'static profile::Site),
}

/// A call under way, recorded when it drops.
pub(crate) struct Open {
    id: FnId,
    outer: bool,
    epoch: u32,
    start: web_time::Instant,
    traced: Traced,
}

/// Start timing a script function's call of `id`, if this thread counts:
/// off, one flag read. Traced, it is a scope.
#[inline]
pub(crate) fn enter(id: FnId) -> Option<Open> {
    if !ON.with(Cell::get) {
        return None;
    }
    Some(open(id, false))
}

/// Start timing an engine function's call of `id` (`api/builder.rs`'s
/// wrappers), if this thread counts: off, one flag read. Traced, its calls
/// are summed: one record a function in each scope that called it, with
/// the count (#1288), where a scope a call was 800 records a frame.
#[inline]
pub(crate) fn enter_engine(id: FnId) -> Option<Open> {
    if !ON.with(Cell::get) {
        return None;
    }
    Some(open(id, true))
}

#[inline(never)]
fn open(id: FnId, engine: bool) -> Open {
    let (outer, site, epoch) = SINK.with(|s| {
        let mut s = s.borrow_mut();
        let (outer, site) = s.open(id);
        if site.is_some() && !engine {
            // The engine calls made in this function are summed under it
            s.summed.push(Vec::new());
        }
        (outer, site, s.epoch)
    });
    // The scope opens before the clock starts and closes after it stops,
    // so the trace's bar holds the time counted
    let traced = match site {
        None => Traced::No,
        Some(site) if engine => Traced::Call(profile::Call::enter(), site),
        Some(site) => Traced::Scope(profile::Scope::enter(site)),
    };
    Open {
        id,
        outer,
        epoch,
        start: web_time::Instant::now(),
        traced,
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        let ns = self.start.elapsed().as_nanos() as u64;
        let _ = SINK.try_with(|s| {
            let Ok(mut s) = s.try_borrow_mut() else {
                return;
            };
            let current = s.epoch == self.epoch;
            match std::mem::replace(&mut self.traced, Traced::No) {
                Traced::No => {}
                Traced::Call(call, site) => {
                    // Closed whatever the epoch, so the depth comes back
                    if let Some((start, ticks)) = call.close(site) {
                        if current {
                            s.fold(self.id, start, ticks);
                        }
                    }
                }
                Traced::Scope(scope) => {
                    // Its engine calls, inside it while it is still open
                    if current {
                        s.flush();
                    }
                    drop(scope);
                }
            }
            if current {
                s.close(self.id, self.outer, ns);
            }
        });
    }
}

/// What a profiled wrapper holds in its scope: its call, closed when Rhai
/// drops it. A copy (Rhai clones a value read from a variable) holds
/// nothing, so only the wrapper's own closes the call.
pub(crate) struct FnGuard(#[allow(dead_code)] Option<Open>);

impl Clone for FnGuard {
    fn clone(&self) -> Self {
        FnGuard(None)
    }
}

/// Register [`ENTER`], what a profiled wrapper calls first. Outside a
/// profiled call (a closure kept from one, called later) it opens nothing.
pub(crate) fn register(engine: &mut rhai::Engine) {
    engine.register_fn(ENTER, |id: rhai::INT| FnGuard(enter(id as FnId)));
}

/// Whether `name` is a profiled script's renamed original body, which an
/// error names as its own call inside the wrapper's.
pub(crate) fn is_inner(name: &str) -> bool {
    name.strip_prefix(INNER_PREFIX)
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
}

/// Whose functions [`profiled`] times, which names their records.
#[derive(Clone, Copy)]
pub(crate) enum Naming<'a> {
    /// A script, by its source: `scripts/hud.rhai:draw_bar()`.
    Script(&'a str),
    /// An imported module, by the path its `import` gave: `menu::open()`
    /// (#1281). Its renamed bodies are private, so the module exports what
    /// it did.
    Module(&'a str),
}

impl<'a> Naming<'a> {
    fn function(self, name: &str) -> String {
        match self {
            Naming::Script(source) => format!("{source}:{name}()"),
            Naming::Module(path) => format!("{path}::{name}()"),
        }
    }

    /// A closure has no name, so its line is its name (#1281): two on one
    /// line are one record.
    fn closure(self, line: Option<usize>) -> String {
        let at = line.map_or_else(|| "?".to_string(), |l| l.to_string());
        match self {
            Naming::Script(source) => format!("{source}:closure at line {at}"),
            Naming::Module(path) => format!("{path}::closure at line {at}"),
        }
    }

    fn source(self) -> &'a str {
        match self {
            Naming::Script(s) | Naming::Module(s) => s,
        }
    }
}

/// `calls` (a script's callable AST, `ScriptInstance::calls`, or a module's
/// whole AST) with each of its functions timed: a wrapper of the same name
/// and parameters that opens a guard and calls the original under
/// [`INNER_PREFIX`]`<id>`; and each closure made in a function body timed
/// ([`Closures`]). Left as they are: what the module doc lists. `None` when
/// nothing is left to time or the wrappers didn't compile; the script is
/// then called on `calls`, untimed inside.
pub(crate) fn profiled(engine: &rhai::Engine, calls: &AST, naming: Naming) -> Option<AST> {
    use std::fmt::Write;
    let source = naming.source();
    // Every closure made in a body becomes a timed one first, so a wrapped
    // function's renamed body and an unwrapped one (a callback) both make
    // timed closures
    let mut closures = Closures::new(engine, calls, naming);
    let defs: Vec<rhai::Shared<rhai::ScriptFuncDef>> = calls
        .iter_fn_def()
        .map(|def| closures.in_def(def))
        .collect();
    // Which defs get a wrapper: (index, id)
    let mut wrapped: Vec<(usize, FnId)> = Vec::new();
    let mut text = String::new();
    for (i, def) in defs.iter().enumerate() {
        if !wants_wrapper(def) {
            continue;
        }
        let id = id_of(&naming.function(&def.name));
        let params = def
            .params
            .iter()
            .map(|p| p.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(
            text,
            "fn __flint_w_{i}({params}) {{ let {GUARD} = {ENTER}({id}); {INNER_PREFIX}{id}({params}) }}"
        );
        wrapped.push((i, id));
    }
    if wrapped.is_empty() && closures.timed == 0 {
        return None;
    }
    let compiled = match engine.compile(&text) {
        Ok(ast) => ast,
        Err(e) => {
            tracing::warn!(target: "script", "{source}: its functions can't be timed: {e}");
            return None;
        }
    };
    let wrappers: HashMap<&str, &rhai::Shared<rhai::ScriptFuncDef>> = compiled
        .iter_fn_def()
        .map(|d| (d.name.as_str(), d))
        .collect();
    let mut lib = rhai::Module::new();
    let mut next = wrapped.iter().peekable();
    for (i, def) in defs.iter().enumerate() {
        let Some(&&(_, id)) = next.peek().filter(|(w, _)| *w == i) else {
            lib.set_script_fn(def.clone());
            continue;
        };
        next.next();
        let wrapper = wrappers.get(format!("__flint_w_{i}").as_str())?;
        let mut inner = (**def).clone();
        inner.name = format!("{INNER_PREFIX}{id}").into();
        if matches!(naming, Naming::Module(_)) {
            // Callable from the module's own functions (its environment
            // holds the whole lib), never exported as `menu::__flint_fn_9`
            inner.access = rhai::FnAccess::Private;
        }
        lib.set_script_fn(inner);
        let mut outer = (***wrapper).clone();
        outer.name = def.name.clone();
        outer.access = def.access;
        #[cfg(feature = "metadata")]
        {
            outer.comments = def.comments.clone();
        }
        lib.set_script_fn(outer);
    }
    let mut ast = AST::new(calls.statements().iter().cloned(), lib);
    if let Some(source) = calls.source() {
        ast.set_source(source);
    }
    Some(ast)
}

/// Whether a script function gets a timed wrapper (see [`profiled`]).
fn wants_wrapper(def: &rhai::ScriptFuncDef) -> bool {
    if def.this_type.is_some() {
        return false;
    }
    let callback = flint_core::callbacks::CALLBACKS
        .iter()
        .any(|c| c.name == def.name.as_str());
    // A closure (`anon$<hash>`) carries its definition in the expression
    // that makes it, so a wrapper under its name would never be called:
    // [`Closures`] times it where it is made
    if is_closure(def) {
        return false;
    }
    !callback && !reads_this(def)
}

/// Whether `def` is a closure's (`|x| ..`), which Rhai names `anon$<hash>`.
fn is_closure(def: &rhai::ScriptFuncDef) -> bool {
    def.name.starts_with("anon$")
}

/// Whether `def` reads `this`, which a wrapper can't pass on. A function's
/// Debug form names every node it holds; `ThisPtr` is a read of `this`.
/// Once a script, never a frame.
fn reads_this(def: &rhai::ScriptFuncDef) -> bool {
    format!("{:?}", def.body).contains("ThisPtr")
}

/// Times the closures a script's function bodies make (#1281). Rhai 1.24
/// keeps a closure's definition in the expression that makes it, a
/// constant function pointer (`FnPtr` to the definition itself, never
/// looked up by name), curried with what it captures. Each such constant
/// becomes a constant pointer to a wrapper, curried with the original:
///
/// `|v| v + sum` is `fn __flint_fn_<id>(__flint_f, sum, v) {
///     let __flint_fn_timer = __flint_fn_enter(<id>); call(__flint_f, sum, v) }`
///
/// with `__flint_f` the original closure, so the value is still one
/// self-contained pointer: kept in a variable and called after profiling
/// ends, it runs (its guard then opens nothing). The wrapper takes every
/// parameter, the captured ones included (they are curried after it, as
/// they were onto the original), and hands them on through `call(f, ..)`'s
/// function form, which, unlike a method call's arguments, keeps a
/// captured variable's shared value shared: the closure's writes to it
/// still reach the caller's variable. A closure inside a closure is timed
/// too (its maker's body is rewritten first). Left as they are: a closure
/// reading `this`, one in the top level (run once, untimed) and one Rhai
/// folded into a constant array or map.
struct Closures<'e> {
    engine: &'e rhai::Engine,
    naming: Naming<'e>,
    /// The closures' definitions by name, from the AST's lib (Rhai keeps a
    /// copy there, under the pointer's name)
    defs: HashMap<rhai::ImmutableString, rhai::Shared<rhai::ScriptFuncDef>>,
    /// Each closure's definition with its own body's closures timed, made
    /// once a closure however many places make it
    timed_defs: HashMap<rhai::ImmutableString, rhai::Shared<rhai::ScriptFuncDef>>,
    /// Closures made timed: none, and [`profiled`] may have nothing to do
    timed: usize,
}

impl<'e> Closures<'e> {
    fn new(engine: &'e rhai::Engine, ast: &AST, naming: Naming<'e>) -> Self {
        let defs = ast
            .iter_fn_def()
            .filter(|d| is_closure(d))
            .map(|d| (d.name.clone(), d.clone()))
            .collect();
        Self {
            engine,
            naming,
            defs,
            timed_defs: HashMap::new(),
            timed: 0,
        }
    }

    /// `def` with every closure its body makes timed: itself when it makes
    /// none (or is a closure's, reached only where it is made).
    fn in_def(
        &mut self,
        def: &rhai::Shared<rhai::ScriptFuncDef>,
    ) -> rhai::Shared<rhai::ScriptFuncDef> {
        if is_closure(def) || self.defs.is_empty() {
            return def.clone();
        }
        let before = self.timed;
        let mut copy = (**def).clone();
        self.block(&mut copy.body);
        if self.timed == before {
            def.clone()
        } else {
            rhai::Shared::new(copy)
        }
    }

    fn block(&mut self, block: &mut rhai::StmtBlock) {
        for stmt in block.statements_mut().iter_mut() {
            self.stmt(stmt);
        }
    }

    /// Every expression `stmt` holds, as `Stmt::walk` reaches them.
    fn stmt(&mut self, stmt: &mut rhai::Stmt) {
        use rhai::Stmt;
        match stmt {
            Stmt::Var(x, ..) => self.expr(&mut x.1),
            Stmt::If(x, ..) | Stmt::While(x, ..) | Stmt::Do(x, ..) | Stmt::TryCatch(x, ..) => {
                self.expr(&mut x.expr);
                self.block(&mut x.body);
                self.block(&mut x.branch);
            }
            Stmt::Switch(x, ..) => {
                let (expr, cases) = &mut **x;
                self.expr(expr);
                for case in cases.expressions.iter_mut() {
                    self.expr(&mut case.lhs);
                    self.expr(&mut case.rhs);
                }
            }
            Stmt::For(x, ..) => {
                self.expr(&mut x.2.expr);
                self.block(&mut x.2.body);
                self.block(&mut x.2.branch);
            }
            Stmt::Assignment(x) => {
                self.expr(&mut x.1.lhs);
                self.expr(&mut x.1.rhs);
            }
            Stmt::FnCall(call, ..) => {
                for arg in call.args.iter_mut() {
                    self.expr(arg);
                }
            }
            Stmt::Block(block) => self.block(block),
            Stmt::Expr(e) => self.expr(e),
            Stmt::BreakLoop(Some(e), ..) | Stmt::Return(Some(e), ..) => self.expr(e),
            Stmt::Import(x, ..) => self.expr(&mut x.0),
            // Noop, Export, Share, a bare break or return: no expression
            _ => {}
        }
    }

    /// Every expression inside `expr`, a method call's arguments too (which
    /// `Expr::walk` skips, #998), each closure made timed.
    fn expr(&mut self, expr: &mut rhai::Expr) {
        use rhai::Expr;
        match expr {
            Expr::DynamicConstant(value, pos) => {
                if let Some(timed) = self.timed_closure(value, *pos) {
                    **value = timed;
                }
            }
            Expr::InterpolatedString(items, ..) | Expr::Array(items, ..) => {
                for e in items.iter_mut() {
                    self.expr(e);
                }
            }
            Expr::Map(map, ..) => {
                for (_, e) in map.0.iter_mut() {
                    self.expr(e);
                }
            }
            Expr::Index(x, ..) | Expr::Dot(x, ..) => {
                self.expr(&mut x.lhs);
                self.expr(&mut x.rhs);
            }
            Expr::And(items, ..) | Expr::Or(items, ..) | Expr::Coalesce(items, ..) => {
                for e in items.iter_mut() {
                    self.expr(e);
                }
            }
            Expr::FnCall(call, ..) | Expr::MethodCall(call, ..) => {
                for arg in call.args.iter_mut() {
                    self.expr(arg);
                }
            }
            Expr::Stmt(block) => self.block(block),
            // Constants, variables, properties, `this`: no expression. No
            // `Expr::Custom` either: Flint registers no custom syntax, and
            // the web build's Rhai has none (`no_custom_syntax`)
            _ => {}
        }
    }

    /// The timed form of a constant that is a closure: a pointer to its
    /// wrapper, curried with the closure (see [`Closures`]). `None` for any
    /// other constant, and for a closure left untimed.
    fn timed_closure(
        &mut self,
        value: &rhai::Dynamic,
        pos: rhai::Position,
    ) -> Option<rhai::Dynamic> {
        use std::fmt::Write;
        if !value.is_fnptr() {
            return None;
        }
        let ptr = value.clone().try_cast::<rhai::FnPtr>()?;
        if !ptr.is_anonymous() {
            return None;
        }
        let def = self.defs.get(ptr.fn_name())?.clone();
        if reads_this(&def) {
            return None;
        }
        // Its own body's closures first, once a definition
        let inner = match self.timed_defs.get(&def.name) {
            Some(d) => d.clone(),
            None => {
                let mut copy = (*def).clone();
                self.block(&mut copy.body);
                let d = rhai::Shared::new(copy);
                self.timed_defs.insert(def.name.clone(), d.clone());
                d
            }
        };
        let mut original = rhai::FnPtr::from(inner);
        for v in ptr.iter_curry() {
            original.add_curry(v.clone());
        }
        let id = id_of(&self.naming.closure(pos.line()));
        let mut text = format!("fn {INNER_PREFIX}{id}(__flint_f");
        let mut args = String::from("__flint_f");
        for p in def.params.iter() {
            let _ = write!(text, ", {p}");
            let _ = write!(args, ", {p}");
        }
        let _ = write!(text, ") {{ let {GUARD} = {ENTER}({id}); call({args}) }}");
        let compiled = match self.engine.compile(&text) {
            Ok(ast) => ast,
            Err(e) => {
                tracing::warn!(
                    target: "script",
                    "{}: a closure at line {:?} can't be timed: {e}",
                    self.naming.source(),
                    pos.line()
                );
                return None;
            }
        };
        let wrapper = compiled.iter_fn_def().next()?.clone();
        let mut timed = rhai::FnPtr::from(wrapper);
        timed.add_curry(rhai::Dynamic::from(original));
        self.timed += 1;
        Some(rhai::Dynamic::from(timed))
    }
}
