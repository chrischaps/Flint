//! `flint gen --list-generators` and `flint gen --describe <generator>`
//! (#718): what a spec's `generator` may name, and every parameter one
//! reads, from the same `param_schema()` the validator checks against and
//! the AI prompt embeds. Both print to stdout only.

use anyhow::{bail, Result};
use flint_procgen::{
    op_ports, op_schema, param_reference_markdown, param_reference_text, Generator,
    GeneratorRegistry, OutputKind, ALL_OP_TYPES,
};
use serde_json::{json, Value};

/// How `--describe` prints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Text,
    Json,
    Markdown,
}

fn kind(kind: OutputKind) -> (&'static str, &'static str) {
    match kind {
        OutputKind::Mesh => ("mesh", "glb"),
        OutputKind::Image => ("image set", "png"),
        OutputKind::Sound => ("sound", "wav"),
    }
}

/// The registry's generators in name order.
fn sorted(registry: &GeneratorRegistry) -> Vec<&dyn Generator> {
    let mut names = registry.generator_names();
    names.sort_unstable();
    names.into_iter().filter_map(|n| registry.get(n)).collect()
}

/// How many top-level parameters a generator takes.
fn param_count(generator: &dyn Generator) -> usize {
    generator
        .param_schema()
        .get("properties")
        .and_then(Value::as_object)
        .map_or(0, |p| p.len())
}

/// One generator's summary, as both listings show it.
fn summary_json(generator: &dyn Generator) -> Value {
    let (output, format) = kind(generator.output_kind());
    json!({
        "name": generator.type_name(),
        "output": output,
        "format": format,
        "revision": generator.revision(),
        "living": generator.as_living().is_some(),
        "params": param_count(generator),
    })
}

/// `--list-generators`: every generator a spec may name.
pub fn list(registry: &GeneratorRegistry, as_json: bool) -> Result<()> {
    let generators = sorted(registry);
    if as_json {
        let list: Vec<Value> = generators.iter().map(|g| summary_json(*g)).collect();
        crate::output::document(&json!({ "generators": list }));
        return Ok(());
    }
    let width = generators
        .iter()
        .map(|g| g.type_name().len())
        .max()
        .unwrap_or(0);
    println!("Generators ({}):", generators.len());
    for g in &generators {
        let (output, format) = kind(g.output_kind());
        let living = if g.as_living().is_some() {
            ", can live"
        } else {
            ""
        };
        println!(
            "  {:<width$}  {output}, written as {}; revision {}; {} parameters{living}",
            g.type_name(),
            format.to_uppercase(),
            g.revision(),
            param_count(*g),
        );
    }
    println!("\n`flint gen --describe <name>` prints a generator's parameters.");
    Ok(())
}

/// Whether the generator's spec takes a texture pipeline's `[[params.ops]]`.
fn has_ops(generator: &dyn Generator) -> bool {
    generator.type_name() == flint_procgen::TextureGenerator.type_name()
}

fn op_json(op: &str) -> Value {
    let ports = op_ports(op);
    json!({
        "type": op,
        "label": ports.as_ref().map(|p| p.label),
        "reads": ports.as_ref().map(|p| p.reads),
        "writes": ports.as_ref().map(|p| p.writes),
        "modifies": ports.as_ref().map(|p| p.modifies),
        "schema": op_schema(op),
    })
}

/// "reads height; writes albedo" for an op's ports.
fn ports_line(op: &str) -> String {
    let Some(p) = op_ports(op) else {
        return String::new();
    };
    let mut parts = Vec::new();
    for (verb, channels) in [
        ("reads", p.reads),
        ("writes", p.writes),
        ("modifies", p.modifies),
    ] {
        if !channels.is_empty() {
            parts.push(format!("{verb} {}", channels.join(", ")));
        }
    }
    parts.join("; ")
}

/// `--describe <name>`: one generator's parameters, and for `texture_v1`
/// each pipeline op with its parameters and ports.
pub fn describe(registry: &GeneratorRegistry, name: &str, format: Format) -> Result<()> {
    let Some(generator) = registry.get(name) else {
        let mut names = registry.generator_names();
        names.sort_unstable();
        let near = names
            .iter()
            .map(|n| (flint_core::toml_util::edit_distance(name, n), n))
            .filter(|(d, _)| *d <= 3)
            .min()
            .map(|(_, n)| format!(" (did you mean '{n}'?)"))
            .unwrap_or_default();
        bail!(
            "no generator named '{name}'{near}\n  known generators: {}",
            names.join(", ")
        );
    };
    let schema = generator.param_schema();
    let (output, file) = kind(generator.output_kind());
    let living = generator.as_living().is_some();
    match format {
        Format::Json => {
            let mut doc = summary_json(generator);
            doc["schema"] = schema;
            if has_ops(generator) {
                doc["ops"] = Value::Array(ALL_OP_TYPES.iter().map(|op| op_json(op)).collect());
            }
            crate::output::document(&doc);
        }
        Format::Markdown => {
            let mut out = format!(
                "### `{name}` ({output}, written as {}; revision {}{})\n\n",
                file.to_uppercase(),
                generator.revision(),
                if living { "; can live" } else { "" }
            );
            out.push_str(&param_reference_markdown(&schema));
            if has_ops(generator) {
                out.push_str("\n#### Pipeline ops (`[[params.ops]]`)\n");
                for op in ALL_OP_TYPES {
                    let label = op_ports(op).map(|p| p.label).unwrap_or(op);
                    out.push_str(&format!("\n##### `{op}` ({label})\n\n"));
                    let ports = ports_line(op);
                    if !ports.is_empty() {
                        out.push_str(&format!("Ports: {ports}.\n\n"));
                    }
                    out.push_str(&param_reference_markdown(&op_schema(op)));
                }
            }
            print!("{out}");
        }
        Format::Text => {
            println!(
                "{name}: {output}, written as {}; revision {}{}",
                file.to_uppercase(),
                generator.revision(),
                if living {
                    "; can live ([living] specs)"
                } else {
                    ""
                }
            );
            println!();
            print!("{}", param_reference_text(&schema));
            if has_ops(generator) {
                println!("\nPipeline ops ([[params.ops]], pattern = \"pipeline\"):");
                for op in ALL_OP_TYPES {
                    let label = op_ports(op).map(|p| p.label).unwrap_or(op);
                    println!("\n{op} ({label})");
                    let ports = ports_line(op);
                    if !ports.is_empty() {
                        println!("  ports: {ports}");
                    }
                    print!("{}", param_reference_text(&op_schema(op)));
                }
            }
            println!("\n`flint gen --describe {name} --json` gives the schema itself.");
        }
    }
    Ok(())
}
