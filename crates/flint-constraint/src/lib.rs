//! Flint Constraint - Validation and auto-fix system
//!
//! This crate provides constraint definitions that validate scene integrity
//! and can automatically fix violations when possible.
#![cfg_attr(test, allow(clippy::disallowed_methods))] // unit tests read their files off the disk and assert on TOML as written; the lib build still holds the rules (#761)

mod diff;
mod evaluator;
mod fixer;
mod registry;
mod report;
mod types;

pub use diff::compute_scene_diff;
pub use evaluator::{ConstraintEvaluator, BAD_CONSTRAINT};
pub use fixer::{ConstraintFixer, FixAction, FixReport};
pub use registry::{BadConstraint, ConstraintRegistry};
pub use report::{ValidationReport, Violation};
pub use types::{
    AutoFix, AutoFixStrategy, ConstraintDef, ConstraintFile, ConstraintKind, Severity,
};
