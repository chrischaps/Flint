//! CLI command implementations
//!
//! A module that names a package's crate is compiled only with that
//! package's feature (#1333). A package's own commands are its crate's
//! extension (#1553, #1555), and a kernel command it fills in asks its
//! point (#1557): without the package, `absent` names it, with exit 2.

pub mod absent;
pub mod asset;
pub mod asset_where;
pub mod attach;
pub mod build;
pub mod bundle;
pub mod capture;
mod capture_compare;
mod capture_disk;
pub mod check;
pub mod common_args;
pub mod dxc;
pub mod edit_router;
pub mod entity;
pub mod env;
pub mod examples;
pub mod export;
pub mod frame;
pub mod frame_seq;
// `flint gen` is the kernel's in every build: a glyph set runs here, a
// spec through the procgen package's point (#1543, #1557), which asks the
// ai package's steps and living's strip by points of its own (#1572)
pub mod gen;
/// The GIF writer, the Package API's since #1558 (living's `gen --strip`
/// writes with it too)
pub use flint_package::cli::gif as gif_writer;
pub mod glyph_preview;
pub mod golden_sheet;
pub mod init;
pub mod look;
pub mod look_measure;
pub mod mesh_seq;
pub mod package;

pub mod play;
pub mod playtest;
pub mod prefab;
pub mod preview;
pub mod profile;
pub mod query;
pub mod render;
pub mod render_args;
mod render_frame_dump;
pub mod render_goldens;
pub mod render_plan;
pub mod render_post;
pub mod render_report;
pub mod render_retired;
pub mod render_watch;
pub mod scene;
pub mod schema;
pub mod schema_export;
pub mod script;
// `gen-preview` / `tex-edit`: the kernel's hidden aliases of `flint edit`
// on a spec, whose editors are the procgen package's (#1557)
pub mod spec_edit;
pub mod spline_edit;
pub mod style;
pub mod style_sheet;
pub mod sweep;
pub mod terrain_edit;
pub(crate) mod trace;
pub mod validate;
pub mod variants;
