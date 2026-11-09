//! Scene validation command

use anyhow::{Context, Result};
use flint_constraint::{
    compute_scene_diff, ConstraintEvaluator, ConstraintFixer, ConstraintRegistry, Severity,
    ValidationReport, Violation,
};
use flint_core::EntityId;
use flint_scene::{load_scene, save_scene_string, SceneFile};
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

    /// Paths to schemas directories (can specify multiple)
    #[arg(long, default_value = "schemas", action = clap::ArgAction::Append)]
    pub schemas: Vec<String>,

    /// Output format (json or toml)
    #[arg(long, default_value = "text")]
    pub format: String,
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

pub fn run(args: ValidateArgs) -> Result<()> {
    // Style tables first: they are checked whether or not any constraints
    // exist, on stderr so `--format json` stays one document
    for warning in style_warnings(&args.scene) {
        eprintln!("warning: {warning}");
    }

    // Merge explicit schemas with auto-discovered dirs from scene path
    let mut all_schemas = args.schemas.clone();
    for dir in flint_schema::discover_schema_dirs(&args.scene) {
        let s = dir.to_string_lossy().into_owned();
        if !all_schemas.contains(&s) {
            all_schemas.push(s);
        }
    }

    let existing: Vec<&str> = all_schemas
        .iter()
        .map(|s| s.as_str())
        .filter(|p| std::path::Path::new(p).exists())
        .collect();
    let schema_registry = SchemaRegistry::load_from_directories(&existing)?;
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
        for path in &existing {
            let loaded = ConstraintRegistry::load_from_directory(path)?;
            for constraint in loaded.all() {
                constraint_registry.register(constraint.clone());
            }
        }
    }

    if constraint_registry.is_empty() {
        if !wooze.is_empty() {
            print_and_fail(wooze, &args.format);
        }
        if existing.is_empty() {
            println!("No constraints found in schemas/constraints/");
        } else {
            println!("No constraints found in provided schema directories.");
        }
        println!("Create constraint files to enable validation.");
        return Ok(());
    }

    let (world, _scene_file) = load_scene(&args.scene, &schema_registry)?;

    if args.fix || args.dry_run {
        // Save before state for diff
        let before_toml = if args.output_diff {
            Some(save_scene_string(&world, "scene")?)
        } else {
            None
        };

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
            let (mut world, _) = load_scene(&args.scene, &schema_registry)?;
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
                // Save the fixed scene
                flint_scene::save_scene(&args.scene, &world, "scene")?;
                println!("\nScene saved to {}", args.scene);

                if args.output_diff {
                    if let Some(before) = before_toml {
                        let after = save_scene_string(&world, "scene")?;
                        let diff = compute_scene_diff(&before, &after);
                        println!("\nDiff:");
                        print!("{}", diff);
                    }
                }
            }
        }
        // Nothing here fixes a zone, so a fixed scene can still be a bad one.
        if !wooze.is_empty() {
            print_and_fail(wooze, &args.format);
        }
    } else {
        // Validation only
        let evaluator = ConstraintEvaluator::new(&world, &schema_registry, &constraint_registry);
        let mut report = evaluator.validate();
        report.violations.extend(wooze);

        if args.format == "json" {
            print_report_json(&report);
        } else {
            print_report_text(&report);
        }

        if !report.is_valid() {
            std::process::exit(1);
        }
    }

    Ok(())
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
    let (world, _) = load_scene(scene, schemas)?;
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

/// Report violations with no constraint run to go with them, and fail.
fn print_and_fail(violations: Vec<Violation>, format: &str) -> ! {
    let report = ValidationReport { violations };
    if format == "json" {
        print_report_json(&report);
    } else {
        print_report_text(&report);
    }
    std::process::exit(1)
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

fn print_report_json(report: &flint_constraint::ValidationReport) {
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

    let output = serde_json::json!({
        "valid": report.is_valid(),
        "summary": report.summary(),
        "errors": report.error_count(),
        "warnings": report.warning_count(),
        "info": report.info_count(),
        "violations": violations,
    });

    println!("{}", serde_json::to_string_pretty(&output).unwrap());
}
