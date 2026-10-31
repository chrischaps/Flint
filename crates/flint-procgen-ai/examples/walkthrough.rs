//! Run the mock agent's create → interpret → refine loop and write each spec
//! to disk, ready for `flint gen`.
//!
//! ```bash
//! cargo run -p flint-procgen-ai --example walkthrough -- out/
//! cargo run --bin flint -- gen out/tall_oak_tree.procgen.toml -o tall_oak_tree.glb
//! ```

use std::path::PathBuf;

use flint_procgen_ai::{MockAgent, ProcGenAgent};

const PROMPTS: &[(&str, &str)] = &[
    ("tall oak tree", "tree"),
    ("short wide autumn tree", "tree"),
    ("sparse weeping willow", "tree"),
    ("old brick wall", "texture"),
    ("weathered mossy stone", "texture"),
    ("a beetle", "creature"),
];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "agent_specs".into()));
    std::fs::create_dir_all(&out)?;
    let agent: Box<dyn ProcGenAgent> = Box::new(MockAgent);

    for (prompt, generator) in PROMPTS {
        let spec = agent.create_spec_from_prompt(prompt, generator)?;
        let feedback = agent.interpret_spec(&spec)?;
        let refined = agent.refine_spec(feedback.revised_spec.as_ref().unwrap_or(&spec), None)?;

        println!("\n\"{prompt}\" ({generator} -> {})", spec.generator);
        println!("  seed {}, tags {:?}", spec.seed.value.unwrap_or(0), spec.meta.tags);
        println!("  confidence {:.2}", feedback.confidence);
        for s in &feedback.suggestions {
            println!("  - {s}");
        }
        for (name, s) in [(spec.meta.name.clone(), &spec), (format!("{}_refined", spec.meta.name), &refined)] {
            let path = out.join(format!("{name}.procgen.toml"));
            std::fs::write(&path, toml::to_string(s)?)?;
            println!("  wrote {}", path.display());
        }
    }
    Ok(())
}
