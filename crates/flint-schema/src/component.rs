//! Component schema definitions
//!
//! The field types, in their one spelling ([`TYPE_NAMES`]; ADR 0115 decision
//! 4, #684):
//!
//! | type | a value is |
//! |---|---|
//! | `bool` | `true` / `false` |
//! | `i32`, `i64` | an integer |
//! | `f32`, `f64` | a number (an integer is taken) |
//! | `string` | a string |
//! | `entity` | the name of an entity in the scene; `""` is "not set" |
//! | `asset` | the name of a file of `kind`, found by the project's asset search; `""` is "not set" |
//! | `vec2`, `vec3`, `vec4` | exactly that many numbers (`vec3` also `{ x, y, z }`) |
//! | `color` | `[r, g, b]` or `[r, g, b, a]` |
//! | `transform` | a table of `position`, `rotation`, `scale` |
//! | `enum` | one of `values` |
//! | `array` | a list of `element` |
//! | `table` | a table; with `fields`, a table of those fields |
//!
//! [`OLD_TYPE_NAMES`] are the spellings that still load and that
//! [`lint`](crate::lint) reports.

use flint_core::project::AssetKind;
use serde::Deserialize;
use std::collections::HashMap;

/// The one spelling of each field type, and what a value of it is. The book's
/// table (concepts/schemas.md) and `flint schema --format json`'s `types` follow it.
pub const TYPE_NAMES: &[(&str, &str)] = &[
    ("bool", "true or false"),
    ("i32", "an integer"),
    ("i64", "an integer"),
    ("f32", "a number; an integer is taken"),
    ("f64", "a number; an integer is taken"),
    ("string", "a string"),
    (
        "entity",
        "the name of an entity in the scene, checked to exist; \"\" is not set",
    ),
    (
        "asset",
        "the name of a file of `kind` (a kind of `flint asset where`, or a list of them), \
         checked through the project's asset search; \"\" is not set",
    ),
    ("vec2", "exactly 2 numbers"),
    ("vec3", "exactly 3 numbers, or { x, y, z }"),
    ("vec4", "exactly 4 numbers"),
    ("color", "[r, g, b] or [r, g, b, a]"),
    ("transform", "a table of position, rotation and scale"),
    ("enum", "one of `values`"),
    (
        "array",
        "a list of `element` (a type name, or a field definition)",
    ),
    (
        "table",
        "a table; with `fields`, a table of those fields, checked like a component's",
    ),
];

/// Spellings that still load, and the one to write instead. A schema file
/// that uses one gets an `old-type-name` warning ([`lint`](crate::lint)).
pub const OLD_TYPE_NAMES: &[(&str, &str)] = &[
    ("float", "f32"),
    ("boolean", "bool"),
    ("int", "i32"),
    ("integer", "i32"),
    ("entity_ref", "entity"),
];

/// The one spelling of `name`: itself, the replacement of an old spelling,
/// or `None` for a name that is no type.
pub fn type_name_of(name: &str) -> Option<&'static str> {
    TYPE_NAMES
        .iter()
        .map(|(n, _)| *n)
        .find(|n| *n == name)
        .or_else(|| {
            OLD_TYPE_NAMES
                .iter()
                .find(|(old, _)| *old == name)
                .map(|(_, new)| *new)
        })
}

/// The type of a field in a component schema
#[derive(Debug, Clone, PartialEq)]
pub enum FieldType {
    Bool,
    I32,
    I64,
    F32,
    F64,
    String,
    /// The name of another entity of the scene. Validated as a string here;
    /// `flint_scene::diagnose` checks the entity exists (`unknown-entity`).
    /// An empty string is "not set": a script fills it at run time.
    Entity,
    /// The name of a file the project's asset search finds as one of
    /// `kinds` (ADR 0114). Validated as a string here; `flint_scene::diagnose`
    /// runs the search (`missing-asset`, with the paths tried). An empty
    /// string is "not set". `unless` names a sibling `bool` field that, when
    /// true, says no file is looked for (`model.runtime`).
    Asset {
        kinds: Vec<AssetKind>,
        unless: Option<String>,
    },
    /// Exactly 2 numbers.
    Vec2,
    /// Exactly 3 numbers, or `{ x, y, z }`.
    Vec3,
    /// Exactly 4 numbers.
    Vec4,
    Transform,
    /// `[r, g, b]` or `[r, g, b, a]`, what `flint_core::toml_util::toml_color`
    /// reads. The older `{ r, g, b }` table and `0xRRGGBB` integer still
    /// validate; no engine reader takes them.
    Color,
    /// A TOML table with no fixed shape (one animator layer, say). Validated
    /// only as "is a table"; the component that reads it checks its keys.
    Table,
    /// A table that declares its own fields (`type = "table"` with
    /// `fields`), checked like a component: each value against its field, an
    /// undeclared key a warning.
    Fields {
        fields: HashMap<String, FieldSchema>,
    },
    Enum {
        values: Vec<String>,
    },
    Array {
        element: Box<FieldType>,
    },
}

impl FieldType {
    /// The type's one spelling ([`TYPE_NAMES`]).
    pub fn type_name(&self) -> &'static str {
        match self {
            FieldType::Bool => "bool",
            FieldType::I32 => "i32",
            FieldType::I64 => "i64",
            FieldType::F32 => "f32",
            FieldType::F64 => "f64",
            FieldType::String => "string",
            FieldType::Entity => "entity",
            FieldType::Asset { .. } => "asset",
            FieldType::Vec2 => "vec2",
            FieldType::Vec3 => "vec3",
            FieldType::Vec4 => "vec4",
            FieldType::Transform => "transform",
            FieldType::Color => "color",
            FieldType::Table | FieldType::Fields { .. } => "table",
            FieldType::Enum { .. } => "enum",
            FieldType::Array { .. } => "array",
        }
    }

    /// The type as a person reads it: `enum[a, b]`, `array<vec3>`,
    /// `asset<models|specs>`, `table{xy, xz}`. What `flint schema` prints.
    pub fn describe(&self) -> String {
        match self {
            FieldType::Enum { values } => format!("enum[{}]", values.join(", ")),
            FieldType::Array { element } => format!("array<{}>", element.describe()),
            FieldType::Asset { kinds, .. } => {
                let labels: Vec<&str> = kinds.iter().map(|k| k.label()).collect();
                format!("asset<{}>", labels.join("|"))
            }
            FieldType::Fields { fields } => {
                let mut names: Vec<&str> = fields.keys().map(String::as_str).collect();
                names.sort_unstable();
                format!("table{{{}}}", names.join(", "))
            }
            other => other.type_name().to_string(),
        }
    }
}

/// Schema for a single field within a component
#[derive(Debug, Clone, PartialEq)]
pub struct FieldSchema {
    pub field_type: FieldType,
    pub default: Option<toml::Value>,
    pub required: bool,
    pub description: Option<String>,
    pub min: Option<f64>,
    pub max: Option<f64>,
}

impl FieldSchema {
    /// A field of `field_type` with nothing else said about it.
    pub fn of(field_type: FieldType) -> Self {
        Self {
            field_type,
            default: None,
            required: false,
            description: None,
            min: None,
            max: None,
        }
    }
}

/// Schema definition for a component
#[derive(Debug, Clone, PartialEq)]
pub struct ComponentSchema {
    pub name: String,
    pub description: Option<String>,
    pub fields: HashMap<String, FieldSchema>,
}

impl ComponentSchema {
    /// Get a field schema by name
    pub fn get_field(&self, name: &str) -> Option<&FieldSchema> {
        self.fields.get(name)
    }

    /// List all field names
    pub fn field_names(&self) -> Vec<&str> {
        self.fields.keys().map(|s| s.as_str()).collect()
    }

    /// Check if a field is required
    pub fn is_field_required(&self, name: &str) -> bool {
        self.fields.get(name).map(|f| f.required).unwrap_or(false)
    }

    /// Whether the schema declares no field: it names the component and
    /// checks nothing in it (a game's script-only component).
    pub fn is_open(&self) -> bool {
        self.fields.is_empty()
    }
}

/// TOML file format for component schemas
#[derive(Debug, Deserialize)]
pub struct ComponentSchemaFile {
    pub component: HashMap<String, ComponentSchemaDefinition>,
}

#[derive(Debug, Deserialize)]
pub struct ComponentSchemaDefinition {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub fields: HashMap<String, FieldSchemaDefinition>,
}

/// The keys a `[component.<name>]` table may hold; any other is ignored by
/// the loader and reported by [`lint`](crate::lint).
pub const COMPONENT_KEYS: &[&str] = &["description", "fields"];

/// The keys a field definition may hold ([`DetailedFieldSchema`]).
pub const FIELD_KEYS: &[&str] = &[
    "type",
    "values",
    "default",
    "required",
    "description",
    "min",
    "max",
    "element",
    "kind",
    "unless",
    "fields",
];

/// Field definition as it appears in TOML files
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum FieldSchemaDefinition {
    Simple(String),
    Detailed(DetailedFieldSchema),
}

/// `kind = "models"` or `kind = ["models", "specs"]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum KindNames {
    One(String),
    Many(Vec<String>),
}

impl KindNames {
    pub fn names(&self) -> Vec<&str> {
        match self {
            KindNames::One(name) => vec![name.as_str()],
            KindNames::Many(names) => names.iter().map(String::as_str).collect(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct DetailedFieldSchema {
    #[serde(rename = "type")]
    pub field_type: String,
    #[serde(default)]
    pub values: Option<Vec<String>>,
    #[serde(default)]
    pub default: Option<toml::Value>,
    #[serde(default)]
    pub required: Option<bool>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    /// An array's element: a type name, or a whole field definition (an
    /// array of `entity`, of tables with `fields`).
    #[serde(default)]
    pub element: Option<Box<FieldSchemaDefinition>>,
    /// An asset's kind or kinds, by the labels `flint asset where` takes.
    #[serde(default)]
    pub kind: Option<KindNames>,
    /// An asset's sibling `bool` field that turns the search off.
    #[serde(default)]
    pub unless: Option<String>,
    /// A table's own fields.
    #[serde(default)]
    pub fields: Option<HashMap<String, FieldSchemaDefinition>>,
}

impl FieldSchemaDefinition {
    pub fn to_field_schema(self) -> FieldSchema {
        match self {
            FieldSchemaDefinition::Simple(type_str) => {
                FieldSchema::of(parse_field_type(&type_str, None))
            }
            FieldSchemaDefinition::Detailed(d) => FieldSchema {
                field_type: parse_field_type(&d.field_type, Some(&d)),
                default: d.default,
                required: d.required.unwrap_or(false),
                description: d.description,
                min: d.min,
                max: d.max,
            },
        }
    }

    fn field_type(&self) -> FieldType {
        match self {
            FieldSchemaDefinition::Simple(type_str) => parse_field_type(type_str, None),
            FieldSchemaDefinition::Detailed(d) => parse_field_type(&d.field_type, Some(d)),
        }
    }

    /// The type name this field writes that the parser doesn't know, if any:
    /// its own type, its array element's, or one of its table's fields'. An
    /// unknown name still loads (as `String`), so this is how the loader and
    /// tests catch a typo.
    pub fn unknown_type_name(&self) -> Option<&str> {
        let (type_str, detail) = match self {
            FieldSchemaDefinition::Simple(t) => (t.as_str(), None),
            FieldSchemaDefinition::Detailed(d) => (d.field_type.as_str(), Some(d)),
        };
        if type_name_of(type_str).is_none() {
            return Some(type_str);
        }
        let detail = detail?;
        if let Some(unknown) = detail.element.as_ref().and_then(|e| e.unknown_type_name()) {
            return Some(unknown);
        }
        detail
            .fields
            .iter()
            .flat_map(|fields| fields.values())
            .find_map(|field| field.unknown_type_name())
    }
}

/// Unknown names fall back to String. The registry warns about them at
/// load, naming the schema and field (see `unknown_type_name`).
fn parse_field_type(type_str: &str, detail: Option<&DetailedFieldSchema>) -> FieldType {
    match type_name_of(type_str) {
        Some("bool") => FieldType::Bool,
        Some("i32") => FieldType::I32,
        Some("i64") => FieldType::I64,
        Some("f32") => FieldType::F32,
        Some("f64") => FieldType::F64,
        Some("entity") => FieldType::Entity,
        Some("asset") => FieldType::Asset {
            // A kind that is no kind is left out (the lint says so); with
            // none left the field is checked as a string and no file is
            // looked for
            kinds: detail
                .and_then(|d| d.kind.as_ref())
                .map(|kind| {
                    kind.names()
                        .into_iter()
                        .filter_map(AssetKind::from_label)
                        .collect()
                })
                .unwrap_or_default(),
            unless: detail.and_then(|d| d.unless.clone()),
        },
        Some("vec2") => FieldType::Vec2,
        Some("vec3") => FieldType::Vec3,
        Some("vec4") => FieldType::Vec4,
        Some("transform") => FieldType::Transform,
        Some("color") => FieldType::Color,
        Some("table") => match detail.and_then(|d| d.fields.as_ref()) {
            Some(fields) => FieldType::Fields {
                fields: fields
                    .iter()
                    .map(|(name, def)| (name.clone(), def.clone().to_field_schema()))
                    .collect(),
            },
            None => FieldType::Table,
        },
        Some("enum") => FieldType::Enum {
            values: detail.and_then(|d| d.values.clone()).unwrap_or_default(),
        },
        // An array with no `element` is an array of strings.
        Some("array") => FieldType::Array {
            element: Box::new(
                detail
                    .and_then(|d| d.element.as_ref())
                    .map(|element| element.field_type())
                    .unwrap_or(FieldType::String),
            ),
        },
        _ => FieldType::String,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(type_str: &str, element: Option<&str>) -> FieldSchemaDefinition {
        FieldSchemaDefinition::Detailed(DetailedFieldSchema {
            field_type: type_str.into(),
            element: element.map(|e| Box::new(FieldSchemaDefinition::Simple(e.into()))),
            ..Default::default()
        })
    }

    fn parsed(toml_text: &str) -> FieldSchema {
        let def: FieldSchemaDefinition = toml::from_str::<toml::Value>(toml_text)
            .unwrap()
            .try_into()
            .unwrap();
        def.to_field_schema()
    }

    #[test]
    fn toml_spelled_names_parse() {
        let parse = |name: &str| def(name, None).to_field_schema().field_type;
        assert_eq!(parse("integer"), FieldType::I32);
        assert_eq!(parse("boolean"), FieldType::Bool);
        assert_eq!(parse("float"), FieldType::F32);
        assert_eq!(parse("entity_ref"), FieldType::Entity);
        assert_eq!(parse("table"), FieldType::Table);
        assert_eq!(
            def("array", Some("table")).to_field_schema().field_type,
            FieldType::Array {
                element: Box::new(FieldType::Table)
            }
        );
    }

    #[test]
    fn unknown_names_are_reported() {
        assert_eq!(def("intger", None).unknown_type_name(), Some("intger"));
        assert_eq!(
            def("array", Some("flaot")).unknown_type_name(),
            Some("flaot")
        );
        assert_eq!(
            FieldSchemaDefinition::Simple("nope".into()).unknown_type_name(),
            Some("nope")
        );
        assert_eq!(def("array", None).unknown_type_name(), None);
        assert_eq!(def("string", None).unknown_type_name(), None);
    }

    /// Every name has one spelling, and an old spelling maps onto one.
    #[test]
    fn each_type_has_one_spelling() {
        for (name, what) in TYPE_NAMES {
            assert_eq!(type_name_of(name), Some(*name));
            assert!(!what.is_empty());
            assert_eq!(
                def(name, None).to_field_schema().field_type.type_name(),
                *name,
                "`{name}` parses to a type that calls itself something else"
            );
        }
        for (old, new) in OLD_TYPE_NAMES {
            assert!(
                TYPE_NAMES.iter().any(|(name, _)| name == new),
                "`{old}` maps to `{new}`, which is no type"
            );
            assert!(!TYPE_NAMES.iter().any(|(name, _)| name == old));
            assert_eq!(type_name_of(old), Some(*new));
        }
        assert_eq!(type_name_of("flaot"), None);
    }

    #[test]
    fn the_new_types_parse() {
        assert_eq!(
            parsed("type = \"entity\"\ndefault = \"\"").field_type,
            FieldType::Entity
        );
        assert_eq!(parsed("type = \"vec2\"").field_type, FieldType::Vec2);
        assert_eq!(parsed("type = \"vec4\"").field_type, FieldType::Vec4);
        assert_eq!(
            parsed("type = \"asset\"\nkind = \"models\"").field_type,
            FieldType::Asset {
                kinds: vec![AssetKind::Models],
                unless: None
            }
        );
        assert_eq!(
            parsed("type = \"asset\"\nkind = [\"models\", \"specs\"]\nunless = \"runtime\"")
                .field_type,
            FieldType::Asset {
                kinds: vec![AssetKind::Models, AssetKind::Specs],
                unless: Some("runtime".into())
            }
        );
        // A kind that is none is dropped; `lint` reports it
        assert_eq!(
            parsed("type = \"asset\"\nkind = \"modles\"").field_type,
            FieldType::Asset {
                kinds: vec![],
                unless: None
            }
        );
    }

    #[test]
    fn a_table_with_fields_and_an_array_of_them_parse() {
        let shear = parsed(
            "type = \"table\"\nfields = { xy = \"f32\", xz = { type = \"f32\", min = -1.0 } }",
        );
        let FieldType::Fields { fields } = &shear.field_type else {
            panic!("{:?}", shear.field_type);
        };
        assert_eq!(fields["xy"].field_type, FieldType::F32);
        assert_eq!(fields["xz"].min, Some(-1.0));
        assert_eq!(shear.field_type.describe(), "table{xy, xz}");

        let layers = parsed(
            "type = \"array\"\nelement = { type = \"table\", fields = { target = \"entity\" } }",
        );
        let FieldType::Array { element } = &layers.field_type else {
            panic!("{:?}", layers.field_type);
        };
        assert!(matches!(
            &**element,
            FieldType::Fields { fields } if fields["target"].field_type == FieldType::Entity
        ));
        assert_eq!(
            parsed("type = \"array\"\nelement = \"vec2\"")
                .field_type
                .describe(),
            "array<vec2>"
        );
    }
}
