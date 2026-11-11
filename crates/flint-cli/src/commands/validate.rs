//! Scene validation command

use anyhow::{Context, Result};
use flint_constraint::{
    compute_scene_diff, ConstraintEvaluator, ConstraintFixer, ConstraintRegistry, Severity,
    ValidationReport, Violation,
};
use flint_core::diagnostic::{self, codes, Diagnostic};
use flint_core::EntityId;
use flint_scene::{load_scene_diagnosed, SceneDocument, SceneFile};
use flint_schema::SchemaRegistry;

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

    /// Output format (text or json)
    #[arg(long, default_value = "text")]
    pub format: String,

    /// A field no schema declares is an error, not a warning (as
    /// `[project] strict = true` in flint.toml makes it for every run)
    #[arg(long)]
    pub strict: bool,
}

/// What is wrong with the scene's entities (ADR 0115): every archetype,
/// component and field against the schemas, in one pass that never stops
/// early. `strict` (the flag; the manifest's `strict` is applied by
/// `diagnose`) makes an unknown field an error.
///
/// First, what is wrong with the game's own schema files (#684): an old
/// type spelling, an unknown type, a key the loader ignores. Warnings, and
/// only about files outside an engine checkout (`SchemaRegistry::game_lints`).
fn scene_diagnostics(scene: &str, schemas: &SchemaRegistry, strict: bool) -> Vec<Diagnostic> {
    let path = std::path::Path::new(scene);
    let project = flint_core::project::Project::for_scene(path);
    let mut found = schemas.game_lints();
    found.extend(flint_scene::diagnose(path, schemas, &project));
    if strict {
        diagnostic::promote_strict(&mut found);
    }
    found
}

/// Whether the scene can't be loaded at all, so there is nothing more to
/// check: the diagnostics are the whole report.
fn cannot_load(diagnostics: &[Diagnostic]) -> bool {
    diagnostics
        .iter()
        .any(|d| [codes::TOML_PARSE, codes::FILE_UNREADABLE, codes::BAD_PREFAB].contains(&d.code))
}

/// What a scene load would warn about its `[post_process.styles]` (#318):
/// unknown styles, keys and closed options' values, after its own user
/// styles are registered. Warnings, not violations: the scene still loads.
fn style_warnings(scene: &str) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(scene) else {
        return Vec::new();
    };
    let Ok(file) = toml::from_str::<flint_scene::SceneFile>(&text) else {
        return Vec::new(); // load_scene below says what is wrong with it
    };
    let Some(mut post) = file.post_process else {
        return Vec::new();
    };
    let tables = post.style_tables();
    if tables.is_empty() && post.look.is_none() {
        return Vec::new();
    }
    let scene_dir = std::path::Path::new(scene)
        .parent()
        .unwrap_or(std::path::Path::new("."));
    let mut warnings = flint_render::stylize::custom::load_dir(scene_dir);
    warnings.extend(flint_render::stylize::StyleSettings::from_tables(&tables).1);
    // The scene's look (#356) must be found and fit under its keys
    if let Some(name) = post.look.clone() {
        post.authored = flint_scene::PostProcessDef::authored_table(&text);
        if let Err(e) = flint_render::look::resolve(&post, Some(&name), scene) {
            warnings.push(format!("[post_process] look = \"{name}\": {e}"));
        }
    }
    warnings
}

/// Everything `flint validate <scene>` checks, returned instead of printed:
/// the style warnings, the violations (constraints, `[wooze]`, species),
/// and the scene's diagnostics (ADR 0115). A scene that doesn't load is an
/// `Err`. `flint examples --check` (#763) runs it on every example.
pub fn check(
    scene: &str,
    schemas: &[String],
) -> Result<(Vec<String>, ValidationReport, Vec<Diagnostic>)> {
    let warnings = style_warnings(scene);
    let (schema_registry, schema_dirs) = super::common_args::load_schemas(Some(scene), schemas)?;
    let diagnostics = scene_diagnostics(scene, &schema_registry, false);
    if let Some(fatal) = diagnostics
        .iter()
        .find(|d| d.is_error() && d.entity.is_none())
    {
        anyhow::bail!("{}", fatal.one_line());
    }
    let mut violations = wooze_violations(scene, &schema_registry)?;
    violations.extend(species_violations(scene, &schema_registry)?);
    let mut constraint_registry = ConstraintRegistry::new();
    for path in &schema_dirs.dirs {
        for constraint in ConstraintRegistry::load_from_directory(path)?.all() {
            constraint_registry.register(constraint.clone());
        }
    }
    let (world, _, _) = load_scene_diagnosed(scene, &schema_registry)?;
    let mut report =
        ConstraintEvaluator::new(&world, &schema_registry, &constraint_registry).validate();
    report.violations.extend(violations);
    Ok((warnings, report, diagnostics))
}

pub fn run(args: ValidateArgs) -> Result<()> {
    // Style tables first: they are checked whether or not any constraints
    // exist, on stderr so `--format json` stays one document
    for warning in style_warnings(&args.scene) {
        eprintln!("warning: {warning}");
    }

    // The one resolver (ADR 0114): --schemas, else the manifest, else discovery
    let (schema_registry, schema_dirs) =
        super::common_args::load_schemas(Some(&args.scene), &args.schemas)?;
    super::common_args::warn_dropped_schemas(&schema_dirs);
    let existing = &schema_dirs.dirs;

    // The scene's own diagnostics first (ADR 0115): they need no constraint
    // files, and a scene that doesn't parse has nothing else to check
    let diagnostics = scene_diagnostics(&args.scene, &schema_registry, args.strict);
    if cannot_load(&diagnostics) {
        if args.format == "json" {
            let nothing = ValidationReport {
                violations: Vec::new(),
            };
            print_report_json(&nothing, &diagnostics);
        } else {
            print!("{}", diagnostics_text(&args.scene, &diagnostics));
            println!("The scene does not load, so nothing else was checked.");
        }
        std::process::exit(1);
    }

    let mut wooze = wooze_violations(&args.scene, &schema_registry)?;
    wooze.extend(species_violations(&args.scene, &schema_registry)?);
    let mut constraint_registry = ConstraintRegistry::new();

    if existing.is_empty() {
        // Preserve existing single-directory fallback behavior
        let loaded = ConstraintRegistry::load_from_directory("schemas")?;
        for constraint in loaded.all() {
            constraint_registry.register(constraint.clone());
        }
    } else {
        // Merge constraints from all schema directories (later directories are additive)
        for path in existing {
            let loaded = ConstraintRegistry::load_from_directory(path)?;
            for constraint in loaded.all() {
                constraint_registry.register(constraint.clone());
            }
        }
    }

    // With no constraint files the scene is still loaded and its
    // diagnostics reported: this used to return here, having checked
    // nothing about the entities (#683)
    let no_constraints = constraint_registry.is_empty();

    // `scene_diagnostics` has what a load would log; don't say it twice
    let (world, _scene_file, _) = load_scene_diagnosed(&args.scene, &schema_registry)?;
    // The world as the file has it, for what the fixes change
    let loaded = &world;

    if args.fix || args.dry_run {
        let fixer = ConstraintFixer::new(&schema_registry, &constraint_registry);

        if args.dry_run {
            let report = fixer.dry_run(&world)?;
            println!("Dry run results ({} iteration(s)):", report.iterations);

            if report.actions.is_empty() {
                println!("  No fixes would be applied.");
            } else {
                for action in &report.actions {
                    println!(
                        "  [{}] {} — {}",
                        action.strategy, action.entity_name, action.description
                    );
                }
            }

            if report.remaining_violations > 0 {
                println!(
                    "\n  {} violation(s) would remain after fixes.",
                    report.remaining_violations
                );
            }

            if report.cycle_detected {
                println!("  Warning: Fix cycle detected.");
            }
        } else {
            // Actually fix by reloading, modifying, and saving
            let (mut world, _, _) = load_scene_diagnosed(&args.scene, &schema_registry)?;
            let report = fixer.fix(&mut world)?;

            println!("Fix results ({} iteration(s)):", report.iterations);

            for action in &report.actions {
                println!(
                    "  [{}] {} — {}",
                    action.strategy, action.entity_name, action.description
                );
            }

            if report.remaining_violations > 0 {
                println!(
                    "\n  {} violation(s) remain after fixes.",
                    report.remaining_violations
                );
            }

            if report.cycle_detected {
                println!("  Warning: Fix cycle detected.");
            }

            if !report.actions.is_empty() {
                // Write only what the fixes changed (#677): the file keeps its
                // name, look, camera, wooze field, prefabs and comments.
                let mut doc = SceneDocument::from_file(&args.scene).map_err(anyhow::Error::msg)?;
                let before_text = doc.to_string();
                let problems = doc.apply_world_diff(&loaded, &world);
                for problem in &problems {
                    eprintln!("  not written: {problem}");
                }
                doc.save(&args.scene).map_err(anyhow::Error::msg)?;
                println!("\nScene saved to {}", args.scene);

                if args.output_diff {
                    let diff = compute_scene_diff(&before_text, &doc.to_string());
                    println!("\nDiff:");
                    print!("{}", diff);
                }
            }
        }
        // Nothing here fixes a zone or a misspelled field, so a fixed scene
        // can still be a bad one.
        if !wooze.is_empty() || !diagnostics.is_empty() {
            let report = ValidationReport { violations: wooze };
            print_report(&args.scene, &report, &diagnostics, &args.format, false);
            if !report.is_valid() || diagnostic::has_errors(&diagnostics) {
                std::process::exit(1);
            }
        }
    } else {
        // Validation only
        let evaluator = ConstraintEvaluator::new(&world, &schema_registry, &constraint_registry);
        let mut report = evaluator.validate();
        report.violations.extend(wooze);

        print_report(
            &args.scene,
            &report,
            &diagnostics,
            &args.format,
            no_constraints,
        );

        if !report.is_valid() || diagnostic::has_errors(&diagnostics) {
            std::process::exit(1);
        }
    }

    Ok(())
}

/// The whole report, as one JSON document or as text: the scene's
/// diagnostics (if any), then the constraint violations.
fn print_report(
    scene: &str,
    report: &ValidationReport,
    diagnostics: &[Diagnostic],
    format: &str,
    no_constraints: bool,
) {
    if format == "json" {
        print_report_json(report, diagnostics);
        return;
    }
    print!("{}", diagnostics_text(scene, diagnostics));
    if no_constraints && report.violations.is_empty() {
        if diagnostics.is_empty() {
            println!("No problems found.");
        }
        println!("No constraints found in the schema directories; schemas were checked.");
    } else {
        print_report_text(report);
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

/// What's wrong with the scene's `[wooze]` table, as violations of a constraint
/// that isn't about any entity. A table that doesn't parse at all, such as one
/// with a misspelt key, is an error here, with the parser's words for it.
fn wooze_violations(scene: &str, schemas: &SchemaRegistry) -> Result<Vec<Violation>> {
    let text = std::fs::read_to_string(scene).with_context(|| format!("reading {scene}"))?;
    let mut file: SceneFile = toml::from_str(&text).with_context(|| format!("parsing {scene}"))?;
    let preset_issues = flint_scene::apply_preset_file(&mut file, std::path::Path::new(scene));
    let Some(table) = file.wooze else {
        return Ok(Vec::new());
    };
    let issues = preset_issues.into_iter().chain(table.check(schemas));
    Ok(issues
        .map(|issue| Violation {
            constraint_name: "wooze".to_string(),
            entity_name: issue.zone.clone().unwrap_or_else(|| "[wooze]".to_string()),
            entity_id: EntityId::from_raw(0),
            severity: Severity::Error,
            message: issue.to_string(),
            has_auto_fix: false,
        })
        .collect())
}

/// What's wrong with the scene's `[living.species.<spec>]` tables (#246): a value
/// the player would leave out, and a species no living prop in the scene grows
/// from, which is most likely a misspelt spec name and would change nothing.
fn species_violations(scene: &str, schemas: &SchemaRegistry) -> Result<Vec<Violation>> {
    let text = std::fs::read_to_string(scene).with_context(|| format!("reading {scene}"))?;
    let mut file: SceneFile = toml::from_str(&text).with_context(|| format!("parsing {scene}"))?;
    // A preset the scene names can hold species too, laid under its own.
    flint_scene::apply_preset_file(&mut file, std::path::Path::new(scene));
    let Some(living) = file.living.filter(|living| !living.species.is_empty()) else {
        return Ok(Vec::new());
    };
    let (world, _, _) = load_scene_diagnosed(scene, schemas)?;
    let mut grown: Vec<String> = world
        .all_entities()
        .iter()
        .filter_map(|entity| {
            let comps = world.get_components(entity.id)?;
            comps.get(flint_core::components::LIVING_PROP)?;
            let asset = comps.get(flint_core::components::MODEL)?.get("asset")?;
            Some(asset.as_str()?.to_string())
        })
        .collect();
    grown.sort();
    grown.dedup();

    let violation = |name: &str, message: String| Violation {
        constraint_name: "living_species".to_string(),
        entity_name: format!("[living.species.{name}]"),
        entity_id: EntityId::from_raw(0),
        severity: Severity::Error,
        message,
        has_auto_fix: false,
    };
    let mut violations = Vec::new();
    for (name, def) in &living.species {
        for problem in flint_player::species_problems(def) {
            violations.push(violation(name, problem));
        }
        if !grown.contains(name) {
            let species = if grown.is_empty() {
                "it has no living props".to_string()
            } else {
                format!("its living props grow from {}", grown.join(", "))
            };
            violations.push(violation(
                name,
                format!(
                    "no living prop in this scene grows from '{name}', \
                     so it changes nothing; {species}"
                ),
            ));
        }
    }
    Ok(violations)
}

fn print_report_text(report: &flint_constraint::ValidationReport) {
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

fn print_report_json(report: &flint_constraint::ValidationReport, diagnostics: &[Diagnostic]) {
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
    let output = serde_json::json!({
        "valid": errors == 0,
        "summary": summary,
        "errors": errors,
        "warnings": warnings,
        "info": report.info_count(),
        "violations": violations,
        "diagnostics": diagnostics,
    });

    println!("{}", serde_json::to_string_pretty(&output).unwrap());
}
