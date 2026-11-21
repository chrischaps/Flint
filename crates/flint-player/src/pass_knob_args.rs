//! The pass knobs' flags (#1317): skip a part of the frame, or draw it N
//! times, by the label F2, `--gpu-time`, traces and RenderDoc give it, so
//! its cost is the frame-time difference on a GPU whose timestamps can't
//! be read. `flint play`, `flint-player` and `flint render` flatten the
//! same two flags; the web page takes `?skip_pass=` and `?repeat_pass=`.
//! What each part does when skipped or repeated: `flint_render::pass_knobs`.

/// `--skip-pass` and `--repeat-pass`, shared by the player and `flint render`
#[derive(clap::Args, Clone, Debug, Default)]
pub struct PassKnobArgs {
    /// Draw nothing of the part of the frame named LABEL (#1317), by the
    /// name F2's GPU rows and --gpu-time give it: `Shadows`, `Scene`,
    /// `Style edges`, `ASCII`, `Bloom`, `Composite`... Its targets keep
    /// what they last held (zero from the first frame), and what reads
    /// them still runs, so the frame time drops by the part's cost.
    /// Repeatable, or comma separated
    #[arg(long = "skip-pass", value_name = "LABEL")]
    pub skip_pass: Vec<String>,

    /// Draw the part named LABEL N times a frame (1-256, #1317), inside
    /// its one GPU timer scope, so its cost grows N-fold. A part that adds
    /// into what it drew or keeps history (Grass, World lines, ASCII, the
    /// UI) draws once and says so. Repeatable, or comma separated
    #[arg(long = "repeat-pass", value_name = "LABEL:N")]
    pub repeat_pass: Vec<String>,
}

impl PassKnobArgs {
    /// The knobs the flags ask for, `None` when they ask for none; an error
    /// names a `--repeat-pass` word it can't read.
    pub fn knobs(&self) -> anyhow::Result<Option<flint_render::PassKnobs>> {
        let mut knobs = flint_render::PassKnobs::new();
        for word in &self.skip_pass {
            knobs.skip_list(word);
        }
        for word in &self.repeat_pass {
            knobs.repeat_list(word).map_err(anyhow::Error::msg)?;
        }
        Ok((!knobs.is_empty()).then_some(knobs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        knobs: PassKnobArgs,
    }

    #[test]
    fn the_flags_make_the_knobs() {
        let cli = Cli::parse_from([
            "x",
            "--skip-pass",
            "Style edges",
            "--skip-pass",
            "Bloom,SSAO",
            "--repeat-pass",
            "Scene:4",
        ]);
        let k = cli.knobs.knobs().unwrap().unwrap();
        assert_eq!(k.times("Style edges"), 0);
        assert_eq!(k.times("ssao"), 0);
        assert_eq!(k.times("Scene"), 4);
        assert_eq!(k.times("Composite"), 1);
    }

    #[test]
    fn no_flags_are_no_knobs() {
        let cli = Cli::parse_from(["x"]);
        assert!(cli.knobs.knobs().unwrap().is_none());
    }

    #[test]
    fn a_bad_repeat_is_refused() {
        let cli = Cli::parse_from(["x", "--repeat-pass", "Scene"]);
        let e = format!("{:#}", cli.knobs.knobs().unwrap_err());
        assert!(e.contains("<label>:<N>"), "{e}");
    }
}
