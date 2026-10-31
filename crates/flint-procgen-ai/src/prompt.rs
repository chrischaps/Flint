//! The words [`crate::AnthropicAgent`] sends: one system prompt per
//! generator (stable, so the prompt cache can hold it) and one user turn per
//! task. Kept apart from the HTTP code so tests can read them whole.

use flint_procgen::{Generator, ProcGenSpec};

use crate::agent::ProcGenAgent;
use crate::MockAgent;

/// The system prompt for one generator: what a spec is, the generator's
/// param schema verbatim, a valid example, and the reply contract.
///
/// Built only from the generator, never from the request, so every call for
/// the same generator shares a cacheable prefix.
pub fn system_prompt(name: &str, generator: &dyn Generator) -> String {
    let schema = serde_json::to_string_pretty(&generator.param_schema())
        .unwrap_or_else(|_| "{}".to_string());
    let example = example_params(name);
    format!(
        r##"You tune procedural-generation specs for Flint, a game engine. A spec is a TOML file that a deterministic generator turns into an asset: the same spec and seed always give the same bytes, so every number you choose is exactly what gets built.

You are writing for the `{name}` generator, which produces a {kind:?}.

## Spec format

```toml
generator = "{name}"
[meta]      # name, description, version, tags
[seed]      # mode = "fixed", value = <integer>
[params]    # this generator's parameters, described below
```

You write only the params (plus a name and tags when asked). The engine fills in the rest.

## Parameter schema (JSON Schema)

```json
{schema}
```

Rules for params:
- Use only the properties listed above. The generator ignores any other key and the validator rejects it.
- Stay inside every minimum, maximum, exclusiveMinimum, enum and pattern.
- A property of type `number` is a float: write 1.0, not 1. An `integer` is a whole number.
- Colours are "#RRGGBB" strings.
- A param you leave out takes its default. Set the ones that carry the request; repeating a default is fine.
- Arrays and objects must keep the shape the schema and the example show.

## A valid example

```toml
{example}
```

## Replying

Reply with one JSON object and nothing else: no prose before or after it and no code fence. Each request names the keys it needs. If a validator report comes back, fix every issue it lists and send the complete JSON object again, not just the changes."##,
        kind = generator.output_kind(),
    )
}

/// A `[params]` table the generator accepts, written by the mock agent from
/// the schema's own defaults (and, for creatures, its template).
fn example_params(name: &str) -> String {
    let params = MockAgent
        .create_spec_from_prompt("a typical example", name)
        .map(|s| s.params)
        .unwrap_or_else(|_| toml::Value::Table(Default::default()));
    let mut wrapper = toml::map::Map::new();
    wrapper.insert("params".to_string(), params);
    toml::to_string(&toml::Value::Table(wrapper)).unwrap_or_default()
}

/// The user turn for `create_spec_from_prompt`.
pub fn create_task(prompt: &str) -> String {
    format!(
        r#"Write a spec for this request:

"{prompt}"

Reply with:
{{
  "name": "short_snake_case_name",
  "tags": ["three", "to", "six", "lowercase", "words"],
  "params": {{ ...the params that make this asset... }},
  "rationale": "one or two sentences: which params carry the request, and why those values"
}}"#
    )
}

/// The user turn for `interpret_spec`. `findings` are the validator's own
/// results for the spec as it stands, so the review starts from facts.
pub fn interpret_task(spec: &ProcGenSpec, findings: &[String]) -> String {
    let text = toml::to_string(spec).unwrap_or_default();
    let found = if findings.is_empty() {
        "The validator found no problems: the params fit the schema and the generator builds them."
            .to_string()
    } else {
        format!(
            "The validator found these problems:\n{}",
            findings
                .iter()
                .map(|f| format!("- {f}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    format!(
        r#"Review this spec.

```toml
{text}
```

{found}

Picture the asset these params build and judge it against the spec's description (or, without one, against its name and tags). Say what will look wrong or weak, and how to fix it. Each suggestion must name the param, its current value and the value you would use, and say what the change does to the asset. Do not write generic advice.

Reply with:
{{
  "suggestions": ["`param` a -> b: what changes and why", "..."],
  "confidence": 0.0 to 1.0, how well the spec as it stands achieves its description,
  "params": {{ ...the full revised params... }} or null if nothing should change
}}"#
    )
}

/// The user turn for `refine_spec`. The images, when there are any, come
/// before this text in the same turn.
///
/// `measured` is the average colour of the reference and of the current
/// output, when both are images.
pub fn refine_task(
    spec: &ProcGenSpec,
    reference: bool,
    current: bool,
    measured: Option<([u8; 3], [u8; 3])>,
) -> String {
    let text = toml::to_string(spec).unwrap_or_default();
    let hex = |c: [u8; 3]| format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2]);
    let measured = match measured {
        Some((r, c)) => format!(
            "\n\nMeasured average colour of the whole image: reference {} {:?}, current output {} {:?}. Overall tone is hard to judge by eye in a small image; trust these numbers for it, and the images for pattern and structure.",
            hex(r), r, hex(c), c
        ),
        None => String::new(),
    };
    let goal = match (reference, current) {
        (true, true) => "The first image is the reference. The second is what this spec generates now. Adjust the params so the generated asset looks more like the reference: its colours, proportions, density and texture.",
        (true, false) => "The image is the reference. Adjust the params so the asset this spec generates looks more like it: its colours, proportions, density and silhouette.",
        (false, _) => "There is no reference image. Adjust the params so the asset better matches the spec's description (or its name and tags).",
    };
    format!(
        r#"{goal}{measured}

```toml
{text}
```

Change what the goal needs and leave the rest alone.

Reply with:
{{
  "params": {{ ...the full refined params... }},
  "changes": ["`param` a -> b: what it moves toward", "..."]
}}"#
    )
}

/// The user turn sent back when a reply fails validation.
pub fn feedback(round: u32, issues: &[String]) -> String {
    format!(
        "Validator report for round {round}: the reply was not accepted.\n{}\n\nFix every issue and reply again with the complete JSON object.",
        issues
            .iter()
            .map(|i| format!("- {i}"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::registry;

    #[test]
    fn system_prompt_holds_the_schema_the_format_and_a_valid_example() {
        let registry = registry();
        for name in ["tree_v1", "texture_v1", "creature_v1"] {
            let g = registry.get(name).unwrap();
            let p = system_prompt(name, g);
            // The schema, verbatim.
            let schema = serde_json::to_string_pretty(&g.param_schema()).unwrap();
            assert!(p.contains(&schema), "{name}: schema missing");
            // The spec format and the reply contract.
            assert!(p.contains(&format!("generator = \"{name}\"")), "{name}");
            assert!(p.contains("[params]"));
            assert!(p.contains("Reply with one JSON object"));
            // The example parses and fits the schema.
            let start = p.find("## A valid example").unwrap();
            let block = &p[start..];
            let toml_start = block.find("```toml\n").unwrap() + 8;
            let toml_end = block[toml_start..].find("```").unwrap() + toml_start;
            let example: toml::Value = toml::from_str(&block[toml_start..toml_end]).unwrap();
            let issues = crate::check_params(&g.param_schema(), &example["params"]);
            assert!(issues.is_empty(), "{name}: {issues:?}");
        }
    }

    #[test]
    fn system_prompt_is_stable_per_generator() {
        let registry = registry();
        let g = registry.get("tree_v1").unwrap();
        assert_eq!(system_prompt("tree_v1", g), system_prompt("tree_v1", g));
    }

    #[test]
    fn tasks_carry_their_inputs() {
        assert!(create_task("tall pine tree with sparse branches")
            .contains("\"tall pine tree with sparse branches\""));
        let spec = MockAgent
            .create_spec_from_prompt("tall oak", "tree")
            .unwrap();
        let t = interpret_task(&spec, &["`trunk_height` = -2 is outside (0, 50]".into()]);
        assert!(t.contains("- `trunk_height` = -2"));
        assert!(t.contains("generator = \"tree_v1\""));
        assert!(interpret_task(&spec, &[]).contains("found no problems"));
        assert!(
            refine_task(&spec, true, true, None).starts_with("The first image is the reference")
        );
        assert!(refine_task(&spec, false, false, None).contains("no reference image"));
        let m = refine_task(&spec, true, true, Some(([113, 88, 68], [10, 200, 255])));
        assert!(
            m.contains("reference #715844 [113, 88, 68], current output #0AC8FF"),
            "{m}"
        );
    }
}
