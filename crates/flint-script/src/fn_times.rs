//! Time inside a script, by function (#1100).
//!
//! #703 timed each callback (`hud.rhai:on_draw_ui`), which names the file
//! but not the cause: a 12 ms `on_update` could be one loop or one engine
//! call made 400 times. While a callback is timed (`set_timing`, the
//! player's `--perf`) or the recorder records (ADR 0128), every function it
//! calls is counted here: calls, total and worst time, each also a scope
//! in the trace, nested under the callback's.
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
//!   caller's time: callbacks (the callback's own scope), functions with a
//!   `this` type or reading `this` (a wrapper can't pass `this` on),
//!   closures (Rhai 1.24 keeps a closure's definition in the expression
//!   that makes it, never looking it up by name) and an imported module's
//!   functions (the module resolver's, compiled once for every script).
//!
//! Names are interned once a run ([`id_of`]): an engine function as
//! `name()`, a script's as `<source>:<name>()`. Records go to a per-thread sink, which
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

#[derive(Default)]
struct Sink {
    slots: Vec<Slot>,
    /// The ids with a record since [`begin`], in the order first called.
    touched: Vec<FnId>,
    /// Each id's profiler site, made the first time it is traced here.
    sites: Vec<Option<&'static profile::Site>>,
    /// Whether the recorder records: each call is a scope too.
    trace: bool,
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
        let site = self.trace.then(|| {
            if self.sites.len() <= i {
                self.sites.resize(i + 1, None);
            }
            *self.sites[i].get_or_insert_with(|| profile::site_named(text_of(id)))
        });
        (outer, site)
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
    });
    ON.with(|on| on.set(true));
    true
}

/// Stop counting and hand what was counted since [`begin`] to `out`, in
/// the order each function was first called.
pub(crate) fn end(out: &mut Vec<Took>) {
    ON.with(|on| on.set(false));
    SINK.with(|s| {
        let mut s = s.borrow_mut();
        let s = &mut *s;
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

/// A call under way, recorded when it drops.
pub(crate) struct Open {
    id: FnId,
    outer: bool,
    epoch: u32,
    start: web_time::Instant,
    scope: Option<profile::Scope>,
}

/// Start timing a call of `id`, if this thread counts: off, one flag read.
#[inline]
pub(crate) fn enter(id: FnId) -> Option<Open> {
    if !ON.with(Cell::get) {
        return None;
    }
    Some(open(id))
}

#[inline(never)]
fn open(id: FnId) -> Open {
    let (outer, site, epoch) = SINK.with(|s| {
        let mut s = s.borrow_mut();
        let (outer, site) = s.open(id);
        (outer, site, s.epoch)
    });
    // The scope opens before the clock starts and closes after it stops,
    // so the trace's bar holds the time counted
    let scope = site.map(profile::Scope::enter);
    Open {
        id,
        outer,
        epoch,
        start: web_time::Instant::now(),
        scope,
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        let ns = self.start.elapsed().as_nanos() as u64;
        drop(self.scope.take());
        let _ = SINK.try_with(|s| {
            if let Ok(mut s) = s.try_borrow_mut() {
                if s.epoch == self.epoch {
                    s.close(self.id, self.outer, ns);
                }
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

/// `calls` (a script's callable AST, `ScriptInstance::calls`) with each of
/// its functions timed: a wrapper of the same name and parameters that
/// opens a guard and calls the original under [`INNER_PREFIX`]`<id>`. Left
/// as they are: what the module doc lists. `None` when nothing is left to wrap or the wrappers didn't
/// compile; the script is then called on `calls`, untimed inside.
pub(crate) fn profiled(engine: &rhai::Engine, calls: &AST, source: &str) -> Option<AST> {
    use std::fmt::Write;
    let defs: Vec<_> = calls.iter_fn_def().cloned().collect();
    // Which defs get a wrapper: (index, id)
    let mut wrapped: Vec<(usize, FnId)> = Vec::new();
    let mut text = String::new();
    for (i, def) in defs.iter().enumerate() {
        if !wants_wrapper(def) {
            continue;
        }
        let id = id_of(&format!("{source}:{}()", def.name));
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
    if wrapped.is_empty() {
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
    // that makes it, so a wrapper under its name would never be called: its
    // time stays its caller's
    if def.name.starts_with("anon$") {
        return false;
    }
    // A function's Debug form names every node it holds; `ThisPtr` is a
    // read of `this`. Once a script, never a frame.
    !callback && !format!("{:?}", def.body).contains("ThisPtr")
}
