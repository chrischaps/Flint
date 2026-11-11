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
}

impl Found {
    fn from(found: Vec<Diagnostic>) -> Self {
        let mut out = Self::default();
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
    ));
    if let Some(fatal) = found
        .diagnostics
        .iter()
        .find(|d| d.is_error() && d.entity.is_none())
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
    /// The file's diff, for `--output-diff`.
    diff: Option<String>,
}

pub fn run(args: ValidateArgs) -> Result<()> {
    // The one resolver (ADR 0114): --schemas, else the manifest, else discovery
    let schemas = load_schemas(&args.scene, &args.schemas)?;
    super::common_args::warn_dropped_schemas(&schemas.dirs);
    let json = args.format == Format::Json;

    // What the scene has wrong with it, from the pass `flint check` runs:
    // it needs no constraint files, so a project with none is still checked
    let Found {
        warnings,
        violations: table_violations,
        mut diagnostics,
    } = Found::from(check::scene(
        Path::new(&args.scene),
        &schemas,
        Checks::VALIDATE,
        args.strict,
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
                print_json(report_json(&nothing, &diagnostics, None));
            } else {
                print!("{}", diagnostics_text(&args.scene, &diagnostics));
                println!("The scene does not load, so nothing else was checked.");
            }
            std::process::exit(1);
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
            print_json(report_json(&report, &diagnostics, None));
        } else {
            print_report_text(&args.scene, &report, &diagnostics, no_constraints);
        }
        if !report.is_valid() || diagnostic::has_errors(&diagnostics) {
            std::process::exit(1);
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
        }
        evaluate(&fixed_world)
    };

    if json {
        print_json(report_json(&report, &diagnostics, Some(&fixed)));
    } else {
        print!("{}", fix_text(&args.scene, &fixed));
        for problem in &fixed.not_written {
            eprintln!("  not written: {problem}");
        }
        // Nothing here fixes a zone, a misspelled field or a constraint
        // with no fix, so a fixed scene can still be a bad one
        if !report.violations.is_empty() || !diagnostics.is_empty() {
            println!();
            print_report_text(&args.scene, &report, &diagnostics, no_constraints);
        }
    }
    if !report.is_valid() || diagnostic::has_errors(&diagnostics) {
        std::process::exit(1);
    }
    Ok(())
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
    no_constraints: bool,
) {
    print!("{}", diagnostics_text(scene, diagnostics));
    if no_constraints && report.violations.is_empty() {
        if diagnostics.is_empty() {
            println!("No problems found.");
        }
        println!("No constraints found in the schema directories; schemas were checked.");
    } else {
        print_violations_text(report);
    }
}

/// The full list (ADR 0115 decision 2), errors first: a count line, then
/// each diagnostic with its hint on the line below. Empty when there are
/// none.
fn diagnostics_text(scene: &str, diagnostics: &[Diagnostic]) -> String {
    if diagnostics.is_empty() {
        return String::new();
    }
    let mut ordered = diagnostics.to_vec();
    diagnostic::sort_errors_first(&mut ordered);
    let mut out = format!("{scene}: {}\n\n", diagnostic::counts_line(diagnostics));
    for d in &ordered {
        // A diagnostic about another file (a constraint file) names it
        if let Some(file) = d.file.as_deref().filter(|f| *f != Path::new(scene)) {
            out.push_str(&format!("  in {}:\n", flint_core::project::shown(file)));
        }
        // A parse error's message runs over several lines
        out.push_str(&format!("  {}\n", d.to_string().replace('\n', "\n    ")));
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
    println!("{}", serde_json::to_string_pretty(&document).unwrap());
}

/// The report as one JSON document. With `--fix` or `--dry-run` it also has
/// a `fix` object, and `violations` are those of the file as the run left
/// it.
fn report_json(
    report: &ValidationReport,
    diagnostics: &[Diagnostic],
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
            "diff": fixed.diff,
        });
    }
    output
}
