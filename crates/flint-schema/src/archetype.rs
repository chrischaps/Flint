//! Archetype schema definitions

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Schema definition for an archetype (a bundle of components)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchetypeSchema {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// List of component names this archetype includes
    pub components: Vec<String>,
    /// Default values for component fields
    #[serde(default)]
    pub defaults: HashMap<String, toml::Value>,
    /// How the renderer draws this archetype's placeholder box, from
    /// `[archetype.<name>.visual]` (#751); `None` is the grey of
    /// [`ArchetypeVisual::PLACEHOLDER`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visual: Option<ArchetypeVisual>,
}

/// The placeholder an entity of an archetype is drawn as when it has a
/// `bounds` box and nothing else to draw (no model, sprite or procedural
/// shape): `[archetype.<name>.visual]` in the archetype's TOML (#751).
///
/// ```toml
/// [archetype.crate.visual]
/// color = [0.8, 0.55, 0.3]   # RGB, or RGBA; alpha below 1 is see-through
/// wireframe = false          # true draws the box's edges only
/// ```
///
/// The renderer knows no archetype by name: an archetype with no `visual`
/// draws grey, and a game's own archetype sets its colour here.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchetypeVisual {
    /// The box's colour, 0-1 per channel, as its vertex colour. Three
    /// numbers are opaque.
    #[serde(
        default = "ArchetypeVisual::placeholder_color",
        deserialize_with = "rgb_or_rgba"
    )]
    pub color: [f32; 4],
    /// Draw the box's edges only (the examples' `room`).
    #[serde(default)]
    pub wireframe: bool,
}

impl ArchetypeVisual {
    /// What an archetype with no `visual`, or an entity with none, draws.
    pub const PLACEHOLDER: Self = Self {
        color: [0.5, 0.5, 0.5, 1.0],
        wireframe: false,
    };

    fn placeholder_color() -> [f32; 4] {
        Self::PLACEHOLDER.color
    }
}

impl Default for ArchetypeVisual {
    fn default() -> Self {
        Self::PLACEHOLDER
    }
}

/// `[r, g, b]` (opaque) or `[r, g, b, a]`; integers read as numbers.
fn rgb_or_rgba<'de, D: serde::Deserializer<'de>>(de: D) -> Result<[f32; 4], D::Error> {
    let channels = Vec::<f32>::deserialize(de)?;
    match channels[..] {
        [r, g, b] => Ok([r, g, b, 1.0]),
        [r, g, b, a] => Ok([r, g, b, a]),
        _ => Err(serde::de::Error::custom(format!(
            "a colour is [r, g, b] or [r, g, b, a], not {} numbers",
            channels.len()
        ))),
    }
}

impl ArchetypeSchema {
    /// Check if this archetype includes a specific component
    pub fn has_component(&self, component: &str) -> bool {
        self.components.iter().any(|c| c == component)
    }

    /// Get the default value for a component's field
    pub fn get_default(&self, component: &str, field: &str) -> Option<&toml::Value> {
        self.defaults.get(component).and_then(|v| v.get(field))
    }
}

/// TOML file format for archetype schemas
#[derive(Debug, Deserialize)]
pub struct ArchetypeSchemaFile {
    pub archetype: HashMap<String, ArchetypeSchemaDefinition>,
}

#[derive(Debug, Deserialize)]
pub struct ArchetypeSchemaDefinition {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub components: Vec<String>,
    #[serde(default)]
    pub defaults: HashMap<String, toml::Value>,
    #[serde(default)]
    pub visual: Option<ArchetypeVisual>,
    /// `extend = true` (#1054): these components and defaults are added to
    /// the archetype an earlier schema layer declared, rather than replacing
    /// it whole. A component it lists is appended unless already listed;
    /// each `[archetype.x.defaults.<component>]` merges into the earlier
    /// one key by key; a `visual` replaces the earlier one. Without it a
    /// later layer's archetype of the same name replaces the earlier one.
    #[serde(default)]
    pub extend: bool,
    /// Any other key, which the loader ignores and the registry lints
    /// (`ignored-schema-key`): a misspelt `extned = true` would otherwise
    /// replace the archetype whole without a word.
    #[serde(flatten)]
    pub stray: std::collections::BTreeMap<String, toml::Value>,
}

/// The keys an `[archetype.<name>]` table may hold; any other is ignored by
/// the loader and reported in [`SchemaRegistry::lints`](crate::SchemaRegistry::lints).
pub const ARCHETYPE_KEYS: &[&str] = &["description", "components", "defaults", "visual", "extend"];

impl ArchetypeSchemaDefinition {
    pub fn to_archetype_schema(self, name: String) -> ArchetypeSchema {
        ArchetypeSchema {
            name,
            description: self.description,
            components: self.components,
            defaults: self.defaults,
            visual: self.visual,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn visual_of(toml: &str) -> Result<Option<ArchetypeVisual>, toml::de::Error> {
        let file: ArchetypeSchemaFile = toml::from_str(toml)?;
        Ok(file.archetype.into_values().next().unwrap().visual)
    }

    #[test]
    fn a_visual_block_gives_the_colour_and_wireframe() {
        let visual = visual_of(
            "[archetype.room]\ncomponents = []\n\n[archetype.room.visual]\n\
             color = [0.27, 0.53, 1.0, 0.5]\nwireframe = true\n",
        )
        .unwrap()
        .unwrap();
        assert_eq!(visual.color, [0.27, 0.53, 1.0, 0.5]);
        assert!(visual.wireframe);
    }

    #[test]
    fn three_channels_are_opaque_and_integers_are_numbers() {
        let visual = visual_of("[archetype.crate.visual]\ncolor = [1, 0, 0.5]\n")
            .unwrap()
            .unwrap();
        assert_eq!(visual.color, [1.0, 0.0, 0.5, 1.0]);
        assert!(!visual.wireframe);
    }

    #[test]
    fn no_visual_block_is_none_and_an_empty_one_is_the_placeholder() {
        assert_eq!(visual_of("[archetype.a]\n").unwrap(), None);
        assert_eq!(
            visual_of("[archetype.a.visual]\n").unwrap(),
            Some(ArchetypeVisual::PLACEHOLDER)
        );
    }

    #[test]
    fn a_misspelt_key_or_a_short_colour_is_an_error() {
        let typo = visual_of("[archetype.a.visual]\ncolour = [1, 0, 0]\n").unwrap_err();
        assert!(typo.to_string().contains("colour"), "{typo}");
        let short = visual_of("[archetype.a.visual]\ncolor = [1, 0]\n").unwrap_err();
        assert!(short.to_string().contains("not 2 numbers"), "{short}");
    }
}
