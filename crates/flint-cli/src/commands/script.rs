//! `flint script api` (ADR 0116, #695): every function a script can call.
//! `flint script check` (#694): every script held to it.
//!
//! ```bash
//! flint script api                  # by module: signature, return, one-line doc
//! flint script api --filter draw    # names (or modules) containing "draw"
//! flint script api --json           # one JSON document, for agents and the book
//! flint script api --book           # write the book's generated reference and tables
//! flint script api --check-book     # exit 1 if they're stale or a chapter calls a ghost (CI)
//! flint script check examples/      # every script of a folder: file:line:col and a hint
//! flint script check level.scene.toml --json
//! flint script check . --api game-api.json --allow game_only_fn
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

use anyhow::{bail, Result};
use flint_script::api::registry::{ApiEntry, ApiListing, UndeclaredFn, CONVENTIONS};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub mod book;

#[derive(clap::Subcommand)]
pub enum ScriptCommands {
    /// List every script function: name, parameters and their kinds,
    /// return and a one-line doc (types only for the undeclared ones), and
    /// the callbacks
    Api(Box<ApiArgs>),
    /// Compile every script a project or scene runs, and the modules they
    /// import, and check each call's name and argument count against the
    /// script API, each callback's name and parameters, and each import.
    /// Exit 1 on an error
    Check(Box<CheckArgs>),
}

#[derive(clap::Args)]
pub struct CheckArgs {
    /// A project folder (every script under it, each scene's with the
    /// scene's search), one scene file (its scripts), or one .rhai file
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Print one JSON document instead of text, in `flint check`'s shape
    #[arg(long)]
    pub json: bool,

    /// Schema directories (repeatable), for loading the scenes whose
    /// scripts are checked. Without it: the project's, as `flint check`
    #[arg(long, action = clap::ArgAction::Append)]
    pub schemas: Vec<String>,

    #[command(flatten)]
    pub script: crate::check::scripts::ScriptApiArgs,
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

    /// Write the book's generated script API reference
    /// (`src/reference/script-api.md`) and every `script-api` region in its
    /// chapters, then report calls in the chapters that name no function.
    /// DIR is the book (the folder holding `src/`); default `docs/book`
    #[arg(long, value_name = "DIR", num_args = 0..=1, default_missing_value = "docs/book",
          conflicts_with_all = ["json", "filter", "check_book"])]
    pub book: Option<PathBuf>,

    /// As --book, but write nothing: exit 1 when a generated page or region
    /// is stale or a chapter calls a function that isn't there (CI)
    #[arg(long, value_name = "DIR", num_args = 0..=1, default_missing_value = "docs/book",
          conflicts_with_all = ["json", "filter"])]
    pub check_book: Option<PathBuf>,
}

pub fn run(cmd: ScriptCommands) -> Result<()> {
    match cmd {
        ScriptCommands::Api(args) => api(&args),
        ScriptCommands::Check(args) => check(&args),
    }
}

/// `flint script check`.
fn check(args: &CheckArgs) -> Result<()> {
    let report = check_report(args)?;
    let text = if args.json {
        let mut text = serde_json::to_string_pretty(&check_json(&report))?;
        text.push('\n');
        text
    } else {
        check_text(&report)
    };
    use std::io::Write;
    match std::io::stdout().lock().write_all(text.as_bytes()) {
        Err(e) if e.kind() != std::io::ErrorKind::BrokenPipe => return Err(e.into()),
        _ => {}
    }
    if report.has_errors() {
        std::process::exit(1);
    }
    Ok(())
}

/// What `flint script check` finds, without printing it.
pub fn check_report(args: &CheckArgs) -> Result<crate::check::Report> {
    use crate::check::{self, Checks, Subject};
    let api = args.script.api()?;
    let path = &args.path;
    let is_script = path.extension().is_some_and(|e| e == "rhai");
    if is_script && path.is_file() {
        let file = flint_script::check::ScriptFile::found(path.clone());
        let (diagnostics, scripts) = check::scripts::check(&api, vec![file]);
        return Ok(check::Report {
            scripts,
            diagnostics,
            ..Default::default()
        });
    }
    let mut subject = if path.is_dir() {
        Subject::project(path, super::bundle::project_files(path)?, &args.schemas)
    } else if path.is_file() {
        Subject::scene(path, &args.schemas)
    } else {
        bail!(
            "{} is neither a folder, a scene nor a script",
            flint_core::project::shown(path)
        );
    };
    subject.script_api = Some(api);
    Ok(check::run(&subject, Checks::SCRIPTS))
}

/// `checked 55 scripts (84 scenes): 2 errors, 1 warning`.
fn check_summary(report: &crate::check::Report) -> String {
    let plural = |n: usize, word: &str| format!("{n} {word}{}", if n == 1 { "" } else { "s" });
    let scenes = match report.scenes.len() {
        0 => String::new(),
        n => format!(" ({})", plural(n, "scene")),
    };
    format!(
        "checked {}{scenes}: {}",
        plural(report.scripts.len(), "script"),
        report.counts()
    )
}

/// Each problem as a compiler prints one, `file:line:col: error[code]:
/// message`, the hint and the paths tried under it, then the count.
fn check_text(report: &crate::check::Report) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let mut list: Vec<&flint_core::diagnostic::Diagnostic> = report.diagnostics.iter().collect();
    // Errors first, the files in the order read
    list.sort_by_key(|d| d.severity);
    for d in list {
        let mut place = d
            .file
            .as_deref()
            .map(flint_core::project::shown)
            .unwrap_or_else(|| "(project)".to_string());
        if let (Some(line), Some(col)) = (d.line, d.column) {
            let _ = write!(place, ":{line}:{col}");
        }
        let mut at = d.clone();
        (at.line, at.column) = (None, None);
        let location = at.location();
        let location = if location.is_empty() {
            String::new()
        } else {
            format!(" {location}")
        };
        let _ = writeln!(
            out,
            "{place}: {}[{}]{location}: {}",
            d.severity, d.code, d.message
        );
        if let Some(hint) = &d.hint {
            let _ = writeln!(out, "    hint: {hint}");
        }
        if !d.tried.is_empty() {
            let _ = writeln!(out, "    tried:");
            for path in d.tried.iter().take(6) {
                let _ = writeln!(out, "      {}", flint_core::project::shown(path));
            }
        }
    }
    let _ = writeln!(out, "{}", check_summary(report));
    out
}

/// One document in `flint check`'s shape, with the scripts read.
fn check_json(report: &crate::check::Report) -> Value {
    let mut doc = crate::check::json(report);
    doc["summary"] = Value::from(check_summary(report));
    doc
}

fn api(args: &ApiArgs) -> Result<()> {
    if let Some(dir) = args.check_book.as_deref() {
        return book_cmd(dir, true);
    }
    if let Some(dir) = args.book.as_deref() {
        return book_cmd(dir, false);
    }
    let listing = flint_player::script_system().api_listing();
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

/// The engine's listing, and the signatures Rhai's own packages register.
fn listing_and_packages() -> (ApiListing, Vec<String>) {
    let mut system = flint_player::script_system();
    let listing = system.api_listing();
    let engine = system.rhai_engine_mut();
    let packages = book::package_signatures(
        engine.gen_fn_signatures(true),
        engine.gen_fn_signatures(false),
    );
    (listing, packages)
}

/// `--book` / `--check-book`.
fn book_cmd(dir: &Path, check: bool) -> Result<()> {
    if !dir.join("src").is_dir() {
        bail!(
            "{} has no src/: give the book's folder (the one holding book.toml)",
            dir.display()
        );
    }
    let (listing, packages) = listing_and_packages();
    let outcome = book::run(dir, &listing, &packages, check).map_err(anyhow::Error::msg)?;
    // `docs/book/src/...` with `/` on every platform, as CI prints it
    let shown = |p: &Path| p.display().to_string().replace('\\', "/");
    for path in &outcome.stale {
        if check {
            eprintln!("stale: {}", shown(path));
        } else {
            println!("wrote {}", shown(path));
        }
    }
    for g in &outcome.ghosts {
        eprintln!(
            "{}:{}: `{}(`: {}",
            shown(&dir.join("src").join(&g.file)),
            g.line,
            g.name,
            g.problem
        );
    }
    let stale = check && !outcome.stale.is_empty();
    if stale {
        eprintln!(
            "the book's script API text is stale: run `flint script api --book` and \
             commit the pages"
        );
    }
    if !outcome.ghosts.is_empty() {
        eprintln!(
            "{} call(s) in the book name no script function: fix the page, or, for a \
             name that isn't a script call, add it to {} with a reason",
            outcome.ghosts.len(),
            shown(&dir.join(book::ALLOW))
        );
    }
    if stale || !outcome.ghosts.is_empty() {
        std::process::exit(1);
    }
    if check {
        println!(
            "the book matches the script API ({} functions, {} callbacks)",
            listing.declared.len(),
            listing.callbacks.len()
        );
    } else if outcome.stale.is_empty() {
        println!("the book's script API text was already up to date");
    }
    Ok(())
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

fn declared_json(e: &ApiEntry, listing: &ApiListing) -> Value {
    json!({
        "module": e.module,
        "enabled_when": listing.enabled_when(e.module),
        "name": e.name,
        "declared": true,
        "params": e.params.iter().map(|p| json!({ "name": p.name, "kind": p.kind })).collect::<Vec<_>>(),
        "returns": e.returns.map(|(kind, note)| json!({ "kind": kind, "note": note })),
        "doc": e.doc,
        "typed": e.typed,
        "deprecated": e.deprecated,
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
    let mut functions: Vec<Value> = listing
        .declared
        .iter()
        .map(|e| declared_json(e, listing))
        .collect();
    functions.extend(listing.undeclared.iter().map(undeclared_json));
    json!({
        "counts": {
            "declared": listing.declared.len(),
            "undeclared": listing.undeclared.len(),
            "callbacks": listing.callbacks.len(),
        },
        "undeclared_listed": listing.undeclared_listed,
        // The modules an optional feature owns (#698); every other is core
        "gated_modules": listing
            .gates
            .iter()
            .map(|g| json!({ "module": g.module, "enabled_when": g.enabled_when }))
            .collect::<Vec<_>>(),
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
        match listing.enabled_when(module) {
            // A module an optional feature owns (#698)
            Some(when) => {
                let _ = writeln!(
                    out,
                    "{module} ({} functions; enabled when {when})",
                    entries.len()
                );
            }
            None => {
                let _ = writeln!(out, "{module} ({} functions)", entries.len());
            }
        }
        for e in entries {
            let typed = if e.typed { "  [typed]" } else { "" };
            let _ = writeln!(out, "  {}{typed}", e.signature());
            if let Some(note) = e.deprecated {
                let _ = writeln!(out, "      deprecated: {note}");
            }
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
        flint_player::script_system().api_listing()
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
        let mut system = flint_player::script_system();
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
