//! Flint Constraint - Validation and auto-fix system
//!
//! This crate provides constraint definitions that validate scene integrity
//! and can automatically fix violations when possible.

mod diff;
mod evaluator;
mod fixer;
mod registry;
mod report;
mod types;

pub use diff::compute_scene_diff;
pub use evaluator::ConstraintEvaluator;
pub use fixer::{ConstraintFixer, FixAction, FixReport};
pub use registry::ConstraintRegistry;
pub use report::{ValidationReport, Violation};
pub use types::{AutoFix, AutoFixStrategy, ConstraintDef, ConstraintFile, ConstraintKind, Severity};
