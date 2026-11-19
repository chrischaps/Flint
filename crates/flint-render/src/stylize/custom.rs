//! User-authored styles (#274, ADR 0112): a game describes a style in
//! `styles/<name>.shader.toml` and writes one WGSL function beside it.
//!
//! ```toml
//! # styles/duotone.shader.toml
//! version = 1
//! name = "duotone"
//! label = "Duotone"
//! stage = "display"          # or "hdr"
//! shader = "duotone.wgsl"
//! inputs = ["depth"]         # colour is always there; also "mask", "edges"
//!
//! [params.mix]
//! default = 1.0
//! min = 0.0
//! max = 1.0
//! help = "How far to the two inks"
//! ```
//!
//! ```wgsl
//! // styles/duotone.wgsl
//! fn style(p: StylePixel) -> vec4<f32> {
//!     let l = flint_luma(p.color.rgb);
//!     return vec4<f32>(mix(p.color.rgb, vec3<f32>(l), params.mix), 1.0);
//! }
//! ```
//!
//! The framework writes everything else: a prelude with the bindings, the
//! `params` struct (one `f32` per declared parameter), helpers
//! (`flint_sample`, `flint_view_depth`, `flint_style_blend`, ...) and the
//! entry points. At the display stage `p.color` and the returned colour
//! are display values (what the eye sees) on any target format: the
//! prelude decodes and encodes an sRGB target, so a user never meets the
//! gotcha the built-ins work around by hand.
//!
//! A style that reads the mask (`inputs = ["mask"]`) is per-entity
//! (#414), as ASCII is: it gets the component `<name>_style`, a
//! `style_mask` naming it switches it on with no table, and `params` is
//! a private copy the entry point fills with the pixel's own row of the
//! settings table (row 0 the scene's, then one a variant, and a slot
//! table from mask id to row), so a shader written for the whole frame
//! reads each entity's settings unchanged.
//!
//! The composed module is parsed and validated with naga before wgpu sees
//! it (natively), and an error comes back as `file:line:col` in the user's
//! file. A style that fails draws its input unchanged (a passthrough) and
//! its error is kept for the host to show ([`StyleRegistry::errors`]). The
//! WGSL is polled for changes (natively, a few times a second) and rebuilt
//! in place; an edit that fails keeps the last good build drawing.
//!
//! User styles belong to a [`StyleRegistry`] (#742), never to the process:
//! [`StyleRegistry::for_scene`] / [`load_for`](StyleRegistry::load_for)
//! load a scene's, each install an `Arc` of its own. An edited
//! `.shader.toml` is read again by
//! [`reload_changed_for`](StyleRegistry::reload_changed_for) (#275), which
//! hosts call a few times a second: the new install takes the old one's
//! index, the renderer handed the registry builds it again on the next
//! frame, and the host re-reads the scene's settings for it
//! ([`refresh_settings`]). The old install is freed when its pass is.

use super::kit;
use super::variant_table::{storage_entry, VariantTable};
use super::{
    intern, intern_list, List, MaskUse, Style, StyleDescriptor, StyleDevice, StyleFrame,
    StyleInput, StyleNeed, StyleOption, StyleParam, StylePass, StyleRegistry, StyleStage,
};
use bytemuck::{Pod, Zeroable};
use flint_core::project::{shown, AssetKind, AssetScope};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[cfg(not(target_arch = "wasm32"))]
mod bounded;
mod template;
#[cfg(not(target_arch = "wasm32"))]
mod uniformity;
pub use template::{template, Template};

mod check;
mod compose;
mod format;
mod loading;
mod packed;
mod parts;
mod pass;
pub use check::*;
pub use compose::*;
pub use format::*;
pub use loading::*;
pub use packed::*;
pub use parts::*;
pub use pass::*;

#[cfg(test)]
mod tests;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod uniformity_tests;
