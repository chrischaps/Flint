//! Scene validation command

use anyhow::Result;
use flint_constraint::{
    compute_scene_diff, ConstraintEvaluator, ConstraintFixer, ConstraintRegistry, Severity,
};
use flint_scene::{load_scene, save_scene_string};
use flint_schema::SchemaRegistry;

pub struct ValidateArgs {
    pub scene: String,
    pub fix: bool,
    pub dry_run: bool,
    pub output_diff: bool,
    pub schemas: String,
    pub format: String,
}

pub fn run(args: ValidateArgs) -> Result<()> {
    let schema_registry = SchemaRegistry::load_from_directory(&args.schemas)?;
    let constraint_registry = ConstraintRegistry::load_from_directory(&args.schemas)?;

    if constraint_registry.is_empty() {
        println!("No constraints found in {}/constraints/", args.schemas);
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
    } else {
        // Validation only
        let evaluator =
            ConstraintEvaluator::new(&world, &schema_registry, &constraint_registry);
        let report = evaluator.validate();

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

        let fix_marker = if violation.has_auto_fix { " [fixable]" } else { "" };

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
