//! The AI half of `flint gen`: build an agent, let it create, interpret or
//! refine a spec before generation, and keep account of what it costs.
//!
//! Everything the agent says goes to stderr, so stdout stays free for the
//! generation summary or, with `--print-spec`, for the spec alone.

use std::path::Path;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use flint_procgen::ProcGenSpec;
use flint_procgen_ai::cost::{self, price_for, CostLedger, CostMeter, CostSummary};
use flint_procgen_ai::{AgentConfig, AnthropicAgent, Distance, MockAgent, ProcGenAgent, Report};
use serde::Deserialize;

use super::gen::GenArgs;
use super::gen_refine::{MeshRenderPreviewer, PREVIEW_SIZE};

/// Project defaults for the agent, read from this file when it exists.
/// Every key is optional; flags override them.
pub const CONFIG_PATH: &str = ".flint/procgen_ai.toml";

/// The provider used when neither a flag nor the config names one.
pub const DEFAULT_PROVIDER: &str = "anthropic";

/// `.flint/procgen_ai.toml`. The key never lives here: it comes from
/// `ANTHROPIC_API_KEY`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectConfig {
    provider: Option<String>,
    model: Option<String>,
    max_iterations: Option<u32>,
    effort: Option<String>,
}

/// Which AI steps a run asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Steps {
    pub create: bool,
    pub interpret: bool,
    pub refine: bool,
}

impl Steps {
    pub fn of(args: &GenArgs) -> Self {
        Self {
            create: args.ai_create.is_some(),
            interpret: args.ai_assist,
            refine: args.ai_assist || args.ai_refine,
        }
    }

    pub fn any(self) -> bool {
        self.create || self.interpret || self.refine
    }

    fn names(self) -> Vec<&'static str> {
        [
            (self.create, "create"),
            (self.interpret, "interpret"),
            (self.refine, "refine"),
        ]
        .into_iter()
        .filter_map(|(on, n)| on.then_some(n))
        .collect()
    }
}

/// The agent config this run uses: built-in defaults, then the project
/// config, then the flags.
pub fn resolve_config(args: &GenArgs, project_file: &Path) -> Result<AgentConfig> {
    let project: ProjectConfig = match std::fs::read_to_string(project_file) {
        Ok(text) => toml::from_str(&text)
            .with_context(|| format!("failed to parse {}", project_file.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => ProjectConfig::default(),
        Err(e) => {
            return Err(e).with_context(|| format!("failed to read {}", project_file.display()))
        }
    };
    let provider = args
        .ai_provider
        .clone()
        .or(project.provider)
        .unwrap_or_else(|| DEFAULT_PROVIDER.to_string());
    let mut config = match provider.as_str() {
        "anthropic" => AgentConfig::anthropic(),
        "mock" => AgentConfig::mock(),
        other => bail!("unknown --ai-provider '{other}'; known providers: anthropic, mock"),
    };
    if let Some(m) = args.ai_model.clone().or(project.model) {
        config.model = m;
    }
    if let Some(n) = args.ai_iterations.or(project.max_iterations) {
        config.max_iterations = n;
    }
    if project.effort.is_some() {
        config.effort = project.effort;
    }
    Ok(config)
}

/// The meter every call of this run goes through: the project ledger, and
/// the `--max-cost` ceiling if one was given.
pub fn build_meter(config: &AgentConfig, args: &GenArgs) -> Arc<CostMeter> {
    let mut meter = CostMeter::new(&config.provider).with_ledger(CostLedger::open_default());
    if let Some(max) = args.max_cost {
        meter = meter.with_ceiling(max);
    }
    Arc::new(meter)
}

/// The agent for `config`, its calls going through `meter`. A Claude agent
/// gets the mesh render previewer, so a refine against a reference sees
/// what the spec builds and loops for up to `refine_rounds` measured rounds
/// (default: the config's `max_iterations`) while it gets closer (#183).
pub fn build_agent(
    config: AgentConfig,
    meter: Arc<CostMeter>,
    refine_rounds: Option<u32>,
) -> Result<Box<dyn ProcGenAgent>> {
    match config.provider.as_str() {
        "mock" => Ok(Box::new(MockAgent)),
        "anthropic" => {
            let mut agent = AnthropicAgent::new(config)
                .map_err(|e| anyhow!("{e}"))?
                .with_meter(meter)
                .with_previewer(MeshRenderPreviewer::new(
                    PREVIEW_SIZE,
                    std::env::temp_dir().join("flint-refine"),
                ));
            if let Some(n) = refine_rounds {
                agent = agent.with_refine_rounds(n);
            }
            Ok(Box::new(agent))
        }
        other => bail!("unknown --ai-provider '{other}'"),
    }
}

/// Words that point a description at a generator, for `--ai-create` without
/// `--ai-generator`. The first generator with a hit wins.
const GENERATOR_WORDS: &[(&str, &[&str])] = &[
    (
        "tree_v1",
        &[
            "tree", "trees", "oak", "pine", "fir", "spruce", "willow", "birch", "maple", "elm",
            "ash", "cedar", "cypress", "palm", "sapling", "bush", "shrub", "conifer", "redwood",
            "baobab", "aspen", "beech", "yew",
        ],
    ),
    (
        "creature_v1",
        &[
            "creature",
            "animal",
            "beast",
            "monster",
            "beetle",
            "bug",
            "insect",
            "spider",
            "ant",
            "crab",
            "lizard",
            "dog",
            "cat",
            "wolf",
            "horse",
            "bird",
            "quadruped",
            "biped",
        ],
    ),
    (
        "texture_v1",
        &[
            "texture",
            "material",
            "surface",
            "wall",
            "brick",
            "bricks",
            "stone",
            "rock",
            "tile",
            "tiles",
            "floor",
            "ground",
            "dirt",
            "grass",
            "sand",
            "marble",
            "concrete",
            "plaster",
            "cobble",
            "cobblestone",
            "bark",
            "metal",
            "rust",
            "moss",
            "mossy",
            "pattern",
        ],
    ),
];

/// The generator a description most likely wants: a keyword in it, else
/// what the output's extension can hold (a PNG is a texture).
pub fn infer_generator(prompt: &str, output: Option<&str>) -> Option<&'static str> {
    let words: Vec<String> = prompt
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect();
    let png = output.is_some_and(|o| o.to_ascii_lowercase().ends_with(".png"));
    let by_word = GENERATOR_WORDS
        .iter()
        .filter(|(g, _)| !png || *g == "texture_v1")
        .find(|(_, ws)| words.iter().any(|w| ws.contains(&w.as_str())))
        .map(|(g, _)| *g);
    by_word.or(png.then_some("texture_v1"))
}

/// Run the AI steps: create a spec (or start from `start`), interpret it,
/// refine it. Prints what the agent says to stderr and returns the spec to
/// generate.
pub fn run_steps(
    args: &GenArgs,
    agent: &dyn ProcGenAgent,
    meter: &CostMeter,
    start: Option<ProcGenSpec>,
) -> Result<ProcGenSpec> {
    let steps = Steps::of(args);
    let reference = match &args.reference {
        Some(p) => {
            Some(std::fs::read(p).with_context(|| format!("failed to read reference image {p}"))?)
        }
        None => None,
    };

    let mut spec = match (&args.ai_create, start) {
        (Some(prompt), _) => {
            let generator = match &args.ai_generator {
                Some(g) => g.clone(),
                None => infer_generator(prompt, args.output.as_deref())
                    .ok_or_else(|| {
                        anyhow!(
                            "can't tell which generator \"{prompt}\" is for; pass --ai-generator \
                             (tree_v1, texture_v1, creature_v1)"
                        )
                    })?
                    .to_string(),
            };
            eprintln!("ai: creating a {generator} spec from \"{prompt}\"");
            let (spec, report) = agent
                .create_spec_with_report(prompt, &generator)
                .map_err(|e| anyhow!("ai create failed: {e}"))?;
            print_report("create", &report);
            eprintln!(
                "ai: created '{}' ({}, seed {})",
                spec.meta.name,
                spec.generator,
                spec.seed
                    .value
                    .map_or("random".to_string(), |s| s.to_string())
            );
            spec
        }
        (None, Some(spec)) => spec,
        (None, None) => bail!("an AI step needs a spec file or --ai-create"),
    };

    if steps.interpret {
        let (feedback, report) = agent
            .interpret_spec_with_report(&spec)
            .map_err(|e| anyhow!("ai interpret failed: {e}"))?;
        print_report("interpret", &report);
        eprintln!(
            "ai: interpret: confidence {:.2}{}",
            feedback.confidence,
            if feedback.revised_spec.is_some() {
                ", revised"
            } else {
                ""
            }
        );
        for s in &feedback.suggestions {
            eprintln!("  - {s}");
        }
        if let Some(revised) = feedback.revised_spec {
            spec = revised;
        }
    }

    if steps.refine {
        let what = if reference.is_some() {
            "against the reference image"
        } else {
            "without a reference"
        };
        eprintln!("ai: refining '{}' {what}", spec.meta.name);
        let (refined, report) = agent
            .refine_spec_with_report(&spec, reference.as_deref())
            .map_err(|e| anyhow!("ai refine failed: {e}"))?;
        print_report("refine", &report);
        spec = refined;
    }

    eprintln!("{}", cost_line(meter));
    Ok(spec)
}

/// What a step's conversation said and how it went.
fn print_report(step: &str, report: &Report) {
    if report.rounds.is_empty() {
        return;
    }
    let rounds = report.rounds.len();
    let how = if report.converged() {
        "converged".to_string()
    } else if report.repaired > 0 {
        format!("schema repaired {} param(s)", report.repaired)
    } else {
        "did not converge".to_string()
    };
    eprintln!(
        "ai: {step}: {rounds} round{}, {how}",
        if rounds == 1 { "" } else { "s" }
    );
    for (i, r) in report.rounds.iter().enumerate() {
        for issue in &r.issues {
            eprintln!("  round {}: {issue}", i + 1);
        }
    }
    // A measured refine: the distance to the reference, start and per round.
    for (i, d) in report.distances.iter().enumerate() {
        let label = if i == 0 {
            "start".to_string()
        } else {
            format!("round {i}")
        };
        let kept = if i == report.kept { "  <- kept" } else { "" };
        eprintln!("  {label:<8} {}{kept}", describe_distance(d));
    }
    for note in &report.notes {
        eprintln!("  > {note}");
    }
}

/// `distance   2.0  (colour   2.5, histogram 0.04, layout 0.02)`.
fn describe_distance(d: &Distance) -> String {
    format!(
        "distance {:5.1}  (colour {:5.1}, histogram {:.2}, layout {:.2})",
        d.score, d.mean_colour, d.histogram, d.layout
    )
}

/// `ai: 3 calls, $0.0421 (ceiling $0.50) -> .flint/procgen_costs.json`.
pub fn cost_line(meter: &CostMeter) -> String {
    let calls = meter.calls();
    if calls == 0 {
        return "ai: no paid API calls".to_string();
    }
    let ceiling = meter
        .ceiling()
        .map(|c| format!(" of --max-cost {}", cost::usd(c)))
        .unwrap_or_default();
    format!(
        "ai: {calls} call{}, {}{ceiling}, recorded in {}",
        if calls == 1 { "" } else { "s" },
        cost::usd(meter.spent()),
        cost::LEDGER_PATH
    )
}

/// `--dry-run` with AI steps: what would be asked, and the most each call
/// could cost, without sending anything. Lines, so the caller picks the stream
/// (`--report json` keeps stdout for the report).
pub fn plan_lines(config: &AgentConfig, steps: Steps, args: &GenArgs) -> Vec<String> {
    let (price, priced_as) = price_for(&config.provider, &config.model);
    let max_out = AnthropicAgent::DEFAULT_MAX_TOKENS;
    // A refine against a reference is measured, and may converse once per
    // refine round.
    let refine_rounds = match (&args.reference, steps.refine) {
        (Some(_), true) => args.refine_rounds.unwrap_or(config.max_iterations).max(1),
        _ => 1,
    };
    let mut lines = vec![
        format!("AI steps:   {}", steps.names().join(" -> ")),
        format!("Provider:   {} ({})", config.provider, config.model),
        format!("Rounds:     up to {} per step", config.max_iterations),
    ];
    if refine_rounds > 1 {
        lines.push(format!(
            "Refine:     up to {refine_rounds} measured rounds against the reference"
        ));
    }
    if config.provider == "mock" {
        lines.push("Cost:       free (mock agent, no API calls)".to_string());
    } else {
        let per_reply = f64::from(max_out) * price.output / 1e6;
        let conversations = steps.names().len() as u32 - u32::from(steps.refine)
            + if steps.refine { refine_rounds } else { 0 };
        let calls = conversations * config.max_iterations;
        lines.push(format!(
            "Cost:       output up to {} per call ({max_out} tokens at ${}/MTok{}), \
             at most {calls} calls",
            cost::usd(per_reply),
            price.output,
            priced_as
                .map(|m| format!(", priced as {m}"))
                .unwrap_or_default()
        ));
    }
    if let Some(m) = args.max_cost {
        lines.push(format!("Ceiling:    --max-cost {}", cost::usd(m)));
    }
    lines.push("(nothing was sent; drop --dry-run to run the AI steps)".to_string());
    lines
}

/// `flint gen --cost-report [--since DATE]`.
pub fn cost_report(ledger: &CostLedger, since: Option<&str>) -> Result<()> {
    let since = since
        .map(cost::parse_since)
        .transpose()
        .map_err(|e| anyhow!("{e}"))?;
    let contents = ledger.read().map_err(|e| anyhow!("{e}"))?;
    if !contents.skipped.is_empty() {
        eprintln!(
            "warning: {}: skipped {} unreadable line(s): {:?}",
            ledger.path().display(),
            contents.skipped.len(),
            contents.skipped
        );
    }
    let summary = CostSummary::from_entries(&contents.entries, since.as_deref());
    print!("{}", summary.render());
    println!("\nLedger: {}", ledger.path().display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use flint_procgen_ai::{ScriptedTransport, Transport};

    use super::*;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        gen: GenArgs,
    }

    fn args(argv: &[&str]) -> GenArgs {
        let mut full = vec!["flint-gen"];
        full.extend_from_slice(argv);
        Cli::try_parse_from(full).map(|c| c.gen).unwrap()
    }

    const PINE: &str = r##"{
      "name": "twisted_dead_tree",
      "params": {"trunk_height": 6, "leaf_density": 0.0, "crown_radius": 1.5},
      "rationale": "No leaves and a short trunk read as dead."
    }"##;

    fn anthropic_over(script: &ScriptedTransport, meter: Arc<CostMeter>) -> AnthropicAgent {
        let mut config = AgentConfig::anthropic();
        config.api_key = "sk-test".into();
        AnthropicAgent::with_env_key(config, None)
            .unwrap()
            .with_transport(script.clone())
            .with_meter(meter)
    }

    #[test]
    fn every_ai_flag_parses() {
        let a = args(&[
            "--ai-create",
            "tall oak tree",
            "-o",
            "tree.glb",
            "--ai-provider",
            "mock",
            "--ai-model",
            "claude-sonnet-5-5",
            "--ai-iterations",
            "5",
            "--ai-generator",
            "tree_v1",
            "--max-cost",
            "0.50",
            "--print-spec",
            "--save-spec",
            "out.procgen.toml",
        ]);
        assert_eq!(a.ai_create.as_deref(), Some("tall oak tree"));
        assert_eq!(a.max_cost, Some(0.5));
        assert!(a.print_spec);
        let b = args(&["spec.toml", "--ai-assist"]);
        assert!(b.ai_assist && Steps::of(&b).interpret && Steps::of(&b).refine);
        let c = args(&[
            "spec.toml",
            "--ai-refine",
            "--reference",
            "ref.png",
            "--refine-rounds",
            "4",
        ]);
        assert_eq!(c.refine_rounds, Some(4));
        assert_eq!(
            Steps::of(&c),
            Steps {
                create: false,
                interpret: false,
                refine: true
            }
        );
        let d = args(&["--cost-report", "--since", "2024-01-01"]);
        assert!(d.cost_report && d.spec.is_none());
    }

    #[test]
    fn contradictory_flags_are_refused() {
        let bad = |argv: &[&str]| {
            let mut full = vec!["flint-gen"];
            full.extend_from_slice(argv);
            Cli::try_parse_from(full).is_err()
        };
        assert!(bad(&[])); // no spec, no --ai-create, no --cost-report
        assert!(bad(&["spec.toml", "--ai-create", "x"]));
        assert!(bad(&["spec.toml", "--ai-refine"])); // needs --reference
        assert!(bad(&["spec.toml", "--reference", "r.png"])); // needs an AI step
        assert!(bad(&["spec.toml", "--since", "2024-01-01"]));
        assert!(bad(&["spec.toml", "--max-cost", "-1"]));
        assert!(bad(&["spec.toml", "--ai-iterations", "0"]));
        assert!(bad(&["spec.toml", "--ai-assist", "--refine-rounds", "2"])); // needs --reference
        assert!(bad(&[
            "spec.toml",
            "--ai-refine",
            "--reference",
            "r.png",
            "--refine-rounds",
            "9"
        ]));
    }

    #[test]
    fn config_layers_defaults_project_and_flags() {
        let dir = std::env::temp_dir().join(format!("flint-gen-ai-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("procgen_ai.toml");
        let none = resolve_config(&args(&["--ai-create", "x"]), &file).unwrap();
        assert_eq!(none.provider, "anthropic");
        assert_eq!(none.model, flint_procgen_ai::anthropic::DEFAULT_MODEL);
        assert_eq!(none.max_iterations, 3);

        std::fs::write(&file, "model = \"claude-sonnet-5-5\"\nmax_iterations = 2\n").unwrap();
        let project = resolve_config(&args(&["--ai-create", "x"]), &file).unwrap();
        assert_eq!(project.model, "claude-sonnet-5-5");
        assert_eq!(project.max_iterations, 2);
        let flags = resolve_config(
            &args(&[
                "--ai-create",
                "x",
                "--ai-model",
                "claude-haiku-5-5",
                "--ai-iterations",
                "4",
            ]),
            &file,
        )
        .unwrap();
        assert_eq!(flags.model, "claude-haiku-5-5");
        assert_eq!(flags.max_iterations, 4);

        std::fs::write(&file, "api_key = \"sk-in-a-file\"\n").unwrap();
        assert!(resolve_config(&args(&["--ai-create", "x"]), &file).is_err());
        assert!(
            resolve_config(&args(&["--ai-create", "x", "--ai-provider", "gpt"]), &file).is_err()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn descriptions_pick_their_generator() {
        assert_eq!(
            infer_generator("twisted dead tree", Some("t.glb")),
            Some("tree_v1")
        );
        assert_eq!(infer_generator("tall oak", None), Some("tree_v1"));
        assert_eq!(infer_generator("a beetle", None), Some("creature_v1"));
        assert_eq!(infer_generator("old brick wall", None), Some("texture_v1"));
        // A PNG can only hold a texture: "bark" goes there, not to a tree.
        assert_eq!(
            infer_generator("oak bark", Some("bark.png")),
            Some("texture_v1")
        );
        assert_eq!(
            infer_generator("something", Some("x.png")),
            Some("texture_v1")
        );
        assert_eq!(infer_generator("a spaceship", Some("x.glb")), None);
    }

    #[test]
    fn mock_create_assist_yields_a_spec_the_generator_builds() {
        let a = args(&[
            "--ai-create",
            "twisted dead tree",
            "--ai-assist",
            "--ai-provider",
            "mock",
        ]);
        let meter = CostMeter::new("mock");
        let spec = run_steps(&a, &MockAgent, &meter, None).unwrap();
        assert_eq!(spec.generator, "tree_v1");
        let mut registry = flint_procgen::GeneratorRegistry::new();
        flint_procgen::register_built_in_generators(&mut registry);
        registry.generate_from_spec(&spec).unwrap();
        assert_eq!(meter.calls(), 0);
        // What --print-spec prints parses back to the same spec.
        let text = toml::to_string(&spec).unwrap();
        assert_eq!(ProcGenSpec::parse(&text).unwrap(), spec);
    }

    #[test]
    fn anthropic_create_records_each_call_in_the_ledger() {
        let dir = std::env::temp_dir().join(format!("flint-gen-ai-ledger-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ledger = CostLedger::new(dir.join("procgen_costs.json"));
        let meter = Arc::new(
            CostMeter::new("anthropic")
                .with_ledger(ledger.clone())
                .with_notify(|_| {}),
        );
        let script = ScriptedTransport::new(vec![ScriptedTransport::reply(
            "claude-opus-5-5",
            PINE,
            2_000,
            1_000,
        )]);
        let agent = anthropic_over(&script, meter.clone());
        let a = args(&["--ai-create", "twisted dead tree"]);
        let spec = run_steps(&a, &agent, &meter, None).unwrap();
        assert_eq!(spec.meta.name, "twisted_dead_tree");
        let entries = ledger.read().unwrap().entries;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].spec_name, "twisted_dead_tree");
        // 2K in x $4 + 1K out x $20, per MTok.
        assert!((entries[0].estimated_usd - 0.028).abs() < 1e-9);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_cost_over_the_ceiling_aborts_with_a_clear_message() {
        // The first reply is unusable and costs $0.30; the second call can't
        // fit what's left of $0.50, so it is never sent.
        let script = ScriptedTransport::new(vec![
            ScriptedTransport::reply("claude-opus-5-5", "not json", 5_000, 14_000),
            ScriptedTransport::reply("claude-opus-5-5", PINE, 5_000, 1_000),
        ]);
        let meter = Arc::new(
            CostMeter::new("anthropic")
                .with_ceiling(0.30)
                .with_notify(|_| {}),
        );
        let agent = anthropic_over(&script, meter.clone());
        let a = args(&["--ai-create", "twisted dead tree", "--max-cost", "0.30"]);
        let err = run_steps(&a, &agent, &meter, None).unwrap_err().to_string();
        assert!(err.contains("cost ceiling reached"), "{err}");
        assert!(err.contains("--max-cost $0.3000"), "{err}");
        assert_eq!(script.requests().len(), 1);
        assert!(meter.spent() <= 0.30 + 1e-9);
    }

    #[test]
    fn the_scripted_transport_is_a_transport() {
        let t: Box<dyn Transport> = Box::new(ScriptedTransport::new(vec![]));
        assert!(t
            .post(&flint_procgen_ai::HttpRequest {
                url: String::new(),
                headers: vec![],
                body: String::new(),
            })
            .is_err());
    }
}
