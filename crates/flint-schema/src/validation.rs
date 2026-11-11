//! Validation of component data against schemas
//!
//! [`validate_component`] is the whole answer (ADR 0115): every problem in
//! one component table, each a [`Diagnostic`] naming the field by its dotted
//! path. It never stops at the first. [`validate_component_data`] is the
//! older yes-or-no form over it, for a caller that wants one error.

use crate::component::{ComponentSchema, FieldType};
use flint_core::diagnostic::{self, codes, Diagnostic};
use flint_core::{FlintError, Result};

/// Everything wrong with `data` as a `schema` component, in a stable order:
/// required fields that are missing (by name), then each written field (by
/// name). Each diagnostic carries the component's name and the field's
/// dotted path (`friction`, `points[2]`, `position[1]`); the caller adds the
/// entity and the file.
///
/// A field the schema doesn't declare is a **warning** with the nearest
/// declared name as its hint: games add fields to engine components, and
/// scripts read them. A wrong type, a string that is none of an enum's
/// values (the hint lists them), a number outside `min`/`max` and a missing
/// required field are errors.
///
/// A schema that declares **no fields** is open: it makes the component's
/// name known and nothing in its table is checked. That is how a game
/// declares a component only its scripts read
/// (`[component.inventory]` and a description).
pub fn validate_component(schema: &ComponentSchema, data: &toml::Value) -> Vec<Diagnostic> {
    problems(schema, data)
        .into_iter()
        .map(|problem| problem.diagnostic(schema))
        .collect()
}

/// Validate component data against its schema: `Err` with the first error
/// [`validate_component`] finds (a missing required field before a bad
/// value). An undeclared field is not an error here.
pub fn validate_component_data(schema: &ComponentSchema, data: &toml::Value) -> Result<()> {
    match problems(schema, data)
        .into_iter()
        .find(|problem| !matches!(problem, Problem::Unknown { .. }))
    {
        Some(problem) => Err(problem.into_error()),
        None => Ok(()),
    }
}

/// One thing wrong, kept typed so it can become a [`Diagnostic`] or the
/// [`FlintError`] older callers match on.
enum Problem {
    NotATable {
        got: &'static str,
    },
    Missing {
        field: String,
    },
    Unknown {
        field: String,
    },
    BadType {
        field: String,
        expected: String,
        got: &'static str,
    },
    BadEnum {
        field: String,
        value: String,
        allowed: Vec<String>,
    },
    OutOfRange {
        field: String,
        min: Option<f64>,
        max: Option<f64>,
        value: f64,
    },
    /// The right TOML type in the wrong shape: a vec3 of two numbers.
    Shape {
        field: String,
        message: String,
    },
}

impl Problem {
    fn diagnostic(self, schema: &ComponentSchema) -> Diagnostic {
        let d = match self {
            Problem::NotATable { got } => Diagnostic::error(
                codes::BAD_TYPE,
                format!("a component is a table, got {got}"),
            ),
            Problem::Missing { field } => Diagnostic::error(
                codes::MISSING_FIELD,
                format!("required field `{field}` is missing"),
            )
            .field(field),
            Problem::Unknown { field } => {
                let hint =
                    diagnostic::did_you_mean(&field, schema.fields.keys().map(String::as_str))
                        .unwrap_or_else(|| {
                            format!(
                                "the fields of `{}` are {}; a game's own field is declared in its \
                             schema layer",
                                schema.name,
                                listed(schema.fields.keys())
                            )
                        });
                Diagnostic::warning(
                    codes::UNKNOWN_FIELD,
                    format!("`{}` has no field `{field}`", schema.name),
                )
                .field(field)
                .hint(hint)
            }
            Problem::BadType {
                field,
                expected,
                got,
            } => Diagnostic::error(codes::BAD_TYPE, format!("expected {expected}, got {got}"))
                .field(field),
            Problem::BadEnum {
                field,
                value,
                allowed,
            } => {
                let values = format!("allowed values: {}", listed(allowed.iter()));
                let hint =
                    match diagnostic::did_you_mean(&value, allowed.iter().map(String::as_str)) {
                        Some(near) => format!("{near} {values}"),
                        None => values,
                    };
                Diagnostic::error(
                    codes::BAD_ENUM_VALUE,
                    format!("\"{value}\" is not a value of `{field}`"),
                )
                .field(field)
                .hint(hint)
            }
            Problem::OutOfRange {
                field,
                min,
                max,
                value,
            } => {
                let message = match (min, max) {
                    (Some(min), _) if value < min => format!("{value} is below the minimum {min}"),
                    (_, Some(max)) => format!("{value} is above the maximum {max}"),
                    _ => format!("{value} is out of range"),
                };
                let hint = match (min, max) {
                    (Some(min), Some(max)) => Some(format!("the range is {min} to {max}")),
                    _ => None,
                };
                Diagnostic::error(codes::OUT_OF_RANGE, message)
                    .field(field)
                    .hint_opt(hint)
            }
            Problem::Shape { field, message } => {
                Diagnostic::error(codes::BAD_TYPE, message).field(field)
            }
        };
        d.component(schema.name.clone())
    }

    fn into_error(self) -> FlintError {
        match self {
            Problem::NotATable { .. } => {
                FlintError::ValidationError("Component data must be a table".to_string())
            }
            Problem::Missing { field } => FlintError::MissingRequiredField(field),
            Problem::Unknown { field } => {
                FlintError::ValidationError(format!("Field '{field}': not in the schema"))
            }
            Problem::BadType {
                field,
                expected,
                got,
            } => FlintError::InvalidFieldType {
                field,
                expected,
                got: got.to_string(),
            },
            Problem::BadEnum {
                field,
                value,
                allowed,
            } => FlintError::InvalidEnumValue {
                field,
                value,
                allowed,
            },
            Problem::OutOfRange {
                field,
                min,
                max,
                value,
            } => FlintError::ValueOutOfRange {
                field,
                min: min.unwrap_or(f64::MIN),
                max: max.unwrap_or(f64::MAX),
                value,
            },
            Problem::Shape { field, message } => {
                FlintError::ValidationError(format!("Field '{field}': {message}"))
            }
        }
    }
}

/// `a, b, c` in name order.
fn listed<'a>(names: impl Iterator<Item = &'a String>) -> String {
    let mut names: Vec<&str> = names.map(String::as_str).collect();
    names.sort_unstable();
    names.join(", ")
}

fn problems(schema: &ComponentSchema, data: &toml::Value) -> Vec<Problem> {
    let Some(table) = data.as_table() else {
        return vec![Problem::NotATable {
            got: value_type_name(data),
        }];
    };
    let mut found = Vec::new();

    // Required fields that are not there, by name (the schema is a HashMap)
    let mut missing: Vec<&String> = schema
        .fields
        .iter()
        .filter(|(name, field)| field.required && !table.contains_key(*name))
        .map(|(name, _)| name)
        .collect();
    missing.sort_unstable();
    found.extend(missing.into_iter().map(|field| Problem::Missing {
        field: field.clone(),
    }));

    // Each written field, by name
    let mut written: Vec<(&String, &toml::Value)> = table.iter().collect();
    written.sort_unstable_by_key(|(name, _)| *name);
    for (field_name, value) in written {
        match schema.fields.get(field_name) {
            Some(field_schema) => check_value(
                field_name,
                &field_schema.field_type,
                value,
                field_schema.min,
                field_schema.max,
                &mut found,
            ),
            // A schema that declares no field at all is open: it names
            // the component and checks nothing in it
            None if schema.fields.is_empty() => {}
            None => found.push(Problem::Unknown {
                field: field_name.clone(),
            }),
        }
    }
    found
}

fn check_value(
    field_name: &str,
    field_type: &FieldType,
    value: &toml::Value,
    min: Option<f64>,
    max: Option<f64>,
    found: &mut Vec<Problem>,
) {
    let shape = |message: &str| Problem::Shape {
        field: field_name.to_string(),
        message: message.to_string(),
    };
    match (field_type, value) {
        (FieldType::Bool, toml::Value::Boolean(_)) => {}
        (FieldType::I32 | FieldType::I64, toml::Value::Integer(n)) => {
            check_range(field_name, *n as f64, min, max, found)
        }
        (FieldType::F32 | FieldType::F64, toml::Value::Float(n)) => {
            check_range(field_name, *n, min, max, found)
        }
        // Integers are allowed where floats are expected
        (FieldType::F32 | FieldType::F64, toml::Value::Integer(n)) => {
            check_range(field_name, *n as f64, min, max, found)
        }
        (FieldType::String, toml::Value::String(_)) => {}
        // Vec3 can be {x, y, z} or an array [x, y, z]
        (FieldType::Vec3, toml::Value::Table(t)) => {
            if ["x", "y", "z"].iter().all(|k| t.contains_key(*k)) {
                for key in ["x", "y", "z"] {
                    check_number(&format!("{field_name}.{key}"), &t[key], found);
                }
            } else {
                found.push(shape("a vec3 table needs x, y and z"));
            }
        }
        (FieldType::Vec3, toml::Value::Array(arr)) => {
            if arr.len() == 3 {
                for (i, item) in arr.iter().enumerate() {
                    check_number(&format!("{field_name}[{i}]"), item, found);
                }
            } else {
                found.push(shape(&format!("a vec3 is 3 numbers, got {}", arr.len())));
            }
        }
        // Transform needs position, rotation, scale - all optional with defaults
        (FieldType::Transform, toml::Value::Table(t)) => {
            for key in ["position", "rotation", "scale"] {
                if let Some(v) = t.get(key) {
                    check_value(
                        &format!("{field_name}.{key}"),
                        &FieldType::Vec3,
                        v,
                        None,
                        None,
                        found,
                    );
                }
            }
        }
        // Color can be {r, g, b, a}
        (FieldType::Color, toml::Value::Table(t)) => {
            if !["r", "g", "b"].iter().all(|k| t.contains_key(*k)) {
                found.push(shape("a color table needs r, g and b"));
            }
        }
        // ...or a hex color like 0xFF8844
        (FieldType::Color, toml::Value::Integer(hex)) => {
            if !(0..=0xFFFFFF).contains(hex) {
                found.push(shape("a hex color is 0x000000 to 0xFFFFFF"));
            }
        }
        (FieldType::Enum { values }, toml::Value::String(s)) => {
            if !values.contains(s) {
                found.push(Problem::BadEnum {
                    field: field_name.to_string(),
                    value: s.clone(),
                    allowed: values.clone(),
                });
            }
        }
        (FieldType::Table, toml::Value::Table(_)) => {}
        (FieldType::Array { element }, toml::Value::Array(arr)) => {
            for (i, item) in arr.iter().enumerate() {
                check_value(
                    &format!("{field_name}[{i}]"),
                    element,
                    item,
                    None,
                    None,
                    found,
                );
            }
        }
        _ => found.push(Problem::BadType {
            field: field_name.to_string(),
            expected: field_type.type_name().to_string(),
            got: value_type_name(value),
        }),
    }
}

fn check_number(field_name: &str, value: &toml::Value, found: &mut Vec<Problem>) {
    if !matches!(value, toml::Value::Integer(_) | toml::Value::Float(_)) {
        found.push(Problem::BadType {
            field: field_name.to_string(),
            expected: "a number".to_string(),
            got: value_type_name(value),
        });
    }
}

fn check_range(
    field_name: &str,
    value: f64,
    min: Option<f64>,
    max: Option<f64>,
    found: &mut Vec<Problem>,
) {
    let below = min.is_some_and(|min| value < min);
    let above = max.is_some_and(|max| value > max);
    if below || above {
        found.push(Problem::OutOfRange {
            field: field_name.to_string(),
            min,
            max,
            value,
        });
    }
}

fn value_type_name(value: &toml::Value) -> &'static str {
    match value {
        toml::Value::String(_) => "string",
        toml::Value::Integer(_) => "integer",
        toml::Value::Float(_) => "float",
        toml::Value::Boolean(_) => "bool",
        toml::Value::Datetime(_) => "datetime",
        toml::Value::Array(_) => "array",
        toml::Value::Table(_) => "table",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::FieldSchema;
    use std::collections::HashMap;

    fn make_test_schema() -> ComponentSchema {
        let mut fields = HashMap::new();
        fields.insert(
            "locked".to_string(),
            FieldSchema {
                field_type: FieldType::Bool,
                default: None,
                required: false,
                description: None,
                min: None,
                max: None,
            },
        );
        fields.insert(
            "angle".to_string(),
            FieldSchema {
                field_type: FieldType::F32,
                default: None,
                required: false,
                description: None,
                min: Some(0.0),
                max: Some(180.0),
            },
        );
        fields.insert(
            "style".to_string(),
            FieldSchema {
                field_type: FieldType::Enum {
                    values: vec!["hinged".to_string(), "sliding".to_string()],
                },
                default: None,
                required: true,
                description: None,
                min: None,
                max: None,
            },
        );

        ComponentSchema {
            name: "door".to_string(),
            description: None,
            fields,
        }
    }

    #[test]
    fn test_valid_data() {
        let schema = make_test_schema();
        let data: toml::Value = toml::from_str(
            r#"
            locked = true
            angle = 90.0
            style = "hinged"
            "#,
        )
        .unwrap();

        assert!(validate_component_data(&schema, &data).is_ok());
    }

    #[test]
    fn test_missing_required_field() {
        let schema = make_test_schema();
        let data: toml::Value = toml::from_str(
            r#"
            locked = true
            "#,
        )
        .unwrap();

        assert!(matches!(
            validate_component_data(&schema, &data),
            Err(FlintError::MissingRequiredField(_))
        ));
    }

    #[test]
    fn test_invalid_enum_value() {
        let schema = make_test_schema();
        let data: toml::Value = toml::from_str(
            r#"
            style = "rotating"
            "#,
        )
        .unwrap();

        assert!(matches!(
            validate_component_data(&schema, &data),
            Err(FlintError::InvalidEnumValue { .. })
        ));
    }

    #[test]
    fn test_value_out_of_range() {
        let schema = make_test_schema();
        let data: toml::Value = toml::from_str(
            r#"
            style = "hinged"
            angle = 200.0
            "#,
        )
        .unwrap();

        assert!(matches!(
            validate_component_data(&schema, &data),
            Err(FlintError::ValueOutOfRange { .. })
        ));
    }

    use flint_core::Severity;
    fn field(field_type: FieldType) -> FieldSchema {
        FieldSchema {
            field_type,
            default: None,
            required: false,
            description: None,
            min: None,
            max: None,
        }
    }

    fn check(toml_text: &str) -> Vec<Diagnostic> {
        let mut schema = make_test_schema();
        schema
            .fields
            .insert("hinge".to_string(), field(FieldType::Vec3));
        schema.fields.insert(
            "stops".to_string(),
            field(FieldType::Array {
                element: Box::new(FieldType::F32),
            }),
        );
        let data: toml::Value = toml::from_str(toml_text).unwrap();
        validate_component(&schema, &data)
    }

    /// The whole point of ADR 0115: one pass, every problem, each naming its
    /// field. The order is fixed: missing fields, then written ones by name.
    #[test]
    fn every_problem_is_reported_with_its_field() {
        let found = check(
            r#"
            angle = 200.0
            lockd = true
            locked = "yes"
            hinge = [0, "up", 0]
            stops = [10.0, "wide"]
            "#,
        );
        let said: Vec<(&str, &str, Severity)> = found
            .iter()
            .map(|d| (d.code, d.field.as_deref().unwrap(), d.severity))
            .collect();
        assert_eq!(
            said,
            [
                (codes::MISSING_FIELD, "style", Severity::Error),
                (codes::OUT_OF_RANGE, "angle", Severity::Error),
                (codes::BAD_TYPE, "hinge[1]", Severity::Error),
                (codes::UNKNOWN_FIELD, "lockd", Severity::Warning),
                (codes::BAD_TYPE, "locked", Severity::Error),
                (codes::BAD_TYPE, "stops[1]", Severity::Error),
            ]
        );
        assert!(found.iter().all(|d| d.component.as_deref() == Some("door")));
        assert_eq!(found[1].message, "200 is above the maximum 180");
        assert_eq!(found[1].hint.as_deref(), Some("the range is 0 to 180"));
        assert_eq!(found[4].message, "expected bool, got string");
        assert_eq!(found[5].message, "expected f32, got string");
    }

    #[test]
    fn an_unknown_field_is_a_warning_with_the_nearest_name() {
        let found = check("style = \"hinged\"\nlockd = true\n");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].severity, Severity::Warning);
        assert_eq!(found[0].code, codes::UNKNOWN_FIELD);
        assert_eq!(found[0].message, "`door` has no field `lockd`");
        assert_eq!(found[0].hint.as_deref(), Some("did you mean `locked`?"));
        // ...and the yes-or-no form still passes it: a game's own field
        let data: toml::Value = toml::from_str("style = \"hinged\"\nlockd = true\n").unwrap();
        assert!(validate_component_data(&make_test_schema(), &data).is_ok());
    }

    /// With nothing near, the hint is the fields there are and what to do
    /// about a field of the game's own.
    #[test]
    fn an_unknown_field_like_no_other_lists_the_fields() {
        let found = check("style = \"hinged\"\nhealth = 3\n");
        let hint = found[0].hint.as_deref().unwrap();
        assert!(
            hint.starts_with("the fields of `door` are angle, hinge, locked, stops, style;"),
            "{hint}"
        );
        assert!(hint.contains("schema layer"), "{hint}");
    }

    #[test]
    fn a_bad_enum_value_lists_the_allowed_ones() {
        let found = check("style = \"slidng\"\n");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, codes::BAD_ENUM_VALUE);
        assert_eq!(found[0].severity, Severity::Error);
        assert_eq!(found[0].field.as_deref(), Some("style"));
        assert_eq!(found[0].message, "\"slidng\" is not a value of `style`");
        assert_eq!(
            found[0].hint.as_deref(),
            Some("did you mean `sliding`? allowed values: hinged, sliding")
        );
        let far = check("style = \"rotating\"\n");
        assert_eq!(
            far[0].hint.as_deref(),
            Some("allowed values: hinged, sliding")
        );
    }

    #[test]
    fn a_wrong_shape_is_a_bad_type() {
        let found = check("style = \"hinged\"\nhinge = [0, 1]\n");
        assert_eq!(found[0].code, codes::BAD_TYPE);
        assert_eq!(found[0].field.as_deref(), Some("hinge"));
        assert_eq!(found[0].message, "a vec3 is 3 numbers, got 2");
        let not_a_table = validate_component(&make_test_schema(), &toml::Value::Integer(3));
        assert_eq!(not_a_table.len(), 1);
        assert_eq!(not_a_table[0].code, codes::BAD_TYPE);
        assert_eq!(not_a_table[0].field, None);
    }

    /// A schema with no fields names a component and checks nothing in it:
    /// a game's script-only component, declared in one line.
    #[test]
    fn a_schema_with_no_fields_is_open() {
        let schema = ComponentSchema {
            name: "inventory".to_string(),
            description: None,
            fields: HashMap::new(),
        };
        let data: toml::Value = toml::from_str("slots = 4\nitems = [\"key\"]\n").unwrap();
        assert_eq!(validate_component(&schema, &data), []);
        // ...but it is still a table
        assert_eq!(
            validate_component(&schema, &toml::Value::Boolean(true)).len(),
            1
        );
    }

    /// The old error variants name their field now (#683).
    #[test]
    fn the_first_error_names_its_field() {
        let schema = make_test_schema();
        let data: toml::Value = toml::from_str("style = \"hinged\"\nlocked = 3\n").unwrap();
        let err = validate_component_data(&schema, &data).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Invalid field type: `locked` expected bool, got integer"
        );
        let data: toml::Value = toml::from_str("style = \"rotating\"\n").unwrap();
        let err = validate_component_data(&schema, &data).unwrap_err();
        assert!(
            matches!(&err, FlintError::InvalidEnumValue { field, .. } if field == "style"),
            "{err}"
        );
    }
}
