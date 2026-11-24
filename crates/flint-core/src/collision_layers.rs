//! Collision layer names (#729), and what is said of a collider naming one
//! that isn't there (#1220).
//!
//! `flint_physics` reads `collider.layer` and `collider.mask` when the game
//! runs, and `flint_scene::diagnose` reads them before it does (`flint
//! check`, `flint validate`). Neither crate depends on the other, so the
//! names and the one message both say live here: a typo is told the same
//! way at either end.

use crate::diagnostic::{self, codes, Diagnostic};

/// The layer a collider with no `layer` is on, and the first layer of every
/// project, declared or not.
pub const DEFAULT_LAYER: &str = "default";

/// The layer names a project's colliders may use: `default`, then the
/// project's `[project.physics] layers` in their order, each once.
pub fn layer_names(declared: &[impl AsRef<str>]) -> Vec<&str> {
    let mut names = vec![DEFAULT_LAYER];
    for name in declared {
        let name = name.as_ref().trim();
        if !name.is_empty() && !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// Where a name that is no layer was written, which decides what is made
/// of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerUse {
    /// `collider.layer`: it is read as `default`.
    Layer,
    /// An entry of `collider.mask`: it is left out.
    Mask,
}

/// What to do about `name`, which is none of `names` (from
/// [`layer_names`]): the nearest layer; else, when the project declares no
/// layers, the line to add to `flint.toml`; else the layers there are and
/// where they are declared.
pub fn unknown_layer_hint(name: &str, names: &[&str]) -> String {
    if let Some(near) = diagnostic::did_you_mean(name, names.iter().copied()) {
        return near;
    }
    if names.len() <= 1 {
        return format!(
            "this project declares no collision layers: add `[project.physics] layers = \
             [\"{name}\"]` to flint.toml"
        );
    }
    format!(
        "the layers are {}; a project declares its own in flint.toml, `[project.physics] \
         layers = [..]`",
        names.join(", ")
    )
}

/// `bad-enum-value` for a collider naming a layer that isn't one, with no
/// entity or field: the caller says where (the physics view's `report`
/// does, `flint_scene::diagnose` gives `layer` or `mask[i]`).
pub fn unknown_layer(name: &str, names: &[&str], used: LayerUse) -> Diagnostic {
    let then = match used {
        LayerUse::Layer => format!("it is read as `{DEFAULT_LAYER}`"),
        LayerUse::Mask => "it is left out".to_string(),
    };
    Diagnostic::error(
        codes::BAD_ENUM_VALUE,
        format!("\"{name}\" is not a collision layer"),
    )
    .hint(format!("{}; {then}", unknown_layer_hint(name, names)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_comes_first_and_once() {
        assert_eq!(
            layer_names(&["player", "default", "pickup", "player"]),
            ["default", "player", "pickup"]
        );
        assert_eq!(layer_names(&[] as &[&str]), ["default"]);
    }

    #[test]
    fn a_typo_is_told_the_nearest_and_what_becomes_of_it() {
        let names = layer_names(&["player", "pickup"]);
        let d = unknown_layer("playr", &names, LayerUse::Layer);
        assert_eq!(d.code, codes::BAD_ENUM_VALUE);
        assert_eq!(d.message, "\"playr\" is not a collision layer");
        let hint = d.hint.unwrap();
        assert!(hint.contains("`player`"), "{hint}");
        assert!(hint.ends_with("it is read as `default`"), "{hint}");
        let hint = unknown_layer("pikcup", &names, LayerUse::Mask)
            .hint
            .unwrap();
        assert!(
            hint.contains("`pickup`") && hint.ends_with("left out"),
            "{hint}"
        );
    }

    #[test]
    fn a_project_with_no_layers_is_told_to_declare_one() {
        let hint = unknown_layer_hint("player", &layer_names(&[] as &[&str]));
        assert!(
            hint.contains("add `[project.physics] layers = [\"player\"]` to flint.toml"),
            "{hint}"
        );
    }

    #[test]
    fn a_name_near_nothing_is_given_the_list() {
        let hint = unknown_layer_hint("water", &layer_names(&["player", "pickup"]));
        assert!(
            hint.contains("the layers are default, player, pickup"),
            "{hint}"
        );
    }
}
