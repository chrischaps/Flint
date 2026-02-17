//! GUI panels for the viewer

mod entity_inspector;
mod render_stats;
mod scene_tree;
pub mod spline_panel;
mod view_gizmo;

pub use entity_inspector::EntityInspector;
pub use render_stats::RenderStats;
pub use scene_tree::SceneTree;
pub use spline_panel::SplinePanelAction;
pub use view_gizmo::{CameraView, GizmoAction, ViewGizmo};
