//! The launcher (#1552, ADR 0137 decision 5): a package that lives outside
//! flint-player, handed to the kernel by a launcher's list, is found the way
//! the player's own are: its subsystem built for a scene that uses it, its
//! script module declared and enabled, its stages heard. And the list, not
//! the build's features, is what the player was built with: this launcher
//! lists none of the stock packages, so the player lacks them though this
//! test binary compiled them in.
//!
//! The run lacks living and music too, so it is also where the kernel's
//! words for a package it lacks are heard (#1585): a scene with living's
//! parts gets the one line its manifest writes, and a game state naming
//! the `living` policy still registers.
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

/// A scene of living's parts, played by a run without living: a state
/// that pauses `living` registers all the same.
const STILL: &str = r#"
fn on_init() {
    register_state("still", #{ living: "pause" });
    test_log("still", true);
}
fn on_update() { test_pass(); }
"#;

fn project(dir: &Path, table: &str) -> String {
    project_with(dir, table, SCRIPT)
}

fn project_with(dir: &Path, table: &str, script: &str) -> String {
    std::fs::create_dir_all(dir.join("scenes")).unwrap();
    std::fs::create_dir_all(dir.join("scripts")).unwrap();
    std::fs::write(dir.join("flint.toml"), "[project]\n").unwrap();
    std::fs::write(dir.join("scripts/tick.rhai"), script).unwrap();
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
    // The playtest's warning count, as `flint playtest` installs it
    {
        use tracing_subscriber::layer::SubscriberExt as _;
        use tracing_subscriber::util::SubscriberInitExt as _;
        let _ = tracing_subscriber::registry()
            .with(flint_player::playtest_warning_layer())
            .try_init();
    }
    assert!(
        flint_player::packages::launch(&[&TICKER]),
        "the first to ask registers"
    );
    let registry = flint_player::packages::registry();
    assert_eq!(registry.built_with(), ["engine", "ticker"]);

    // What the player was built with is the list: these are compiled in
    // (the default features) but not listed, so the player lacks them
    for package in flint_player::packages::PLAYER_PACKAGES {
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

    // A player without living doesn't know `living_prop` (#1529), though
    // the engine's schemas on the disk bring living's folder: this run
    // drops it, as a web player built without living carries none of it
    let engine = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas");
    let engine = engine.to_string_lossy().into_owned();
    let dirs = flint_core::project::schema_dirs(None, std::slice::from_ref(&engine)).unwrap();
    let living = |dirs: &flint_core::project::SchemaDirs| {
        dirs.dirs
            .iter()
            .any(|d| d.ends_with("packages/living/schemas"))
    };
    assert!(living(&dirs), "the engine's tree brings it: {dirs:?}");
    let built = flint_player::packages::built_schema_dirs(dirs);
    assert!(!living(&built), "{built:?}");
    assert!(built.dirs.iter().any(|d| d.ends_with("schemas")));

    // Living's parts, played against those schemas in a run without living
    // (#1585, #1601): the one line the manifest writes, once, with what to
    // build, and nothing else at the load. The summary used to add an
    // `unknown-component` for the prop and an `unknown-table` for the
    // field. A state pausing `living` registers all the same
    let scene = project_with(
        &dir.join("grove"),
        "[wooze]\namplitude = 0.5\n\n[entities.oak]\nliving_prop = { species = \"oak\" }\n",
        STILL,
    );
    let mut options = flint_player::PlaytestOptions::new(scene);
    options.schemas = vec![engine];
    options.timeout = Some(5.0);
    let report = flint_player::playtest(&options);
    assert!(report.passed, "{:#?}", report.failures);
    assert_eq!(report.log["still"], serde_json::json!(true), "{report:#?}");
    assert!(report.warnings.counted);
    let said: Vec<&str> = report
        .warnings
        .messages
        .iter()
        .map(|w| w.message.as_str())
        .collect();
    assert_eq!(
        said,
        [
            "[living] this player is built without the `living` package: the scene's living_prop, \
             [wooze] stand still; run `flint build` in the project for a player with it"
        ],
        "{:#?}",
        report.warnings
    );
    assert_eq!(report.warnings.at_load, 1, "{:#?}", report.warnings);
    let _ = std::fs::remove_dir_all(&dir);
}
