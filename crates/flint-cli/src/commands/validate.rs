//! Scene validation command: one scene against its schemas and the
//! project's constraints, with `--fix` for what a constraint can fix.
//!
//! What it checks of the scene itself is `crate::check`'s
//! ([`Checks::VALIDATE`]: that it loads, its entities against the schemas,
//! its style tables and look, its `[wooze]` and `[living]` tables), the
//! same code `flint check` and `flint bundle` run. What is its own is the
//! constraint report and the fixes. `flint check` is the wider gate: it
//! adds every asset and file the scene names.

use crate::check::{self, Checks, SceneSchemas};
use anyhow::Result;
use flint_constraint::{
    compute_scene_diff, ConstraintEvaluator, ConstraintFixer, FixReport, Severity,
    ValidationReport, Violation,
};
use flint_core::diagnostic::{self, codes, Diagnostic};
use flint_core::EntityId;
use flint_scene::{load_scene_diagnosed, SceneDocument};
use std::path::Path;

/// How the report is printed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum Format {
    /// For a person
    #[default]
    Text,
    /// One JSON document on stdout
    Json,
}

#[derive(clap::Args)]
pub struct ValidateArgs {
    /// Path to scene file
    pub scene: String,

    /// Apply auto-fixes
    #[arg(long)]
    pub fix: bool,

    /// Preview fixes without applying
    #[arg(long)]
    pub dry_run: bool,

    /// Show diff of changes
    #[arg(long)]
    pub output_diff: bool,

    /// Schema directories, later ones overriding earlier (repeatable).
    /// Without it: the project's flint.toml `schemas`, else schemas/ in the
    /// working directory, beside the scene and one level up
    #[arg(long, action = clap::ArgAction::Append)]
    pub schemas: Vec<String>,

    /// Output format
    #[arg(long, value_enum, default_value_t = Format::Text)]
    pub format: Format,

    /// A field no schema declares is an error, not a warning (as
    /// `[project] strict = true` in flint.toml makes it for every run)
    #[arg(long)]
    pub strict: bool,

    /// A game's own script functions, for the script check
    #[command(flatten)]
    pub script: check::scripts::ScriptApiArgs,
}

/// What the shared pass found in a scene, in the three lists `flint
/// validate` has always printed.
#[derive(Default)]
struct Found {
    /// The style tables' and the look's problems: a scene still loads
    /// without them, and they have never failed `validate` (they fail
    /// `flint check`).
    warnings: Vec<String>,
    /// `[wooze]` and `[living.species]` problems, as violations of a
    /// constraint that isn't about any entity.
    violations: Vec<Violation>,
    /// Everything else (ADR 0115).
    diagnostics: Vec<Diagnostic>,
    /// How many the project's `[project.diagnostics] allow` left out (#795).
    allowed: usize,
}

impl Found {
    fn from(found: check::scene::SceneFound) -> Self {
        let mut out = Self {
            allowed: found.allowed,
            ..Self::default()
        };
        let found = found.diagnostics;
        let violation = |constraint: &str, entity: String, d: &Diagnostic| Violation {
            constraint_name: constraint.to_string(),
            entity_name: entity,
            entity_id: EntityId::from_raw(0),
            severity: Severity::Error,
            message: d.message.clone(),
            has_auto_fix: false,
        };
        for d in found {
            match d.code {
                codes::BAD_LOOK => out.warnings.push(format!("[post_process] {}", d.message)),
                codes::BAD_USER_STYLE | codes::BAD_STYLE_SETTING => {
                    out.warnings.push(match (&d.file, d.code) {
                        (Some(file), codes::BAD_USER_STYLE) => {
                            format!("{}: {}", flint_core::project::shown(file), d.message)
                        }
                        _ => d.message.clone(),
                    })
                }
                codes::BAD_WOOZE => {
                    let zone = d.field.clone().unwrap_or_else(|| "[wooze]".to_string());
                    out.violations.push(violation("wooze", zone, &d));
                }
                codes::BAD_SPECIES => {
                    let table = format!("[living.{}]", d.field.clone().unwrap_or_default());
                    out.violations.push(violation("living_species", table, &d));
                }
                // A query that doesn't parse is in the constraint report,
                // from the evaluator; a constraint file that doesn't read
                // has no constraint to hang on and stays a diagnostic
                codes::BAD_CONSTRAINT if d.field.is_some() => {}
                _ => out.diagnostics.push(d),
            }
        }
        out
    }
}

/// The schemas and constraints for `scene`, from the one resolver (ADR
/// 0114). A directory a flag or the manifest names that isn't there is the
/// error, by name.
fn load_schemas(scene: &str, flags: &[String]) -> Result<SceneSchemas> {
    use anyhow::Context;
    match SceneSchemas::resolve(Some(Path::new(scene)), flags) {
        Ok(loaded) => Ok(loaded),
        Err(flint_core::FlintError::SchemaDirs(e)) => Err(e.into()),
        Err(e) => Err(e).context("Failed to load schemas"),
    }
}

/// Everything `flint validate <scene>` checks, returned instead of printed:
/// the style warnings, the violations (constraints, `[wooze]`, species),
/// and the scene's diagnostics (ADR 0115). A scene that doesn't load is an
/// `Err`. `flint examples --check` (#763) runs it on every example.
pub fn check(
    scene: &str,
    schemas: &[String],
) -> Result<(Vec<String>, ValidationReport, Vec<Diagnostic>)> {
    let schemas = load_schemas(scene, schemas)?;
    let found = Found::from(check::scene(
        Path::new(scene),
        &schemas,
        Checks::VALIDATE,
        false,
        check::scripts::engine_api(),
    ));
    if let Some(fatal) = found
        .diagnostics
        .iter()
        .find(|d| d.is_error() && d.entity.is_none() && d.table.is_none())
    {
        anyhow::bail!("{}", fatal.one_line());
    }
    let (world, _, _) = load_scene_diagnosed(scene, &schemas.registry)?;
    let mut report =
        ConstraintEvaluator::new(&world, &schemas.registry, &schemas.constraints).validate();
    report.violations.extend(found.violations);
    Ok((found.warnings, report, found.diagnostics))
}

/// What `--fix` or `--dry-run` did, for the report.
struct Fixed {
    dry_run: bool,
    report: FixReport,
    /// The file was rewritten.
    saved: bool,
    /// What a fix changed in the world and the file could not hold.
    not_written: Vec<String>,
    /// The fixes (indices into `report.actions`) whose entity, read back
    /// from the file as written, isn't what the fix left it (#1116): the
    /// file still has what they fixed.
    dropped: Vec<usize>,
    /// The file's diff, for `--output-diff`.
    diff: Option<String>,
}

pub fn run(args: ValidateArgs) -> Result<()> {
    // The one resolver (ADR 0114): --schemas, else the manifest, else discovery
    let schemas = load_schemas(&args.scene, &args.schemas)?;
    let script_api = args.script.api()?;
    super::common_args::warn_dropped_schemas(&schemas.dirs);
    let json = args.format == Format::Json || crate::output::json();

    // What the scene has wrong with it, from the pass `flint check` runs:
    // it needs no constraint files, so a project with none is still checked
    let Found {
        warnings,
        violations: table_violations,
        mut diagnostics,
        mut allowed,
    } = Found::from(check::scene(
        Path::new(&args.scene),
        &schemas,
        Checks::VALIDATE,
        args.strict,
        &script_api,
    ));
    // On stderr, so `--format json` stays one document
    for warning in &warnings {
        eprintln!("warning: {warning}");
    }

    // A scene that doesn't load has nothing else to check
    let world = match load_scene_diagnosed(&args.scene, &schemas.registry) {
        Ok((world, _, _)) => world,
        Err(e) => {
            if !diagnostic::has_errors(&diagnostics) {
                diagnostics
                    .push(Diagnostic::error(codes::SCENE_LOAD, e.to_string()).file(&args.scene));
            }
            if json {
                let nothing = ValidationReport::new();
                print_json(report_json(&nothing, &diagnostics, allowed, None));
            } else {
                print!("{}", diagnostics_text(&args.scene, &diagnostics, allowed));
                println!("The scene does not load, so nothing else was checked.");
            }
            return Err(crate::output::failed());
        }
    };
    let no_constraints = schemas.constraints.is_empty();
    let evaluate = |world: &flint_ecs::FlintWorld| {
        let mut report =
            ConstraintEvaluator::new(world, &schemas.registry, &schemas.constraints).validate();
        report.violations.extend(table_violations.iter().cloned());
        report
    };

    if !(args.fix || args.dry_run) {
        let report = evaluate(&world);
        if json {
            print_json(report_json(&report, &diagnostics, allowed, None));
        } else {
            print_report_text(&args.scene, &report, &diagnostics, allowed, no_constraints);
        }
        if !report.is_valid() || diagnostic::has_errors(&diagnostics) {
            return Err(crate::output::failed());
        }
        return Ok(());
    }

    let fixer = ConstraintFixer::new(&schemas.registry, &schemas.constraints);
    let mut fixed = Fixed {
        dry_run: args.dry_run,
        report: FixReport {
            actions: Vec::new(),
            remaining_violations: 0,
            cycle_detected: false,
            iterations: 0,
        },
        saved: false,
        not_written: Vec::new(),
        dropped: Vec::new(),
        diff: None,
    };
    // What is wrong with the file as this run leaves it: a dry run leaves
    // it as it was
    let report = if args.dry_run {
        fixed.report = fixer.dry_run(&world)?;
        evaluate(&world)
    } else {
        // Fix a second copy of the world, so what the fixes changed is the
        // difference between the two
        let (mut fixed_world, _, _) = load_scene_diagnosed(&args.scene, &schemas.registry)?;
        fixed.report = fixer.fix(&mut fixed_world)?;
        if !fixed.report.actions.is_empty() {
            // Write only what the fixes changed (#677): the file keeps its
            // name, look, camera, wooze field, prefabs and comments.
            let mut doc = SceneDocument::from_file(&args.scene).map_err(anyhow::Error::msg)?;
            let before_text = doc.to_string();
            fixed.not_written = doc.apply_world_diff(&world, &fixed_world);
            doc.save(&args.scene).map_err(anyhow::Error::msg)?;
            fixed.saved = true;
            if args.output_diff {
                fixed.diff = Some(compute_scene_diff(&before_text, &doc.to_string()));
            }
            // The file as written is what's left: read it back and judge
            // that, not the world the fixes were made in (#1116). A fix it
            // couldn't take is still wrong there, and an out-of-range value
            // a fix put right is no longer an error
            let again = Found::from(check::scene(
                Path::new(&args.scene),
                &schemas,
                Checks::VALIDATE,
                args.strict,
                &script_api,
            ));
            diagnostics = again.diagnostics;
            allowed = again.allowed;
            let (written, _, _) = load_scene_diagnosed(&args.scene, &schemas.registry)?;
            fixed.dropped = dropped_fixes(&fixed.report, &world, &fixed_world, &written);
            let mut report =
                ConstraintEvaluator::new(&written, &schemas.registry, &schemas.constraints)
                    .validate();
            // The constraints' count, as the fixer counts it, of the file
            fixed.report.remaining_violations = report.violations.len();
            report.violations.extend(again.violations);
            report
        } else {
            evaluate(&fixed_world)
        }
    };
    let unwritten = !fixed.dropped.is_empty() || !fixed.not_written.is_empty();

    if json {
        print_json(report_json(&report, &diagnostics, allowed, Some(&fixed)));
    } else {
        print!("{}", fix_text(&args.scene, &fixed));
        for problem in &fixed.not_written {
            eprintln!("  not written: {problem}");
        }
        // Nothing here fixes a zone, a misspelled field or a constraint
        // with no fix, so a fixed scene can still be a bad one
        if !report.violations.is_empty() || !diagnostics.is_empty() {
            println!();
            print_report_text(&args.scene, &report, &diagnostics, allowed, no_constraints);
        }
        if unwritten {
            // Last, so it's the line a run ends on (#1116)
            eprintln!("\n{}", unwritten_line(&args.scene, &fixed));
        }
    }
    // A fix the file couldn't take is a fix `--fix` didn't make: the run
    // fails, whatever the constraint's severity (#1116)
    if unwritten || !report.is_valid() || diagnostic::has_errors(&diagnostics) {
        return Err(crate::output::failed());
    }
    Ok(())
}

/// The fixes the file, read back as `written`, doesn't hold: their entity
/// is in `fixed` (the world the fixes were made in) and not in the file or
/// the other way round, or something the fixes changed on it since `before`
/// (its archetype, its parent, a field's value) reads back otherwise.
/// Numbers compare as the file reads them back (`3` is `3.0`); only what
/// changed is compared, so the defaults a load fills in (a component a fix
/// added gets all of its schema's) don't count. A field a fix took away
/// isn't compared here: `apply_world_diff` names one it couldn't take out.
/// Indices into `report.actions`.
fn dropped_fixes(
    report: &FixReport,
    before: &flint_ecs::FlintWorld,
    fixed: &flint_ecs::FlintWorld,
    written: &flint_ecs::FlintWorld,
) -> Vec<usize> {
    use flint_core::toml_util::reads_same;
    use flint_ecs::FlintWorld;
    let parent_of = |world: &FlintWorld, id| {
        world
            .get_parent(id)
            .and_then(|p| world.get_name(p))
            .map(str::to_string)
    };
    let holds = |name: &str| {
        let (Some(id), Some(back)) = (fixed.get_id(name), written.get_id(name)) else {
            return fixed.get_id(name).is_none() && written.get_id(name).is_none();
        };
        let was = before.get_id(name);
        let then = was.and_then(|w| before.get_components(w));
        let (now, read) = (fixed.get_components(id), written.get_components(back));
        let archetype =
            |c: Option<&flint_ecs::DynamicComponents>| c.and_then(|c| c.archetype.clone());
        if archetype(now) != archetype(then) && archetype(now) != archetype(read) {
            return false;
        }
        let parent = parent_of(fixed, id);
        if was.map(|w| parent_of(before, w)) != Some(parent.clone())
            && parent != parent_of(written, back)
        {
            return false;
        }
        let Some(now) = now else { return true };
        now.data.iter().all(|(component, table)| {
            let old = then.and_then(|c| c.data.get(component));
            let back = read.and_then(|c| c.data.get(component));
            match table.as_table() {
                Some(fields) => fields.iter().all(|(field, value)| {
                    let was = old.and_then(|t| t.get(field));
                    let changed = !was.is_some_and(|w| reads_same(w, value));
                    !changed
                        || back
                            .and_then(|t| t.get(field))
                            .is_some_and(|b| reads_same(value, b))
                }),
                None => {
                    old.is_some_and(|o| reads_same(o, table))
                        || back.is_some_and(|b| reads_same(table, b))
                }
            }
        })
    };
    report
        .actions
        .iter()
        .enumerate()
        .filter(|(_, action)| !holds(&action.entity_name))
        .map(|(i, _)| i)
        .collect()
}

/// The line a `--fix` that couldn't write everything ends on: how many of
/// its fixes the file doesn't hold, and which entities they were for.
fn unwritten_line(scene: &str, fixed: &Fixed) -> String {
    let total = fixed.report.actions.len();
    if fixed.dropped.is_empty() {
        // Every fix reads back, but some change the fixes made had no home
        return format!(
            "{} change(s) the fixes made were not written to {scene} (above); \
             every fix reads back from it",
            fixed.not_written.len()
        );
    }
    // Each entity once, in the order its first fix was made
    let mut entities: Vec<&str> = Vec::new();
    for &i in &fixed.dropped {
        let name = fixed.report.actions[i].entity_name.as_str();
        if !entities.contains(&name) {
            entities.push(name);
        }
    }
    format!(
        "{} of {total} fix(es) not written to {scene}, which still has what they fixed: {}",
        fixed.dropped.len(),
        entities.join(", ")
    )
}

/// What `--fix` or `--dry-run` did, as text.
fn fix_text(scene: &str, fixed: &Fixed) -> String {
    let report = &fixed.report;
    let mut out = if fixed.dry_run {
        format!("Dry run results ({} iteration(s)):\n", report.iterations)
    } else {
        format!("Fix results ({} iteration(s)):\n", report.iterations)
    };
    if fixed.dry_run && report.actions.is_empty() {
        out.push_str("  No fixes would be applied.\n");
    }
    for action in &report.actions {
        out.push_str(&format!(
            "  [{}] {} — {}\n",
            action.strategy, action.entity_name, action.description
        ));
    }
    if report.remaining_violations > 0 {
        let verb = if fixed.dry_run {
            "would remain"
        } else {
            "remain"
        };
        out.push_str(&format!(
            "\n  {} violation(s) {verb} after fixes.\n",
            report.remaining_violations
        ));
    }
    if report.cycle_detected {
        out.push_str("  Warning: Fix cycle detected.\n");
    }
    if fixed.saved {
        out.push_str(&format!("\nScene saved to {scene}\n"));
    }
    if let Some(diff) = &fixed.diff {
        out.push_str(&format!("\nDiff:\n{diff}"));
    }
    out
}

/// The whole report as text: the scene's diagnostics (if any), then the
/// constraint violations.
fn print_report_text(
    scene: &str,
    report: &ValidationReport,
    diagnostics: &[Diagnostic],
    allowed: usize,
    no_constraints: bool,
) {
    print!("{}", diagnostics_text(scene, diagnostics, allowed));
    if no_constraints && report.violations.is_empty() {
        if diagnostics.is_empty() && allowed == 0 {
            println!("No problems found.");
        }
        println!("No constraints found in the schema directories; schemas were checked.");
    } else {
        print_violations_text(report);
    }
}

/// The full list (ADR 0115 decision 2), errors first: a count line, then
/// each diagnostic with its hint on the line below. The count line says how
/// many the project allowed (#795). Empty when there is nothing to say.
fn diagnostics_text(scene: &str, diagnostics: &[Diagnostic], allowed: usize) -> String {
    if diagnostics.is_empty() && allowed == 0 {
        return String::new();
    }
    let mut ordered = diagnostics.to_vec();
    diagnostic::sort_errors_first(&mut ordered);
    let mut out = format!(
        "{scene}: {}{}\n\n",
        diagnostic::counts_line(diagnostics),
        diagnostic::allowed_note(allowed)
    );
    for d in &ordered {
        // One about another file (a schema file's lint) names it
        let elsewhere = d
            .file
            .as_deref()
            .filter(|file| !flint_core::project::same_path(file, std::path::Path::new(scene)))
            .map(|file| format!("{}: ", flint_core::project::shown(file)))
            .unwrap_or_default();
        // A parse error's message runs over several lines
        out.push_str(&format!(
            "  {elsewhere}{}\n",
            d.to_string().replace('\n', "\n    ")
        ));
        if let Some(hint) = &d.hint {
            out.push_str(&format!("      hint: {hint}\n"));
        }
    }
    out.push('\n');
    out
}

fn print_violations_text(report: &ValidationReport) {
    if report.violations.is_empty() {
        println!("All constraints passed.");
        return;
    }

    println!("{}", report.summary());
    println!();

    for violation in &report.violations {
        let severity_str = match violation.severity {
            Severity::Error => "ERROR",
            Severity::Warning => "WARN ",
            Severity::Info => "INFO ",
        };

        let fix_marker = if violation.has_auto_fix {
            " [fixable]"
        } else {
            ""
        };

        println!(
            "  [{}] {}: {}{}",
            severity_str, violation.entity_name, violation.message, fix_marker
        );
    }
}

fn print_json(document: serde_json::Value) {
    crate::output::document(&document);
}

/// The report as one JSON document. With `--fix` or `--dry-run` it also has
/// a `fix` object, and `violations` are those of the file as the run left
/// it.
fn report_json(
    report: &ValidationReport,
    diagnostics: &[Diagnostic],
    allowed: usize,
    fixed: Option<&Fixed>,
) -> serde_json::Value {
    let violations: Vec<serde_json::Value> = report
        .violations
        .iter()
        .map(|v| {
            serde_json::json!({
                "constraint": v.constraint_name,
                "entity": v.entity_name,
                "severity": format!("{:?}", v.severity).to_lowercase(),
                "message": v.message,
                "has_auto_fix": v.has_auto_fix,
            })
        })
        .collect();

    // One count over both lists: `valid` is "no error of either kind"
    let errors = report.error_count() + diagnostic::count(diagnostics, diagnostic::Severity::Error);
    let warnings =
        report.warning_count() + diagnostic::count(diagnostics, diagnostic::Severity::Warning);
    let summary = if diagnostics.is_empty() {
        report.summary()
    } else if report.violations.is_empty() {
        format!(
            "schema diagnostics: {}",
            diagnostic::counts_line(diagnostics)
        )
    } else {
        format!(
            "{}; schema diagnostics: {}",
            report.summary(),
            diagnostic::counts_line(diagnostics)
        )
    };
    let mut output = serde_json::json!({
        "valid": errors == 0,
        "summary": summary,
        "errors": errors,
        "warnings": warnings,
        "info": report.info_count(),
        "violations": violations,
        "diagnostics": diagnostics,
        "allowed": allowed,
    });
    if let Some(fixed) = fixed {
        let actions: Vec<serde_json::Value> = fixed
            .report
            .actions
            .iter()
            .map(|a| {
                serde_json::json!({
                    "constraint": a.constraint_name,
                    "entity": a.entity_name,
                    "strategy": a.strategy,
                    "description": a.description,
                })
            })
            .collect();
        output["fix"] = serde_json::json!({
            "dry_run": fixed.dry_run,
            "iterations": fixed.report.iterations,
            "actions": actions,
            "remaining_violations": fixed.report.remaining_violations,
            "cycle_detected": fixed.report.cycle_detected,
            "saved": fixed.saved,
            "not_written": fixed.not_written,
            // The fixes the file as written doesn't hold, as `actions` has
            // them (#1116); any of these, or of `not_written`, fails the run
            "dropped": fixed
                .dropped
                .iter()
                .map(|&i| actions[i].clone())
                .collect::<Vec<_>>(),
            "diff": fixed.diff,
        });
    }
    output
}
