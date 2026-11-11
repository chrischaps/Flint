//! The declared script API (ADR 0116): one builder that registers a Rhai
//! function and records what it registered.
//!
//! ```ignore
//! let mut m = api.module("entity");
//! m.func("set_position", "Move an entity to a position in its parent's space.")
//!     .arg("id", Entity)
//!     .arg("x", Num)
//!     .arg("y", Num)
//!     .arg("z", Num)
//!     .run(|call, (id, x, y, z)| {
//!         let world = call.world_mut()?;
//!         // ...
//!         Ok(())
//!     });
//! ```
//!
//! Each `.arg(name, kind)` says what the parameter accepts; the kind does
//! the conversion once, before the body runs, so the body gets Rust values
//! (`EntityId`, `f64`, `ImmutableString`, ...). An argument that doesn't
//! convert is a script error naming the function, the parameter and the
//! kind. An `Entity` argument that names no live entity (`-1`, or a
//! despawned id) skips the body and returns the function's fallback (its
//! return type's empty value, or `.or_return(v)`), warning once per script
//! and function (#635, #701).
//!
//! `.run` takes the arguments as `Dynamic` and converts them, so an integer
//! works where a number is wanted. `.run_typed` registers with Rhai's typed
//! fast path instead (a `Num` parameter is then a float only) and still
//! records the entry: the escape hatch for hot functions.
//!
//! The body reaches the world only through [`Call`], which returns a script
//! error when no world is in scope. No `unsafe` outside `context.rs`.

use super::registry::ApiRegistry;
#[cfg(not(target_arch = "wasm32"))]
use super::registry::{ApiEntry, ApiParam};
use crate::context::ScriptCallContext;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_physics::PhysicsSystem;
use rhai::{
    Array, Dynamic, Engine, EvalAltResult, FnPtr, ImmutableString, Map, NativeCallContext, Position,
};
use std::any::TypeId;
use std::sync::{Arc, Mutex};

/// A script error, as a registered function returns it.
pub type CallError = Box<EvalAltResult>;
/// What a function body returns when it can fail.
pub type CallResult<T> = Result<T, CallError>;

type RhaiResult = Result<Dynamic, CallError>;

/// Where functions are declared: the engine, the call context their
/// bodies reach the world through, and the registry the declarations go
/// into. `ScriptEngine::new` builds one for Flint's API, and
/// `ScriptSystem::script_api` hands one to a host or game extension.
pub struct ScriptApi<'a> {
    engine: &'a mut Engine,
    ctx: Arc<Mutex<ScriptCallContext>>,
    registry: &'a mut ApiRegistry,
}

impl<'a> ScriptApi<'a> {
    pub fn new(
        engine: &'a mut Engine,
        ctx: Arc<Mutex<ScriptCallContext>>,
        registry: &'a mut ApiRegistry,
    ) -> Self {
        Self {
            engine,
            ctx,
            registry,
        }
    }

    /// Declare functions in a module: `entity`, `ui`, or a game's own.
    pub fn module(&mut self, name: &'static str) -> Module<'_> {
        Module {
            engine: self.engine,
            ctx: &self.ctx,
            registry: self.registry,
            name,
        }
    }

    /// The Rhai engine, for registrations not yet declared through the
    /// builder (ADR 0116 step 2 ports them). They are listed as undeclared.
    pub fn engine_mut(&mut self) -> &mut Engine {
        self.engine
    }

    /// The call context, for registrations not yet declared.
    pub fn ctx(&self) -> &Arc<Mutex<ScriptCallContext>> {
        &self.ctx
    }
}

/// One module's declarations; see [`ScriptApi::module`].
pub struct Module<'m> {
    engine: &'m mut Engine,
    ctx: &'m Arc<Mutex<ScriptCallContext>>,
    registry: &'m mut ApiRegistry,
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    name: &'static str,
}

impl Module<'_> {
    /// Start declaring `name`, with a one-line `doc` (a test fails on an
    /// empty one). On the web the doc is dropped: the registry is
    /// native-only.
    #[cfg_attr(target_arch = "wasm32", inline(always))]
    pub fn func(&mut self, name: &'static str, doc: &'static str) -> Func<'_, ()> {
        #[cfg(target_arch = "wasm32")]
        let _ = doc;
        Func {
            engine: self.engine,
            ctx: self.ctx,
            registry: self.registry,
            name,
            params: Vec::new(),
            fallback: None,
            preferred_signature: true,
            #[cfg(not(target_arch = "wasm32"))]
            entry: ApiEntry {
                module: self.name,
                name,
                params: Vec::new(),
                returns: None,
                doc,
                typed: false,
            },
            _kinds: std::marker::PhantomData,
        }
    }
}

/// A function being declared; finish it with [`Func::run`] or
/// [`Func::run_typed`].
pub struct Func<'f, K> {
    engine: &'f mut Engine,
    ctx: &'f Arc<Mutex<ScriptCallContext>>,
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    registry: &'f mut ApiRegistry,
    name: &'static str,
    params: Vec<&'static str>,
    fallback: Option<Dynamic>,
    preferred_signature: bool,
    #[cfg(not(target_arch = "wasm32"))]
    entry: ApiEntry,
    _kinds: std::marker::PhantomData<K>,
}

impl<'f, K: Kinds> Func<'f, K> {
    /// The next parameter: its name (in errors and the listing) and kind.
    pub fn arg<N: Kind>(self, name: &'static str, _kind: N) -> Func<'f, <K as Push<N>>::Out>
    where
        K: Push<N>,
    {
        let mut params = self.params;
        params.push(name);
        Func {
            engine: self.engine,
            ctx: self.ctx,
            registry: self.registry,
            name: self.name,
            params,
            fallback: self.fallback,
            preferred_signature: self.preferred_signature,
            #[cfg(not(target_arch = "wasm32"))]
            entry: self.entry,
            _kinds: std::marker::PhantomData,
        }
    }

    /// What it returns, for the listing: a kind and the shape in words
    /// (the keys of a map, what it returns on failure).
    #[cfg_attr(target_arch = "wasm32", inline(always))]
    #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
    pub fn returns<R: Kind>(mut self, _kind: R, note: &'static str) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.entry.returns = Some((R::NAME, note));
        }
        #[cfg(target_arch = "wasm32")]
        let _ = note;
        self
    }

    /// What it returns when an `Entity` argument names no entity, in place
    /// of its return type's empty value (`-1`, `f64::MAX`, ...).
    pub fn or_return<T: Clone + Send + Sync + 'static>(mut self, value: T) -> Self {
        self.fallback = Some(Dynamic::from(value));
        self
    }

    /// Register only the all-`Dynamic` signature, without the exact-typed
    /// twin that lets Rhai find the function on its first lookup. For the
    /// overhead measurement (ADR 0116); nothing else wants it.
    #[doc(hidden)]
    pub fn without_preferred_signature(mut self) -> Self {
        self.preferred_signature = false;
        self
    }

    /// Register it, taking each argument as a `Dynamic` and converting it
    /// by its kind. The body gets the converted arguments as a tuple.
    pub fn run<F, R>(self, f: F)
    where
        F: Fn(&mut Call<'_>, K::Out) -> R + Send + Sync + 'static,
        R: ScriptReturn,
    {
        let fallback = self.fallback.clone().unwrap_or_else(R::fallback);
        let params: Arc<[&'static str]> = self.params.clone().into();
        let name = self.name;
        let ctx = self.ctx.clone();
        let body = Arc::new(
            move |nctx: NativeCallContext, args: &mut [&mut Dynamic]| -> RhaiResult {
                let mut guard = crate::lock_or_recover(&ctx);
                let mut call = Call::new(&mut guard, name, nctx.call_position());
                match K::convert(&mut call, args, &params) {
                    Ok(out) => f(&mut call, out).into_result(),
                    Err(ArgFail::Skip) => Ok(fallback.clone()),
                    Err(ArgFail::Error(e)) => Err(e),
                }
            },
        );
        // Rhai looks a call up by its arguments' exact types first, then
        // tries every mix of `Dynamic` in their place (2^n lookups for n
        // parameters). The exact twin (floats for `Num`, ints for `Entity`)
        // is found on the first lookup, so the usual call pays nothing for
        // the conversions it doesn't need.
        let dynamic = vec![TypeId::of::<Dynamic>(); K::LEN];
        let preferred = K::preferred_types();
        if self.preferred_signature && preferred != dynamic {
            let body = body.clone();
            self.engine
                .register_raw_fn(name, preferred, move |nctx, args| body(nctx, args));
        }
        self.engine
            .register_raw_fn(name, dynamic, move |nctx, args| body(nctx, args));
        self.record(false);
    }

    /// Register it on Rhai's typed fast path: each parameter is the kind's
    /// exact Rust type (`Num` is a float only, `Entity` an int), so nothing
    /// is converted and an integer where a float is wanted is "function not
    /// found", as before the builder. Recorded like [`Func::run`], with
    /// those stricter kinds. For hot functions (ADR 0116, Consequences).
    pub fn run_typed<F, R>(self, f: F)
    where
        K: TypedKinds,
        F: Fn(&mut Call<'_>, K::Out) -> R + Send + Sync + 'static,
        R: ScriptReturn,
    {
        let fallback = self.fallback.clone().unwrap_or_else(R::fallback);
        let params: Arc<[&'static str]> = self.params.clone().into();
        K::register_typed(
            self.engine,
            self.name,
            self.ctx.clone(),
            params,
            fallback,
            f,
        );
        self.record(true);
    }

    #[cfg_attr(target_arch = "wasm32", allow(unused_variables, unused_mut))]
    fn record(mut self, typed: bool) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let kinds = K::kind_names(typed);
            self.entry.params = self
                .params
                .iter()
                .zip(kinds)
                .map(|(name, kind)| ApiParam { name, kind })
                .collect();
            self.entry.typed = typed;
            self.registry.push(self.entry);
        }
    }
}

// ─── The call handle ─────────────────────────────────────

/// What a function body sees: the call context, locked for the call, and
/// where in the script it was called from.
pub struct Call<'c> {
    cx: &'c mut ScriptCallContext,
    func: &'static str,
    pos: Position,
}

impl<'c> Call<'c> {
    pub(crate) fn new(cx: &'c mut ScriptCallContext, func: &'static str, pos: Position) -> Self {
        Self { cx, func, pos }
    }

    /// The function being called, as the script named it.
    pub fn name(&self) -> &'static str {
        self.func
    }

    /// Where the script called it.
    pub fn position(&self) -> Position {
        self.pos
    }

    /// The rest of the call context: the current entity, the command and
    /// draw queues, the input snapshot.
    pub fn ctx(&mut self) -> &mut ScriptCallContext {
        self.cx
    }

    /// The call context, read-only.
    pub fn ctx_ref(&self) -> &ScriptCallContext {
        self.cx
    }

    /// The world, or a script error when there is none in scope (a
    /// script's top level runs once at load, before there is one).
    pub fn world(&self) -> CallResult<&FlintWorld> {
        let func = self.func;
        let pos = self.pos;
        self.cx.try_world().ok_or_else(|| no_world(func, pos))
    }

    /// [`Call::world`], mutably.
    pub fn world_mut(&mut self) -> CallResult<&mut FlintWorld> {
        let func = self.func;
        let pos = self.pos;
        self.cx.try_world_mut().ok_or_else(|| no_world(func, pos))
    }

    /// The physics system, when the host has one.
    pub fn physics(&self) -> Option<&PhysicsSystem> {
        self.cx.try_physics()
    }

    /// Resolve an id the script handed over inside a map or array as an
    /// `Entity` argument would be: `None`, having warned once, when no
    /// entity has it.
    pub fn entity(&mut self, id: i64) -> Option<EntityId> {
        self.cx.entity_target(self.func, id, self.pos.line())
    }

    /// A script error from this function, at the call's position.
    pub fn error(&self, message: impl std::fmt::Display) -> CallError {
        Box::new(EvalAltResult::ErrorRuntime(
            format!("{}: {message}", self.func).into(),
            self.pos,
        ))
    }
}

fn no_world(func: &'static str, pos: Position) -> CallError {
    Box::new(EvalAltResult::ErrorRuntime(
        format!(
            "{func}: there is no world here. A script's top level runs once when it \
             loads, before the scene's world exists; call {func} from a callback \
             such as on_init"
        )
        .into(),
        pos,
    ))
}

// ─── Argument kinds ──────────────────────────────────────

/// Why an argument didn't become its kind's value.
pub enum Conv {
    /// The value is of the wrong type for the kind.
    Mismatch,
    /// An `Entity` names no live entity: skip the body, return the fallback.
    Skip,
}

/// Why a call's arguments didn't convert.
pub enum ArgFail {
    Skip,
    Error(CallError),
}

/// What a parameter accepts and becomes. The kinds are unit structs passed
/// to [`Func::arg`]: [`Num`], [`Int`], [`Bool`], [`Entity`],
/// [`MaybeEntity`], [`Str`], [`Map`](MapKind), [`Array`](ArrayKind),
/// [`Color`], [`Any`], [`FnPtr`](FnKind).
pub trait Kind: Copy + Send + Sync + 'static {
    /// What the body gets.
    type Out;
    /// The exact type Rhai passes on the typed path, and the preferred
    /// signature's type on the converting one.
    type Typed: Clone + Send + Sync + 'static;
    /// The kind's name in the listing.
    const NAME: &'static str;
    /// Its name on the typed path, where it converts nothing.
    const TYPED_NAME: &'static str;
    /// What an argument of it must be, for the error.
    const WANTS: &'static str;
    /// Convert a script value; it may be taken (left as `()`).
    fn convert(call: &mut Call<'_>, v: &mut Dynamic) -> Result<Self::Out, Conv>;
    /// The value from the typed path.
    fn from_typed(call: &mut Call<'_>, v: Self::Typed) -> Result<Self::Out, Conv>;
}

/// A number: an int or a float, as `f64`.
#[derive(Clone, Copy)]
pub struct Num;
/// An integer, as `i64`; a float with no fractional part is accepted.
#[derive(Clone, Copy)]
pub struct Int;
/// `true` or `false`.
#[derive(Clone, Copy)]
pub struct Bool;
/// An entity id that must name a live entity, as `EntityId`. `-1` or a
/// gone id skips the body, returning the fallback, and warns once per
/// script and function.
#[derive(Clone, Copy)]
pub struct Entity;
/// An entity id that may name nothing, as `Option<EntityId>`, without a
/// warning: for the functions that ask whether it does.
#[derive(Clone, Copy)]
pub struct MaybeEntity;
/// A string, as `ImmutableString`.
#[derive(Clone, Copy)]
pub struct Str;
/// An object map, as `rhai::Map`.
#[derive(Clone, Copy)]
pub struct MapKind;
/// An array, as `rhai::Array`.
#[derive(Clone, Copy)]
pub struct ArrayKind;
/// A colour: `[r, g, b]`, `[r, g, b, a]` (0..1) or `"#rrggbb"` /
/// `"#rrggbbaa"`, as `[f32; 4]`.
#[derive(Clone, Copy)]
pub struct Color;
/// Anything, as `Dynamic`.
#[derive(Clone, Copy)]
pub struct Any;
/// A function pointer (`Fn("name")` or a closure), as `rhai::FnPtr`.
#[derive(Clone, Copy)]
pub struct FnKind;

/// The kinds under their script names, for `.arg("m", kind::Map)`.
pub mod kind {
    pub use super::{
        Any, ArrayKind as Array, Bool, Color, Entity, FnKind as Fn, Int, MapKind as Map,
        MaybeEntity, Num, Str,
    };
}

fn int_like(v: &Dynamic) -> Option<i64> {
    if let Ok(i) = v.as_int() {
        return Some(i);
    }
    let f = v.as_float().ok()?;
    (f.fract() == 0.0 && f.abs() < 9.0e15).then_some(f as i64)
}

impl Kind for Num {
    type Out = f64;
    type Typed = f64;
    const NAME: &'static str = "num";
    const TYPED_NAME: &'static str = "float";
    const WANTS: &'static str = "a number (an int or a float)";
    #[inline]
    fn convert(_: &mut Call<'_>, v: &mut Dynamic) -> Result<f64, Conv> {
        match v.as_float() {
            Ok(f) => Ok(f),
            Err(_) => v.as_int().map(|i| i as f64).map_err(|_| Conv::Mismatch),
        }
    }
    #[inline]
    fn from_typed(_: &mut Call<'_>, v: f64) -> Result<f64, Conv> {
        Ok(v)
    }
}

impl Kind for Int {
    type Out = i64;
    type Typed = i64;
    const NAME: &'static str = "int";
    const TYPED_NAME: &'static str = "int";
    const WANTS: &'static str = "an integer (or a float with nothing after the point)";
    #[inline]
    fn convert(_: &mut Call<'_>, v: &mut Dynamic) -> Result<i64, Conv> {
        int_like(v).ok_or(Conv::Mismatch)
    }
    #[inline]
    fn from_typed(_: &mut Call<'_>, v: i64) -> Result<i64, Conv> {
        Ok(v)
    }
}

impl Kind for Bool {
    type Out = bool;
    type Typed = bool;
    const NAME: &'static str = "bool";
    const TYPED_NAME: &'static str = "bool";
    const WANTS: &'static str = "true or false";
    #[inline]
    fn convert(_: &mut Call<'_>, v: &mut Dynamic) -> Result<bool, Conv> {
        v.as_bool().map_err(|_| Conv::Mismatch)
    }
    #[inline]
    fn from_typed(_: &mut Call<'_>, v: bool) -> Result<bool, Conv> {
        Ok(v)
    }
}

impl Kind for Entity {
    type Out = EntityId;
    type Typed = i64;
    const NAME: &'static str = "entity";
    const TYPED_NAME: &'static str = "entity";
    const WANTS: &'static str = "an entity id (an int, as get_entity returns)";
    #[inline]
    fn convert(call: &mut Call<'_>, v: &mut Dynamic) -> Result<EntityId, Conv> {
        let id = int_like(v).ok_or(Conv::Mismatch)?;
        Self::from_typed(call, id)
    }
    #[inline]
    fn from_typed(call: &mut Call<'_>, id: i64) -> Result<EntityId, Conv> {
        call.entity(id).ok_or(Conv::Skip)
    }
}

impl Kind for MaybeEntity {
    type Out = Option<EntityId>;
    type Typed = i64;
    const NAME: &'static str = "entity?";
    const TYPED_NAME: &'static str = "entity?";
    const WANTS: &'static str = "an entity id (an int, as get_entity returns)";
    #[inline]
    fn convert(call: &mut Call<'_>, v: &mut Dynamic) -> Result<Option<EntityId>, Conv> {
        let id = int_like(v).ok_or(Conv::Mismatch)?;
        Self::from_typed(call, id)
    }
    #[inline]
    fn from_typed(call: &mut Call<'_>, id: i64) -> Result<Option<EntityId>, Conv> {
        let eid = EntityId::from_raw(id.max(0) as u64);
        let live = id >= 0 && call.cx.try_world().is_some_and(|w| w.contains(eid));
        Ok(live.then_some(eid))
    }
}

impl Kind for Str {
    type Out = ImmutableString;
    type Typed = ImmutableString;
    const NAME: &'static str = "str";
    const TYPED_NAME: &'static str = "str";
    const WANTS: &'static str = "a string";
    #[inline]
    fn convert(_: &mut Call<'_>, v: &mut Dynamic) -> Result<ImmutableString, Conv> {
        if !v.is_string() {
            return Err(Conv::Mismatch);
        }
        std::mem::take(v)
            .into_immutable_string()
            .map_err(|_| Conv::Mismatch)
    }
    #[inline]
    fn from_typed(_: &mut Call<'_>, v: ImmutableString) -> Result<ImmutableString, Conv> {
        Ok(v)
    }
}

impl Kind for MapKind {
    type Out = Map;
    type Typed = Map;
    const NAME: &'static str = "map";
    const TYPED_NAME: &'static str = "map";
    const WANTS: &'static str = "a map (#{ ... })";
    #[inline]
    fn convert(_: &mut Call<'_>, v: &mut Dynamic) -> Result<Map, Conv> {
        if !v.is_map() {
            return Err(Conv::Mismatch);
        }
        std::mem::take(v).try_cast::<Map>().ok_or(Conv::Mismatch)
    }
    #[inline]
    fn from_typed(_: &mut Call<'_>, v: Map) -> Result<Map, Conv> {
        Ok(v)
    }
}

impl Kind for ArrayKind {
    type Out = Array;
    type Typed = Array;
    const NAME: &'static str = "array";
    const TYPED_NAME: &'static str = "array";
    const WANTS: &'static str = "an array ([ ... ])";
    #[inline]
    fn convert(_: &mut Call<'_>, v: &mut Dynamic) -> Result<Array, Conv> {
        if !v.is_array() {
            return Err(Conv::Mismatch);
        }
        std::mem::take(v).try_cast::<Array>().ok_or(Conv::Mismatch)
    }
    #[inline]
    fn from_typed(_: &mut Call<'_>, v: Array) -> Result<Array, Conv> {
        Ok(v)
    }
}

/// `[r, g, b]` or `[r, g, b, a]` of numbers, alpha 1 when left out.
fn color_from_array(a: &Array) -> Option<[f32; 4]> {
    if a.len() != 3 && a.len() != 4 {
        return None;
    }
    let mut out = [1.0f32; 4];
    for (slot, v) in out.iter_mut().zip(a) {
        *slot = match v.as_float() {
            Ok(f) => f as f32,
            Err(_) => v.as_int().ok()? as f32,
        };
    }
    Some(out)
}

/// `"#rrggbb"` or `"#rrggbbaa"`, each channel 0..255 as 0..1.
pub fn color_from_hex(s: &str) -> Option<[f32; 4]> {
    let hex = s.strip_prefix('#')?;
    if !(hex.len() == 6 || hex.len() == 8) || !hex.is_ascii() {
        return None;
    }
    let mut out = [1.0f32; 4];
    for (i, slot) in out.iter_mut().enumerate().take(hex.len() / 2) {
        let byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
        *slot = byte as f32 / 255.0;
    }
    Some(out)
}

impl Kind for Color {
    type Out = [f32; 4];
    type Typed = Array;
    const NAME: &'static str = "color";
    const TYPED_NAME: &'static str = "color array";
    const WANTS: &'static str = "a colour ([r, g, b], [r, g, b, a] or \"#rrggbb\")";
    fn convert(_: &mut Call<'_>, v: &mut Dynamic) -> Result<[f32; 4], Conv> {
        if let Some(a) = v.read_lock::<Array>() {
            return color_from_array(&a).ok_or(Conv::Mismatch);
        }
        if let Some(s) = v.read_lock::<ImmutableString>() {
            return color_from_hex(&s).ok_or(Conv::Mismatch);
        }
        Err(Conv::Mismatch)
    }
    fn from_typed(_: &mut Call<'_>, v: Array) -> Result<[f32; 4], Conv> {
        color_from_array(&v).ok_or(Conv::Mismatch)
    }
}

impl Kind for Any {
    type Out = Dynamic;
    type Typed = Dynamic;
    const NAME: &'static str = "any";
    const TYPED_NAME: &'static str = "any";
    const WANTS: &'static str = "any value";
    #[inline]
    fn convert(_: &mut Call<'_>, v: &mut Dynamic) -> Result<Dynamic, Conv> {
        Ok(std::mem::take(v))
    }
    #[inline]
    fn from_typed(_: &mut Call<'_>, v: Dynamic) -> Result<Dynamic, Conv> {
        Ok(v)
    }
}

impl Kind for FnKind {
    type Out = FnPtr;
    type Typed = FnPtr;
    const NAME: &'static str = "fn";
    const TYPED_NAME: &'static str = "fn";
    const WANTS: &'static str = "a function pointer (Fn(\"name\") or a closure)";
    #[inline]
    fn convert(_: &mut Call<'_>, v: &mut Dynamic) -> Result<FnPtr, Conv> {
        if !v.is_fnptr() {
            return Err(Conv::Mismatch);
        }
        std::mem::take(v).try_cast::<FnPtr>().ok_or(Conv::Mismatch)
    }
    #[inline]
    fn from_typed(_: &mut Call<'_>, v: FnPtr) -> Result<FnPtr, Conv> {
        Ok(v)
    }
}

fn arg_error(call: &Call<'_>, param: &str, wants: &str, got: &str) -> CallError {
    call.error(format_args!(
        "argument `{param}` must be {wants}, not {got}"
    ))
}

#[inline]
fn convert_one<K: Kind>(
    call: &mut Call<'_>,
    v: &mut Dynamic,
    param: &'static str,
) -> Result<K::Out, ArgFail> {
    match K::convert(call, v) {
        Ok(out) => Ok(out),
        Err(Conv::Skip) => Err(ArgFail::Skip),
        Err(Conv::Mismatch) => Err(ArgFail::Error(arg_error(
            call,
            param,
            K::WANTS,
            v.type_name(),
        ))),
    }
}

// ─── Return values ───────────────────────────────────────

/// What a body may return: a script value, or a `CallResult` of one.
pub trait ScriptReturn {
    fn into_result(self) -> Result<Dynamic, CallError>;
    /// The value returned when an `Entity` argument names nothing.
    fn fallback() -> Dynamic;
}

macro_rules! script_return {
    ($($t:ty),*) => {$(
        impl ScriptReturn for $t {
            #[inline]
            fn into_result(self) -> Result<Dynamic, CallError> {
                Ok(Dynamic::from(self))
            }
            fn fallback() -> Dynamic {
                Dynamic::from(<$t>::default())
            }
        }
        impl ScriptReturn for CallResult<$t> {
            #[inline]
            fn into_result(self) -> Result<Dynamic, CallError> {
                self.map(Dynamic::from)
            }
            fn fallback() -> Dynamic {
                <$t as ScriptReturn>::fallback()
            }
        }
    )*};
}
script_return!(
    (),
    bool,
    i64,
    f64,
    String,
    ImmutableString,
    Map,
    Array,
    Dynamic
);

// ─── Parameter lists ─────────────────────────────────────

/// A tuple of kinds, one per parameter.
pub trait Kinds: 'static {
    /// The converted arguments, as the body takes them.
    type Out;
    const LEN: usize;
    /// The kinds' names in the listing.
    fn kind_names(typed: bool) -> Vec<&'static str>;
    /// The exact-typed signature registered beside the `Dynamic` one.
    fn preferred_types() -> Vec<TypeId>;
    fn convert(
        call: &mut Call<'_>,
        args: &mut [&mut Dynamic],
        params: &[&'static str],
    ) -> Result<Self::Out, ArgFail>;
}

/// [`Kinds`] that can register on Rhai's typed path.
pub trait TypedKinds: Kinds {
    fn register_typed<F, R>(
        engine: &mut Engine,
        name: &'static str,
        ctx: Arc<Mutex<ScriptCallContext>>,
        params: Arc<[&'static str]>,
        fallback: Dynamic,
        f: F,
    ) where
        F: Fn(&mut Call<'_>, Self::Out) -> R + Send + Sync + 'static,
        R: ScriptReturn;
}

/// A tuple of kinds with one more on the end.
pub trait Push<N> {
    type Out;
}

macro_rules! kinds_tuple {
    ($len:expr; $($K:ident $a:ident $i:tt),*) => {
        impl<$($K: Kind),*> Kinds for ($($K,)*) {
            type Out = ($($K::Out,)*);
            const LEN: usize = $len;
            fn kind_names(typed: bool) -> Vec<&'static str> {
                let _ = typed;
                vec![$(if typed { $K::TYPED_NAME } else { $K::NAME }),*]
            }
            fn preferred_types() -> Vec<TypeId> {
                vec![$(TypeId::of::<$K::Typed>()),*]
            }
            #[inline]
            #[allow(unused_variables)]
            fn convert(
                call: &mut Call<'_>,
                args: &mut [&mut Dynamic],
                params: &[&'static str],
            ) -> Result<Self::Out, ArgFail> {
                Ok(($(convert_one::<$K>(call, &mut *args[$i], params[$i])?,)*))
            }
        }

        impl<$($K: Kind),*> TypedKinds for ($($K,)*) {
            #[allow(unused_variables)]
            fn register_typed<F, R>(
                engine: &mut Engine,
                name: &'static str,
                ctx: Arc<Mutex<ScriptCallContext>>,
                params: Arc<[&'static str]>,
                fallback: Dynamic,
                f: F,
            ) where
                F: Fn(&mut Call<'_>, Self::Out) -> R + Send + Sync + 'static,
                R: ScriptReturn,
            {
                engine.register_fn(
                    name,
                    move |nctx: NativeCallContext, $($a: $K::Typed),*| -> RhaiResult {
                        let mut guard = crate::lock_or_recover(&ctx);
                        let mut call = Call::new(&mut guard, name, nctx.call_position());
                        let out = ($(
                            match $K::from_typed(&mut call, $a) {
                                Ok(v) => v,
                                Err(Conv::Skip) => return Ok(fallback.clone()),
                                Err(Conv::Mismatch) => {
                                    return Err(arg_error(
                                        &call,
                                        params[$i],
                                        $K::WANTS,
                                        $K::TYPED_NAME,
                                    ))
                                }
                            },
                        )*);
                        f(&mut call, out).into_result()
                    },
                );
            }
        }

        impl<$($K,)* NEXT> Push<NEXT> for ($($K,)*) {
            type Out = ($($K,)* NEXT,);
        }
    };
}

kinds_tuple!(0;);
kinds_tuple!(1; A v0 0);
kinds_tuple!(2; A v0 0, B v1 1);
kinds_tuple!(3; A v0 0, B v1 1, C v2 2);
kinds_tuple!(4; A v0 0, B v1 1, C v2 2, D v3 3);
kinds_tuple!(5; A v0 0, B v1 1, C v2 2, D v3 3, E v4 4);
kinds_tuple!(6; A v0 0, B v1 1, C v2 2, D v3 3, E v4 4, F_ v5 5);
kinds_tuple!(7; A v0 0, B v1 1, C v2 2, D v3 3, E v4 4, F_ v5 5, G v6 6);
kinds_tuple!(8; A v0 0, B v1 1, C v2 2, D v3 3, E v4 4, F_ v5 5, G v6 6, H v7 7);
kinds_tuple!(9; A v0 0, B v1 1, C v2 2, D v3 3, E v4 4, F_ v5 5, G v6 6, H v7 7, I v8 8);
kinds_tuple!(10; A v0 0, B v1 1, C v2 2, D v3 3, E v4 4, F_ v5 5, G v6 6, H v7 7, I v8 8, J v9 9);
kinds_tuple!(11; A v0 0, B v1 1, C v2 2, D v3 3, E v4 4, F_ v5 5, G v6 6, H v7 7, I v8 8, J v9 9, K_ v10 10);
kinds_tuple!(12; A v0 0, B v1 1, C v2 2, D v3 3, E v4 4, F_ v5 5, G v6 6, H v7 7, I v8 8, J v9 9, K_ v10 10, L v11 11);
