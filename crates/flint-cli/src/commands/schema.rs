//! Schema introspection command
//!
//! ```text
//! flint schema light                      one component or archetype, as text
//! flint schema --list                     every component and archetype
//! flint schema --list --json              ...as JSON
//! flint schema light --format json        one, as data
//! flint schema --format json              everything loaded, as data
//! flint schema --format json-schema       a JSON Schema for a .scene.toml
//! ```
//!
//! A name that is no component or archetype is an error (#684: it used to
//! print "not found" and exit 0).

use super::schema_export;
use anyhow::{bail, Context, Result};
use clap::Args;
use flint_core::diagnostic;
use flint_schema::SchemaRegistry;

#[derive(Args)]
pub struct SchemaArgs {
    /// Component or archetype name. Without one: --list, or --format json /
    /// json-schema for everything loaded
    pub name: Option<String>,

    /// List every component and archetype loaded
    #[arg(long)]
    pub list: bool,

    /// text; json (the schemas as data: every field, type, default and
    /// range); or json-schema (a JSON Schema for a .scene.toml, for taplo
    /// and Even Better TOML; with a component's name, for its table alone)
    #[arg(long, default_value = "text")]
    pub format: String,

    /// The same as --format json
    #[arg(long)]
    pub json: bool,

    /// Write to this file (UTF-8, no byte-order mark) instead of stdout
    #[arg(long, short)]
    pub output: Option<String>,

    /// Schema directories, later ones overriding earlier (repeatable).
    /// Without it: the project's flint.toml `schemas`, else schemas/ in
    /// the working directory
    #[arg(long, action = clap::ArgAction::Append)]
    pub schemas: Vec<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum Format {
    Text,
    Json,
    JsonSchema,
}

pub fn run(args: SchemaArgs) -> Result<()> {
    let format = match (args.format.as_str(), args.json) {
        ("text", true) | ("json", _) => Format::Json,
        ("text", false) => Format::Text,
        ("json-schema", false) => Format::JsonSchema,
        ("json-schema", true) => bail!("--json and --format json-schema are two formats: pass one"),
        (other, _) => bail!("unknown format '{other}': text, json or json-schema"),
    };

    // The one resolver (ADR 0114); no scene, so the working directory's project
    let (registry, dirs) = super::common_args::load_schemas(None, &args.schemas)?;
    if dirs.dirs.is_empty() {
        bail!(
            "No schemas directories found: no --schemas, no `schemas` in a flint.toml, \
             and no schemas/ in the working directory"
        );
    }
    // Named from the working directory where they are under it, so a JSON
    // Schema kept in a repository doesn't carry one machine's folders
    let cwd = std::env::current_dir().ok();
    let shown: Vec<String> = dirs
        .dirs
        .iter()
        .map(|dir| {
            let from_here = cwd.as_deref().and_then(|cwd| dir.strip_prefix(cwd).ok());
            flint_core::project::shown(from_here.unwrap_or(dir))
        })
        .collect();

    let name = args.name.as_deref();
    if args.list && name.is_some() {
        bail!("--list is every component and archetype: leave the name out");
    }
    if let Some(name) = name {
        if registry.get_component(name).is_none() && registry.get_archetype(name).is_none() {
            bail!("{}", not_found(name, &registry));
        }
    }

    let out = match (format, name) {
        (Format::Text, None) if args.list => list_text(&registry, &shown),
        (Format::Text, None) => bail!(
            "name a component or archetype (`flint schema light`), or pass --list for all of \
             them, --format json for the schemas as data, --format json-schema for a JSON \
             Schema of a scene file"
        ),
        (Format::Text, Some(name)) => one_text(name, &registry),
        (Format::Json, None) if args.list => pretty(&schema_export::list_json(&registry, &shown)),
        (Format::Json, None) => pretty(&schema_export::registry_json(&registry, &shown)),
        (Format::Json, Some(name)) => pretty(&match registry.get_component(name) {
            Some(component) => schema_export::component_json(component),
            None => schema_export::archetype_json(registry.get_archetype(name).unwrap()),
        }),
        (Format::JsonSchema, None) if args.list => {
            bail!("--list has no JSON Schema: `flint schema --format json-schema` is the scene's")
        }
        (Format::JsonSchema, None) => pretty(&schema_export::scene_json_schema(&registry, &shown)),
        (Format::JsonSchema, Some(name)) => match registry.get_component(name) {
            Some(component) => pretty(&schema_export::component_schema(component, &registry)),
            None => bail!(
                "`{name}` is an archetype, which has no table of its own to describe: \
                 `flint schema --format json-schema` is the whole scene's"
            ),
        },
    };

    match &args.output {
        Some(path) => {
            std::fs::write(path, &out).with_context(|| format!("Failed to write {path}"))?;
            eprintln!("Wrote {path}");
        }
        None => print!("{out}"),
    }
    Ok(())
}

fn pretty(value: &serde_json::Value) -> String {
    let mut text = serde_json::to_string_pretty(value).expect("JSON values serialize");
    text.push('\n');
    text
}

/// The error for a name that is neither: the nearest name, and where the
/// whole list is.
fn not_found(name: &str, registry: &SchemaRegistry) -> String {
    let names = registry
        .component_names()
        .into_iter()
        .chain(registry.archetype_names());
    let mut message = format!("no component or archetype named `{name}`");
    if let Some(near) = diagnostic::did_you_mean(name, names) {
        message.push_str(&format!(": {near}"));
    }
    message.push_str("\n`flint schema --list` names every one loaded");
    message
}

/// The first sentence or line of a description, to fit a list.
fn brief(description: Option<&str>) -> String {
    let text = description.unwrap_or("").trim();
    let line = text.lines().next().unwrap_or("");
    let sentence = &line[..line.find(". ").map(|at| at + 1).unwrap_or(line.len())];
    let mut brief: String = sentence.chars().take(96).collect();
    if brief.len() < sentence.len() {
        brief.push_str("...");
    }
    brief
}

fn list_text(registry: &SchemaRegistry, dirs: &[String]) -> String {
    let components = registry.components();
    let archetypes = registry.archetypes();
    let width = components
        .iter()
        .map(|c| c.name.len())
        .chain(archetypes.iter().map(|a| a.name.len()))
        .max()
        .unwrap_or(0);
    let mut out = format!("Schemas from: {}\n\n", dirs.join(", "));
    out.push_str(&format!("Components ({}):\n", components.len()));
    for c in &components {
        let fields = match c.fields.len() {
            0 => "open".to_string(),
            1 => "1 field".to_string(),
            n => format!("{n} fields"),
        };
        out.push_str(
            format!(
                "  {:width$}  {:>9}  {}",
                c.name,
                fields,
                brief(c.description.as_deref())
            )
            .trim_end(),
        );
        out.push('\n');
    }
    out.push_str(&format!("\nArchetypes ({}):\n", archetypes.len()));
    for a in &archetypes {
        out.push_str(&format!(
            "  {:width$}  {}\n",
            a.name,
            a.components.join(", ")
        ));
    }
    out.push_str(
        "\n`open` is a component with no fields declared: its name is known and nothing in \
         it is checked.\n`flint schema <name>` shows one; `--format json` gives all of it as \
         data.\n",
    );
    out
}

fn one_text(name: &str, registry: &SchemaRegistry) -> String {
    let mut out = String::new();
    // Try component first, then archetype
    if let Some(component) = registry.get_component(name) {
        out.push_str(&format!("Component: {}\n", component.name));
        if let Some(desc) = &component.description {
            out.push_str(&format!("Description: {desc}\n"));
        }
        out.push_str("\nFields:\n");
        if component.is_open() {
            out.push_str(
                "  (none declared: the component is open, and nothing in it is checked)\n",
            );
        }

        let mut fields: Vec<_> = component.fields.iter().collect();
        fields.sort_by_key(|(name, _)| *name);

        for (field_name, field_schema) in fields {
            let required = if field_schema.required {
                " (required)"
            } else {
                ""
            };
            out.push_str(&format!(
                "  {} : {}{}",
                field_name,
                field_schema.field_type.describe(),
                required
            ));
            if let Some(default) = &field_schema.default {
                out.push_str(&format!(" = {}", format_toml_value(default)));
            }
            out.push('\n');

            if let Some(desc) = &field_schema.description {
                out.push_str(&format!("    # {desc}\n"));
            }

            if field_schema.min.is_some() || field_schema.max.is_some() {
                let min = field_schema.min.map(|v| v.to_string()).unwrap_or_default();
                let max = field_schema.max.map(|v| v.to_string()).unwrap_or_default();
                out.push_str(&format!("    # Range: {min} .. {max}\n"));
            }
        }
        return out;
    }

    let archetype = registry
        .get_archetype(name)
        .expect("the caller checked the name");
    out.push_str(&format!("Archetype: {}\n", archetype.name));
    if let Some(desc) = &archetype.description {
        out.push_str(&format!("Description: {desc}\n"));
    }
    out.push_str("\nComponents:\n");
    for comp_name in &archetype.components {
        let exists = if registry.get_component(comp_name).is_some() {
            ""
        } else {
            " (not found)"
        };
        out.push_str(&format!("  - {comp_name}{exists}\n"));
    }

    if !archetype.defaults.is_empty() {
        out.push_str("\nDefaults:\n");
        let mut defaults: Vec<_> = archetype.defaults.iter().collect();
        defaults.sort_by_key(|(name, _)| *name);
        for (comp_name, defaults) in defaults {
            out.push_str(&format!("  [{comp_name}]\n"));
            if let Some(table) = defaults.as_table() {
                for (k, v) in table {
                    out.push_str(&format!("    {} = {}\n", k, format_toml_value(v)));
                }
            } else {
                out.push_str(&format!("    {}\n", format_toml_value(defaults)));
            }
        }
    }
    out
}

fn format_toml_value(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => format!("\"{}\"", s),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => f.to_string(),
        toml::Value::Boolean(b) => b.to_string(),
        toml::Value::Array(arr) => {
            let items: Vec<String> = arr.iter().map(format_toml_value).collect();
            format!("[{}]", items.join(", "))
        }
        toml::Value::Table(t) => {
            let items: Vec<String> = t
                .iter()
                .map(|(k, v)| format!("{} = {}", k, format_toml_value(v)))
                .collect();
            format!("{{ {} }}", items.join(", "))
        }
        toml::Value::Datetime(d) => d.to_string(),
    }
}
