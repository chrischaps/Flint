//! Print the parameter reference for every built-in generator, as Markdown.
//!
//! ```bash
//! cargo run -p flint-procgen --example param_catalog
//! ```
//!
//! This is the catalog in the crate README, between its `catalog` markers;
//! `tests/docs.rs` fails when the two differ, so after changing a schema,
//! paste this output there.

use flint_procgen::{register_built_in_generators, GeneratorRegistry};

fn main() {
    let mut registry = GeneratorRegistry::new();
    register_built_in_generators(&mut registry);
    print!("{}", registry.reference_markdown());
}
