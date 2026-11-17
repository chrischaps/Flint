//! Print the parameter reference for every built-in generator, as Markdown.
//!
//! ```bash
//! cargo run -p flint-procgen --example param_catalog        # the generators
//! cargo run -p flint-procgen --example param_catalog -- ops # the texture ops
//! ```
//!
//! These are the catalogs in the crate README, between its `catalog` and
//! `op-catalog` markers; `tests/examples.rs` fails when either differs, so
//! after changing a schema or an op's descriptor, paste this output there.

use flint_procgen::{op_reference_markdown, register_built_in_generators, GeneratorRegistry};

fn main() {
    if std::env::args().nth(1).as_deref() == Some("ops") {
        print!("{}", op_reference_markdown());
        return;
    }
    let mut registry = GeneratorRegistry::new();
    register_built_in_generators(&mut registry);
    print!("{}", registry.reference_markdown());
}
