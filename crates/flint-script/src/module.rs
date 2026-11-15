//! Script modules a subsystem owns (#698, ADR 0116 decision 5).
//!
//! Most of the API is Flint's core and every script can call it. A few
//! modules belong to one optional feature: Living Props' knobs (`living`),
//! the wooze field (`wooze`), Living Props' A/B and commit (`debug`), a
//! rhythm session's state (`conducted`). Those are declared by the crate
//! that owns the feature, through a [`ScriptModule`], and their functions
//! run only where the host has enabled the module: a host enables what its
//! subsystems for the scene are ([`crate::ScriptSystem::set_enabled_modules`]).
//! A call anywhere else is a script error that says the module is not
//! enabled and when it is, instead of a command that does nothing.
//!
//! What such a function asks of its subsystem is a [`ModuleCommand`] (the
//! one `ScriptCommand::Module` variant), whose payload is the owner's own
//! type: the core command enum names none of them. State the host hands
//! the module's functions each frame (a rhythm session's snapshot) is
//! module state, one value of each type ([`ModuleState`]).

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use crate::api::Module;

/// A module of script functions an optional feature owns, declared by the
/// crate that has the feature (the player's living props, its music).
/// `ScriptSystem::declare_module` declares it; its functions then run only
/// while the host has it enabled.
pub trait ScriptModule: Sync {
    /// The module's name, as `flint script api` lists it: `living`.
    fn name(&self) -> &'static str;

    /// When a host enables it, finishing "it is enabled when ...": `the
    /// scene has living props`. The "not enabled" error and the listing
    /// say it.
    fn enabled_when(&self) -> &'static str;

    /// Declare the module's functions on `m`, with `.run()` (a gated
    /// function needs the call context to know whether it may run).
    fn declare(&self, m: &mut Module<'_>);
}

/// A gated module's name and when it is enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gate {
    pub module: &'static str,
    pub enabled_when: &'static str,
}

/// What a module's payload is: anything its owner can tell apart.
pub trait ModulePayload: Any + fmt::Debug + Send + Sync {
    fn as_any(&self) -> &dyn Any;
}

impl<T: Any + fmt::Debug + Send + Sync> ModulePayload for T {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// A command one of a [`ScriptModule`]'s functions queued: the function's
/// name (for the host's refusals and notes) and what it asked, in the
/// owner's own type. The subsystem that owns the module takes it before
/// the core's dispatcher (`script_command`); one no subsystem takes is
/// `ScriptHost::module_command`'s.
#[derive(Clone)]
pub struct ModuleCommand {
    function: &'static str,
    payload: Arc<dyn ModulePayload>,
}

impl ModuleCommand {
    pub fn new(function: &'static str, payload: impl ModulePayload) -> Self {
        Self {
            function,
            payload: Arc::new(payload),
        }
    }

    /// The script function that queued it.
    pub fn function(&self) -> &'static str {
        self.function
    }

    /// The payload, if it is a `T`.
    pub fn payload<T: Any>(&self) -> Option<&T> {
        // Through the trait object: the blanket impl would answer for the
        // `Arc` itself
        let payload: &dyn ModulePayload = &*self.payload;
        payload.as_any().downcast_ref::<T>()
    }
}

impl fmt::Debug for ModuleCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {:?}", self.function, self.payload)
    }
}

/// One value of each type a module's functions read, set by the host (a
/// rhythm session's per-frame snapshot). It outlives a scene, as the
/// host's other per-frame inputs do; a value never set reads as its
/// `Default`.
#[derive(Default)]
pub struct ModuleState {
    values: HashMap<TypeId, Box<dyn Any + Send>>,
}

impl ModuleState {
    /// The `T`, made with its `Default` the first time it is asked for.
    pub fn get_mut<T: Any + Send + Default>(&mut self) -> &mut T {
        self.values
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(T::default()))
            .downcast_mut::<T>()
            .expect("stored under its own TypeId")
    }

    /// The `T`, if one was set or asked for.
    pub fn get<T: Any + Send>(&self) -> Option<&T> {
        self.values
            .get(&TypeId::of::<T>())
            .and_then(|v| v.downcast_ref::<T>())
    }

    /// Replace the `T`.
    pub fn set<T: Any + Send>(&mut self, value: T) {
        self.values.insert(TypeId::of::<T>(), Box::new(value));
    }
}

impl fmt::Debug for ModuleState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ModuleState({} values)", self.values.len())
    }
}
