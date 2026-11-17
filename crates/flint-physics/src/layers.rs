//! Collision layers (#729): named groups a collider is on (`collider.layer`)
//! and the layers it meets (`collider.mask`), and the masks a ray or a
//! character move is filtered by.
//!
//! The names come from the project, `[project.physics] layers` in
//! `flint.toml`, so every scene of a game agrees on them. `default` is
//! always the first layer: a collider that names none is on it, and meets
//! every layer, which is what every collider did before layers existed.
//! Rapier's test is symmetric: two colliders touch only when each one's
//! mask holds the other's layer. A query (a ray) is on every layer and
//! meets the colliders whose layer is in its mask, whatever their masks.

use rapier3d::prelude::{Group, InteractionGroups};

/// The layer a collider with no `layer` is on.
pub const DEFAULT_LAYER: &str = "default";

/// Rapier has 32 groups; `default` takes the first.
pub const MAX_LAYERS: usize = 32;

/// The layer names a world knows, `default` first, each with its bit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollisionLayers {
    names: Vec<String>,
}

impl Default for CollisionLayers {
    fn default() -> Self {
        Self {
            names: vec![DEFAULT_LAYER.to_string()],
        }
    }
}

/// A set of layers, as bits: what a ray or a move is filtered to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LayerMask(u32);

impl LayerMask {
    /// Every layer.
    pub const ALL: LayerMask = LayerMask(u32::MAX);
    /// No layer: meets nothing.
    pub const NONE: LayerMask = LayerMask(0);

    /// A mask from its bits, layer `i` being bit `i`.
    pub fn from_bits(bits: u32) -> Self {
        LayerMask(bits)
    }

    /// The mask's bits, layer `i` being bit `i`.
    pub fn bits(self) -> u32 {
        self.0
    }

    pub(crate) fn group(self) -> Group {
        Group::from_bits_retain(self.0)
    }

    /// The filter a query with this mask is: on every layer, meeting the
    /// colliders on one of the mask's.
    pub(crate) fn query_groups(self) -> InteractionGroups {
        InteractionGroups::new(Group::ALL, self.group())
    }
}

/// What is wrong with a list of layer names.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LayerError {
    #[error("`{name}` is not a collision layer{hint}")]
    Unknown { name: String, hint: String },
    #[error("`{0}` is listed twice")]
    Twice(String),
    #[error("a layer name is empty")]
    Empty,
    #[error("{0} layers is more than the {max} Rapier has (`default` is one of them)", max = MAX_LAYERS)]
    TooMany(usize),
}

impl CollisionLayers {
    /// The layers a project declares, `default` put first. Naming
    /// `default` in the list is allowed (it is the first layer whatever
    /// its place); a name twice, an empty one, or more than 31 others is
    /// an error.
    pub fn new(declared: &[impl AsRef<str>]) -> Result<Self, LayerError> {
        let mut names = vec![DEFAULT_LAYER.to_string()];
        for name in declared {
            let name = name.as_ref().trim();
            if name.is_empty() {
                return Err(LayerError::Empty);
            }
            if name == DEFAULT_LAYER {
                continue;
            }
            if names.iter().any(|n| n == name) {
                return Err(LayerError::Twice(name.to_string()));
            }
            names.push(name.to_string());
        }
        if names.len() > MAX_LAYERS {
            return Err(LayerError::TooMany(names.len()));
        }
        Ok(Self { names })
    }

    /// The names, `default` first, in bit order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.names.iter().map(String::as_str)
    }

    /// The bit of one layer.
    pub fn bit(&self, name: &str) -> Option<LayerMask> {
        self.names
            .iter()
            .position(|n| n == name)
            .map(|i| LayerMask(1 << i))
    }

    /// The mask of several layers. The first name that is no layer is the
    /// error, with the nearest layer named when there is one.
    pub fn mask<S: AsRef<str>>(&self, names: &[S]) -> Result<LayerMask, LayerError> {
        let mut bits = 0;
        for name in names {
            let name = name.as_ref();
            match self.bit(name) {
                Some(bit) => bits |= bit.0,
                None => return Err(self.unknown(name)),
            }
        }
        Ok(LayerMask(bits))
    }

    /// The error for a name that is no layer: the nearest one, else the
    /// list, and where layers are declared.
    pub fn unknown(&self, name: &str) -> LayerError {
        let hint = match flint_core::diagnostic::did_you_mean(name, self.names()) {
            Some(near) => format!(" ({near})"),
            None => format!(
                " (the layers are {}; a project declares its own in flint.toml, `[project.physics] layers = [..]`)",
                self.names.join(", ")
            ),
        };
        LayerError::Unknown {
            name: name.to_string(),
            hint,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_the_first_layer_whatever_the_list_says() {
        let layers = CollisionLayers::new(&["player", "default", "pickup"]).unwrap();
        assert_eq!(
            layers.names().collect::<Vec<_>>(),
            ["default", "player", "pickup"]
        );
        assert_eq!(layers.bit("default"), Some(LayerMask(1)));
        assert_eq!(layers.bit("pickup"), Some(LayerMask(4)));
        assert_eq!(
            layers.mask(&["player", "pickup"]).unwrap(),
            LayerMask(0b110)
        );
    }

    #[test]
    fn a_list_that_cannot_be_layers_is_refused() {
        assert_eq!(
            CollisionLayers::new(&["a", "a"]),
            Err(LayerError::Twice("a".into()))
        );
        assert_eq!(CollisionLayers::new(&[" "]), Err(LayerError::Empty));
        let many: Vec<String> = (0..32).map(|i| format!("l{i}")).collect();
        assert_eq!(CollisionLayers::new(&many), Err(LayerError::TooMany(33)));
        assert!(CollisionLayers::new(&many[..31]).is_ok());
    }

    #[test]
    fn an_unknown_name_says_the_nearest() {
        let layers = CollisionLayers::new(&["player", "pickup"]).unwrap();
        let err = layers.mask(&["playr"]).unwrap_err().to_string();
        assert!(err.contains("`playr` is not a collision layer"), "{err}");
        assert!(err.contains("player"), "{err}");
    }
}
