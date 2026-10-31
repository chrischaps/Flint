//! A live run of [`AnthropicAgent`] against the Messages API. It spends
//! tokens: a few calls, each about 5K input tokens.
//!
//! ```bash
//! export ANTHROPIC_API_KEY=...
//! cargo run -p flint-procgen-ai --example claude_live -- out/
//! cargo run --bin flint -- gen out/pine.procgen.toml -o pine.glb
//! ```
//!
//! 1. Writes a tree spec from a sentence.
//! 2. Reviews that spec.
//! 3. Refines a brick texture toward a mossy-stone reference image and
//!    measures how far the result's colour moved toward it.
//!
//! Everything lands in the output directory, plus `summary.json`.

use std::path::{Path, PathBuf};

use flint_procgen::{
    register_built_in_generators, GeneratorOutput, GeneratorRegistry, ProcGenSpec,
};
use flint_procgen_ai::{AnthropicAgent, MockAgent, ProcGenAgent, Report};
use serde_json::json;

type Res<T> = Result<T, Box<dyn std::error::Error>>;

fn registry() -> GeneratorRegistry {
    let mut r = GeneratorRegistry::new();
    register_built_in_generators(&mut r);
    r
}

/// The albedo a texture spec builds, as RGBA8.
fn albedo(spec: &ProcGenSpec) -> Res<image::RgbaImage> {
    let img = match registry().generate_from_spec(spec)? {
        GeneratorOutput::Image(i) => i,
        GeneratorOutput::ImageSet(mut s) => s.swap_remove(0),
        _ => return Err("not a texture".into()),
    };
    image::RgbaImage::from_raw(img.width, img.height, img.pixels).ok_or("bad pixels".into())
}

fn png_bytes(img: &image::RgbaImage) -> Res<Vec<u8>> {
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png)?;
    Ok(out.into_inner())
}

fn mean_rgb(img: &image::RgbaImage) -> [f64; 3] {
    let mut sum = [0.0; 3];
    for p in img.pixels() {
        for c in 0..3 {
            sum[c] += f64::from(p[c]);
        }
    }
    let n = f64::from(img.width() * img.height());
    sum.map(|s| s / n)
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn sized_texture(prompt: &str) -> Res<ProcGenSpec> {
    let mut spec = MockAgent.create_spec_from_prompt(prompt, "texture")?;
    let t = spec.params.as_table_mut().ok_or("params")?;
    t.insert("width".into(), toml::Value::Integer(256));
    t.insert("height".into(), toml::Value::Integer(256));
    Ok(spec)
}

fn rounds(report: &Report) -> serde_json::Value {
    json!(report
        .rounds
        .iter()
        .map(|r| json!({
            "issues": r.issues,
            "model": r.model,
            "input_tokens": r.usage.input_tokens,
            "output_tokens": r.usage.output_tokens,
            "cache_read": r.usage.cache_read_input_tokens,
            "cache_write": r.usage.cache_creation_input_tokens,
        }))
        .collect::<Vec<_>>())
}

fn write_spec(dir: &Path, name: &str, spec: &ProcGenSpec) -> Res<String> {
    let text = toml::to_string(spec)?;
    std::fs::write(dir.join(format!("{name}.procgen.toml")), &text)?;
    Ok(text)
}

fn main() -> Res<()> {
    let out = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "claude_live".into()),
    );
    std::fs::create_dir_all(&out)?;
    let agent = AnthropicAgent::from_env()?;
    println!("model {}", agent.config().model);

    // 1. Create.
    let prompt = "tall pine tree with sparse branches";
    let (pine, create) = agent.create_spec_reported(prompt, "tree")?;
    let pine_toml = write_spec(&out, "pine", &pine)?;
    println!("\n== create: \"{prompt}\"\n{pine_toml}");
    for (i, r) in create.rounds.iter().enumerate() {
        println!(
            "round {}: {} issue(s) {:?}",
            i + 1,
            r.issues.len(),
            r.issues
        );
    }
    println!("rationale: {:?}", create.notes);

    // 2. Interpret.
    let (feedback, interpret) = agent.interpret_spec_reported(&pine)?;
    println!("\n== interpret (confidence {:.2})", feedback.confidence);
    for s in &feedback.suggestions {
        println!("- {s}");
    }
    let revised_toml = match &feedback.revised_spec {
        Some(r) => Some(write_spec(&out, "pine_revised", r)?),
        None => None,
    };

    // 3. Refine a brick texture toward two references: a stone of the same
    //    tone (pattern is all that differs) and a green, mossy one.
    let start = sized_texture("old red brick wall")?;
    let start_img = albedo(&start)?;
    start_img.save(out.join("start.png"))?;
    write_spec(&out, "brick_start", &start)?;
    let s = mean_rgb(&start_img);

    let same_tone = sized_texture("weathered mossy stone")?;
    let mut green = same_tone.clone();
    green
        .params
        .as_table_mut()
        .ok_or("params")?
        .insert("base_color".into(), toml::Value::String("#5E6B47".into()));

    let mut total = create.usage().plus(interpret.usage());
    let mut refines = Vec::new();
    for (label, target) in [("same_tone", &same_tone), ("green", &green)] {
        let ref_img = albedo(target)?;
        ref_img.save(out.join(format!("reference_{label}.png")))?;
        let (refined, refine) = agent.refine_spec_reported(&start, Some(&png_bytes(&ref_img)?))?;
        let refined_img = albedo(&refined)?;
        refined_img.save(out.join(format!("refined_{label}.png")))?;
        write_spec(&out, &format!("brick_refined_{label}"), &refined)?;

        let (r, f) = (mean_rgb(&ref_img), mean_rgb(&refined_img));
        let (before, after) = (distance(s, r), distance(f, r));
        println!("\n== refine toward {label}: mean colour distance {before:.1} -> {after:.1}");
        for c in &refine.notes {
            println!("- {c}");
        }
        total = total.plus(refine.usage());
        refines.push(json!({
            "label": label,
            "refined": toml::to_string(&refined.params)?,
            "changes": refine.notes,
            "rounds": rounds(&refine),
            "mean_rgb": {"start": s, "reference": r, "refined": f},
            "distance": {"before": before, "after": after},
        }));
    }

    let summary = json!({
        "model": agent.config().model,
        "create": {"prompt": prompt, "spec": pine_toml, "rounds": rounds(&create), "notes": create.notes},
        "interpret": {
            "confidence": feedback.confidence,
            "suggestions": feedback.suggestions,
            "revised": revised_toml,
            "rounds": rounds(&interpret),
        },
        "refine": {"start": toml::to_string(&start.params)?, "runs": refines},
        "usage": {
            "input_tokens": total.input_tokens,
            "output_tokens": total.output_tokens,
            "cache_read": total.cache_read_input_tokens,
            "cache_write": total.cache_creation_input_tokens,
        },
    });
    std::fs::write(
        out.join("summary.json"),
        serde_json::to_string_pretty(&summary)?,
    )?;
    println!("\nwrote {}", out.display());
    Ok(())
}
