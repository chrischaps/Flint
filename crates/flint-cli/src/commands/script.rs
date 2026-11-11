//! `flint script api` (ADR 0116, #695): every function a script can call.
//!
//! ```bash
//! flint script api                  # by module: signature, return, one-line doc
//! flint script api --filter draw    # names (or modules) containing "draw"
//! flint script api --json           # one JSON document, for agents and the book
//! ```
//!
//! Every engine function is declared through the builder
//! (`flint_script::api::builder`), with named parameters, kinds and a doc.
//! A function a game's extension registers on the Rhai engine directly
//! (`rhai_engine_mut`) is listed from Rhai's metadata as `undeclared`: its
//! Rust parameter types and return, no names or docs. The engine's own
//! listing has none, and prints no such section. The callbacks follow. A
//! game binary with extensions prints its own from
//! `ScriptSystem::api_listing`.

use anyhow::Result;
use flint_script::api::registry::{ApiEntry, ApiListing, UndeclaredFn, CONVENTIONS};
use serde_json::{json, Value};

#[derive(clap::Subcommand)]
pub enum ScriptCommands {
    /// List every script function: name, parameters and their kinds,
    /// return and a one-line doc (types only for the undeclared ones), and
    /// the callbacks
    Api(Box<ApiArgs>),
}

#[derive(clap::Args)]
pub struct ApiArgs {
    /// Print one JSON document instead of text
    #[arg(long)]
    pub json: bool,

    /// Only functions (and callbacks) whose name or module contains this,
    /// ignoring case
    #[arg(long, value_name = "TEXT")]
    pub filter: Option<String>,
}

pub fn run(cmd: ScriptCommands) -> Result<()> {
    match cmd {
        ScriptCommands::Api(args) => api(&args),
    }
}

fn api(args: &ApiArgs) -> Result<()> {
    let listing = flint_script::ScriptSystem::new().api_listing();
    let filter = args.filter.as_deref().map(str::to_lowercase);
    let listing = filtered(listing, filter.as_deref());
    let text = if args.json {
        serde_json::to_string_pretty(&to_json(&listing))?
            + "
"
    } else {
        to_text(&listing)
    };
    // `flint script api | head` closes the pipe early: not an error
    use std::io::Write;
    match std::io::stdout().lock().write_all(text.as_bytes()) {
        Err(e) if e.kind() != std::io::ErrorKind::BrokenPipe => Err(e.into()),
        _ => Ok(()),
    }
}

/// The listing with only what matches `filter` (lowercase).
fn filtered(mut listing: ApiListing, filter: Option<&str>) -> ApiListing {
    let Some(f) = filter else {
        return listing;
    };
    let hit = |s: &str| s.to_lowercase().contains(f);
    listing.declared.retain(|e| hit(e.name) || hit(e.module));
    listing.undeclared.retain(|u| hit(&u.name));
    listing.callbacks.retain(|c| hit(c.name));
    listing
}

fn declared_json(e: &ApiEntry) -> Value {
    json!({
        "module": e.module,
        "name": e.name,
        "declared": true,
        "params": e.params.iter().map(|p| json!({ "name": p.name, "kind": p.kind })).collect::<Vec<_>>(),
        "returns": e.returns.map(|(kind, note)| json!({ "kind": kind, "note": note })),
        "doc": e.doc,
        "typed": e.typed,
        "signature": e.signature(),
    })
}

fn undeclared_json(u: &UndeclaredFn) -> Value {
    json!({
        "module": null,
        "name": u.name,
        "declared": false,
        "params": u.params.iter().map(|t| json!({ "name": null, "type": t })).collect::<Vec<_>>(),
        "returns": { "type": u.returns },
        "doc": null,
        "signature": u.signature,
    })
}

fn to_json(listing: &ApiListing) -> Value {
    let mut functions: Vec<Value> = listing.declared.iter().map(declared_json).collect();
    functions.extend(listing.undeclared.iter().map(undeclared_json));
    json!({
        "counts": {
            "declared": listing.declared.len(),
            "undeclared": listing.undeclared.len(),
            "callbacks": listing.callbacks.len(),
        },
        "undeclared_listed": listing.undeclared_listed,
        "conventions": CONVENTIONS
            .iter()
            .map(|(k, v)| (k.to_string(), Value::from(*v)))
            .collect::<serde_json::Map<_, _>>(),
        "functions": functions,
        "callbacks": listing.callbacks.iter().map(|c| json!({
            "name": c.name,
            "params": c.params,
            "doc": c.doc,
        })).collect::<Vec<_>>(),
    })
}

fn to_text(listing: &ApiListing) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    out.push_str("conventions\n");
    for (name, rule) in CONVENTIONS {
        let _ = writeln!(out, "  {name}: {rule}");
    }
    out.push('\n');
    let mut modules: Vec<&str> = Vec::new();
    for e in &listing.declared {
        if !modules.contains(&e.module) {
            modules.push(e.module);
        }
    }
    for module in modules {
        let entries: Vec<&ApiEntry> = listing
            .declared
            .iter()
            .filter(|e| e.module == module)
            .collect();
        let _ = writeln!(out, "{module} ({} functions)", entries.len());
        for e in entries {
            let typed = if e.typed { "  [typed]" } else { "" };
            let _ = writeln!(out, "  {}{typed}", e.signature());
            let _ = writeln!(out, "      {}", e.doc);
            if let Some((_, note)) = e.returns.filter(|(_, n)| !n.is_empty()) {
                let _ = writeln!(out, "      returns {note}");
            }
        }
        out.push('\n');
    }
    if listing.undeclared_listed {
        if !listing.undeclared.is_empty() {
            let _ = writeln!(
                out,
                "undeclared ({} functions, registered on the Rhai engine directly, not \
                 through the builder: Rust types, no parameter names or docs)",
                listing.undeclared.len()
            );
            for u in &listing.undeclared {
                let _ = writeln!(out, "  {}", u.signature);
            }
            out.push('\n');
        }
    } else {
        out.push_str(
            "undeclared: not listed (flint-script built without its `metadata` feature)\n\n",
        );
    }
    if !listing.callbacks.is_empty() {
        let _ = writeln!(out, "callbacks ({})", listing.callbacks.len());
        for c in &listing.callbacks {
            let _ = writeln!(out, "  fn {}({})", c.name, c.params.join(", "));
            let _ = writeln!(out, "      {}", c.doc);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing() -> ApiListing {
        flint_script::ScriptSystem::new().api_listing()
    }

    #[test]
    fn the_listing_has_every_function_declared_or_not() {
        let l = listing();
        assert!(
            l.undeclared_listed,
            "the CLI builds flint-script with metadata"
        );
        let names: Vec<&str> = l
            .declared
            .iter()
            .map(|e| e.name)
            .chain(l.undeclared.iter().map(|u| u.name.as_str()))
            .collect();
        for name in [
            "set_position",
            "draw_rect",
            "mouse_x",
            "play_sound",
            "random",
        ] {
            assert!(names.contains(&name), "{name} missing");
        }
        assert!(!names.contains(&"mouse_position"), "it never existed");
        let json = to_json(&l);
        let functions = json["functions"].as_array().unwrap();
        let set_position = functions
            .iter()
            .find(|f| f["name"] == "set_position")
            .unwrap();
        assert_eq!(set_position["declared"], true);
        assert_eq!(set_position["params"][1]["kind"], "num");
        assert!(!set_position["doc"].as_str().unwrap().is_empty());
        assert_eq!(
            json["callbacks"].as_array().unwrap().len(),
            l.callbacks.len()
        );
        // The missing-entity rule is stated in the listing itself (#701)
        assert!(json["conventions"]["missing_entity"]
            .as_str()
            .unwrap()
            .contains("warns once"));
        assert!(to_text(&l).starts_with("conventions\n"));
    }

    /// ADR 0116 step 2 ported the last of the engine's functions: only an
    /// extension's raw registration is undeclared, and an empty section
    /// prints nothing.
    #[test]
    fn the_engine_declares_every_function() {
        let l = listing();
        let undeclared: Vec<&str> = l.undeclared.iter().map(|u| u.signature.as_str()).collect();
        assert!(undeclared.is_empty(), "undeclared: {undeclared:?}");
        assert!(!to_text(&l).contains("undeclared"));
        let mut system = flint_script::ScriptSystem::new();
        system
            .rhai_engine_mut()
            .register_fn("game_raw", |n: i64| n + 1);
        let l = system.api_listing();
        assert_eq!(l.undeclared.len(), 1);
        assert!(to_text(&l).contains("undeclared (1 functions"));
    }

    #[test]
    fn a_filter_keeps_only_what_matches() {
        let l = filtered(listing(), Some("material"));
        assert!(!l.declared.is_empty());
        assert!(l.declared.iter().all(|e| e.name.contains("material")));
        assert!(l.undeclared.iter().all(|u| u.name.contains("material")));
        let l = filtered(listing(), Some("entity"));
        assert!(
            l.declared.iter().any(|e| e.name == "set_position"),
            "by module"
        );
        let text = to_text(&filtered(listing(), Some("on_draw")));
        assert!(text.contains("fn on_draw_ui()"), "{text}");
    }
}
