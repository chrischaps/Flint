//! The script API (ADR 0116): every function a script can call, declared
//! a module at a time in `api/<module>.rs` through the builder
//! (`api/builder.rs`) into the registry (`api/registry.rs`) that `flint
//! script api` lists. Each function's body reaches the world through the
//! builder's `Call`, never a raw pointer.
//!
//! What a game's extension registers on the Rhai engine itself
//! (`ScriptSystem::rhai_engine_mut`, `PlayerExtension::register_script_api`)
//! is the only undeclared part; `PlayerExtension::declare_script_api`
//! declares into the same registry instead.
//!
//! A module an optional feature owns (`living`, `wooze`, `debug`,
//! `conducted`) isn't declared here: the feature's crate declares it
//! through a [`crate::ScriptModule`] (#698, `crate::module`).

mod animation;
mod audio;
pub mod builder;
mod camera;
mod camera_2d;
mod chunk;
mod convert;
mod data;
#[cfg(test)]
mod declared_tests;
mod draw;
mod draw_world;
mod entity;
#[cfg(test)]
mod entity_tests;
mod event;
mod expression;
#[cfg(test)]
mod host_io_tests;
mod input;
mod log;
mod math;
mod music;
#[cfg(test)]
mod naming_tests;
mod ocean;
mod opts;
#[cfg(not(target_arch = "wasm32"))]
#[doc(hidden)]
pub mod overhead;
mod particle;
mod persistence;
mod physics;
mod post_process;
mod quality;
pub mod registry;
mod scene;
mod screen_ui;
mod spline;
mod sprite;
mod state;
mod style;
mod terrain;
mod time;
mod touch;
mod ui;
mod view;

pub use builder::{Call, CallError, CallResult, Func, Module, ScriptApi};
pub use convert::{dynamic_to_toml, map_to_toml, toml_to_dynamic};
pub use registry::ApiRegistry;

/// Declare the deterministic run's `timestamp()`, which fails (see
/// `ScriptSystem::make_deterministic`).
pub(crate) fn forbid_wall_clock(api: &mut ScriptApi) {
    time::forbid_wall_clock(api);
}

/// Declare Flint's whole script API, module by module.
pub fn register_all(api: &mut ScriptApi, random: &crate::random::ScriptRandom) {
    entity::register(api);
    input::register(api);
    time::register(api);
    audio::register(api);
    animation::register(api);
    physics::register(api);
    camera::register(api);
    post_process::register(api);
    quality::register(api);
    math::register(api);
    random.declare(api);
    event::register(api);
    log::register(api);
    draw::register(api);
    draw_world::register(api);
    view::register(api);
    ui::register(api);
    screen_ui::register(api);
    sprite::register(api);
    camera_2d::register(api);
    touch::register(api);
    style::register(api);
    particle::register(api);
    spline::register(api);
    ocean::register(api);
    terrain::register(api);
    chunk::register(api);
    scene::register(api);
    state::register(api);
    persistence::register(api);
    data::register(api);
    expression::register(api);
    music::register(api);
}
