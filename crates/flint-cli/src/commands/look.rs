//! Looks (#279): named post-processing presets an agent can put over any
//! scene. Sweeps of one setting at several values live in `sweep.rs`.
//!
//! A look is a `styles/<name>.look.toml`, beside the `*.style.toml`
//! generation guides: a `[look]` table that says what it is and what to tune,
//! and a `[post_process]` block in exactly the scene format. The block is
//! merged into the scene's own `[post_process]` key by key (tables merge,
//! everything else replaces), so a look changes only what it names.
//!
//! ```text
//! flint render demo/pbr_test.scene.toml --look ascii_noir -o noir.png
//! flint render demo/pbr_test.scene.toml --look ascii_noir --sweep ascii.cell=6,9,14 -o cells.png
//! flint render demo/pbr_test.scene.toml --sweep look=ascii_noir,matrix,ink_wash,graphite -o looks.png
//! flint render --look help
//! ```

use anyhow::{anyhow, Result};
use flint_scene::PostProcessDef;

// The looks themselves (the format, the merge and the lookup) live in
// flint-render since #356, so the player and viewer read them too.
pub use flint_render::look::{did_you_mean, Look};

/// `base` with `over` merged into it ([`flint_render::look::overlay`]).
pub fn overlay(base: Option<&PostProcessDef>, over: &toml::Table) -> Result<PostProcessDef> {
    flint_render::look::overlay(base, over).map_err(|e| anyhow!(e))
}

/// Find a look by name or path, from the scene's folders.
pub fn find(name_or_path: &str, scene: &str) -> Result<Look> {
    flint_render::look::find(name_or_path, scene).map_err(|e| anyhow!(e))
}

/// Every look reachable from `scene`; files that don't parse are skipped
/// with a warning.
pub fn list(scene: &str) -> Vec<Look> {
    let (looks, skipped) = flint_render::look::list_checked(scene);
    for message in skipped {
        eprintln!("warning: {message}");
    }
    looks
}

/// `--look help`: each look, what it is and what to sweep first.
pub fn help(scene: &str) -> String {
    let mut out = String::from(
        "Looks (styles/<name>.look.toml; a file beside the scene wins over a built-in):
",
    );
    for look in list(scene) {
        out.push_str(&format!(
            "
  {}  [{}]
",
            look.name, look.source
        ));
        if !look.description.is_empty() {
            out.push_str(&format!(
                "    {}
",
                look.description
            ));
        }
        for sweep in &look.tune {
            out.push_str(&format!(
                "    tune: --sweep {sweep}
"
            ));
        }
        for line in look.notes.lines().filter(|l| !l.trim().is_empty()) {
            out.push_str(&format!(
                "    | {}
",
                line.trim()
            ));
        }
    }
    out.push_str(
        "
Sweep any registered style key (`--style help` lists them), any [post_process]          field as post.<field>, look=a,b,c, or a Living Props knob as wooze.<knob>          (in builds with the wooze field).
         A scene names one with [post_process] look = \"<name>\", laid under its own keys          (--look goes over them);
F4's Look picker and Rhai set_look(\"<name>\") switch          it live.
",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::sweep::Sweep;

    /// The checks of the look format itself live with it, in
    /// `flint_render::look`; here, each built-in's tune sweeps parse.
    #[test]
    fn every_builtin_looks_tune_sweeps_parse() {
        for name in flint_render::look::builtin_names() {
            let look = find(name, "").unwrap_or_else(|e| panic!("{name}: {e:#}"));
            assert!(!look.tune.is_empty(), "{name} suggests a sweep");
            for sweep in &look.tune {
                Sweep::parse(sweep).unwrap_or_else(|e| panic!("{name} tune '{sweep}': {e}"));
            }
        }
    }
}
