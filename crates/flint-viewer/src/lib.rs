//! Flint Viewer - egui-based scene validation GUI
//!
//! Provides an interactive viewer with entity inspector, scene tree,
//! constraint overlay, render stats, and visual scene tweaking.
//! Supports editable properties, 3D transform gizmos, undo/redo,
//! mouse picking, structure-preserving TOML write-back, and an optional
//! spline editor mode for interactive track editing.

pub mod app;
pub mod panels;
pub mod picking;
pub mod projection;
pub mod spline_editor;
pub mod undo;
