//! The launcher (#1552, ADR 0137 decision 5): a package that lives outside
//! flint-player, handed to the kernel by a launcher's list, is found the way
//! the player's own are: its subsystem built for a scene that uses it, its
//! script module declared and enabled, its stages heard. And the list, not
//! the build's features, is what the player was built with: this launcher
//! lists none of the stock packages, so the player lacks them though this
//! test binary compiled them in.
//!
//! One test, one process: a run's packages are registered once.

use flint_package::{Lasts, Package, Registry, SceneCtx, Stage, StageCtx, Subsystem};
use flint_script::api::builder::kind::Int;
use flint_script::api::Module;
use flint_script::ScriptModule;
use std::path::Path;

/// A package of the test's own: a scene with a `[ticker]` table gets a
/// subsystem that counts the frames it hears, and scripts read the count
/// through the `ticker` module's `ticker_frames()`.
struct Ticker;

static TICKER: Ticker = Ticker;

impl Package for Ticker {
    fn name(&self) -> &'static str {
        "ticker"
    }

    fn register(&self, registry: &mut Registry) {
        registry
            .scene_subsystem(build)
            .script_modules(&[&TICKER_MODULE]);
    }
}

fn build(ctx: &mut SceneCtx) -> Option<Box<dyn Subsystem>> {
    ctx.tables.get("ticker")?;
    Some(Box::new(Ticks(0)))
}

struct Ticks(i64);

/// What the module's function reads.
#[derive(Default)]
struct Heard(i64);

impl Subsystem for Ticks {
    fn name(&self) -> &'static str {
        "ticker"
    }

    fn lasts(&self) -> Lasts {
        Lasts::Scene
    }

    fn script_modules(&self) -> &[&'static str] {
        &["ticker"]
    }

    fn update(&mut self, stage: Stage, ctx: &mut StageCtx) {
        if stage == Stage::PreScripts {
            self.0 += 1;
            ctx.script.set_module_state(Heard(self.0));
        }
    }
}

struct TickerModule;

static TICKER_MODULE: TickerModule = TickerModule;

impl ScriptModule for TickerModule {
    fn name(&self) -> &'static str {
        "ticker"
    }

    fn enabled_when(&self) -> &'static str {
        "the scene has a [ticker] table"
    }

    fn declare(&self, m: &mut Module<'_>) {
        m.func("ticker_frames", "The frames the ticker has heard.")
            .returns(Int, "frames")
            .run(|call, ()| call.ctx().module_state.get_mut::<Heard>().0);
    }
}

const SCRIPT: &str = r#"
fn on_update() {
    if ticker_frames() >= 3 {
        test_log("frames", ticker_frames());
        test_pass();
    }
}
"#;

fn project(dir: &Path, table: &str) -> String {
    std::fs::create_dir_all(dir.join("scenes")).unwrap();
    std::fs::create_dir_all(dir.join("scripts")).unwrap();
    std::fs::write(dir.join("flint.toml"), "[project]\n").unwrap();
    std::fs::write(dir.join("scripts/tick.rhai"), SCRIPT).unwrap();
    let scene = dir.join("scenes/main.scene.toml");
    std::fs::write(
        &scene,
        format!(
            "[scene]\nname = \"Ticker\"\n\n{table}\n[entities.director]\nscript = {{ source = \"tick.rhai\" }}\n"
        ),
    )
    .unwrap();
    scene.to_string_lossy().into_owned()
}

#[test]
fn a_package_outside_the_player_runs_from_the_launchers_list() {
    assert!(
        flint_player::packages::launch(&[&TICKER]),
        "the first to ask registers"
    );
    let registry = flint_player::packages::registry();
    assert_eq!(registry.built_with(), ["engine", "ticker"]);

    // What the player was built with is the list: these are compiled in
    // (the default features) but not listed, so the player lacks them
    for package in flint_player::packages::player_packages() {
        assert!(flint_player::packages::lacks(package), "{package}");
    }
    let api = flint_player::script_system().api_listing();
    assert!(
        api.declared.iter().any(|f| f.name == "ticker_frames"),
        "the package's module is declared"
    );
    assert!(
        !api.declared.iter().any(|f| f.module == "wooze"),
        "living's module isn't: the launcher left living out"
    );

    let dir = std::env::temp_dir().join(format!("flint-launcher-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    // A scene that uses it: the subsystem is built, hears each frame, and
    // its module answers the script
    let scene = project(&dir.join("with"), "[ticker]\n");
    let mut options = flint_player::PlaytestOptions::new(scene);
    options.timeout = Some(5.0);
    let report = flint_player::playtest(&options);
    assert!(report.passed, "{:#?}", report.failures);
    assert_eq!(report.log["frames"], serde_json::json!(3), "{report:#?}");

    // A scene without the table: no subsystem, so the module is not
    // enabled and the script's call is its error
    let scene = project(&dir.join("without"), "");
    let mut options = flint_player::PlaytestOptions::new(scene);
    options.exit_after = Some(0.5);
    let report = flint_player::playtest(&options);
    assert!(!report.passed, "{report:#?}");
    assert!(
        report
            .script_errors
            .iter()
            .any(|e| format!("{e:?}").contains("ticker")),
        "{:#?}",
        report.script_errors
    );
    let _ = std::fs::remove_dir_all(&dir);
}
