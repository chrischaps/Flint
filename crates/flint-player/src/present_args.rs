//! How the window's frames reach the display (#1149): `--vsync`,
//! `--frame-latency` and `--no-pacing`, flattened into `flint play` and
//! `flint-player`. Each was a `FLINT_*` switch alone (#715); the variables
//! stay as aliases under the flags, read only when a flag isn't given.

use flint_render::{PresentOptions, Vsync};

/// The present flags, shared by `flint play` and `flint-player`
#[derive(clap::Args, Clone, Debug, Default)]
pub struct PresentArgs {
    /// How frames are presented: `fifo` (or `on`) waits for vsync, `off`
    /// (or `immediate`) is unlocked, for real frame times in a perf run,
    /// and `mailbox` renders uncapped and shows the newest frame each
    /// refresh, with no tearing. Default: FLINT_VSYNC, else fifo
    #[arg(long, value_name = "off|mailbox|fifo", value_parser = parse_vsync)]
    pub vsync: Option<Vsync>,

    /// Frames the CPU may queue ahead of the display, 1 to 3: 1 answers
    /// input soonest, 3 smooths a GPU that runs uneven. Default:
    /// FLINT_FRAME_LATENCY, else 2
    #[arg(long, value_name = "1..3", value_parser = clap::value_parser!(u32).range(1..=3))]
    pub frame_latency: Option<u32>,

    /// Don't pace the game clock to the monitor's refresh under vsync: each
    /// frame steps by the wall time it took. FLINT_PACING=0 does the same
    #[arg(long)]
    pub no_pacing: bool,
}

impl PresentArgs {
    /// Lay the flags into the launch options; what a flag leaves out is
    /// the variable's, read when the window opens.
    pub fn apply(&self, launch: &mut crate::LaunchOptions) {
        launch.vsync = self.vsync;
        launch.frame_latency = self.frame_latency;
        if self.no_pacing {
            launch.pacing = Some(false);
        }
    }
}

/// The flag's words, the same as `FLINT_VSYNC`'s
fn parse_vsync(s: &str) -> Result<Vsync, String> {
    Vsync::parse(s).ok_or_else(|| {
        format!(
            "`{s}`: expected off, mailbox or fifo ({})",
            Vsync::WORDS.join("|")
        )
    })
}

/// The swapchain a launch asks for: its flags over their aliases.
pub fn present_options(launch: &crate::LaunchOptions) -> PresentOptions {
    PresentOptions::resolve(launch.vsync, launch.frame_latency)
}

/// Whether the clock paces to the refresh: `--no-pacing` (or a game's
/// `launch.pacing`), else `FLINT_PACING`, else on.
pub fn pacing(launch: &crate::LaunchOptions) -> bool {
    launch
        .pacing
        .or_else(|| flint_core::env::PACING.flag())
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        present: PresentArgs,
    }

    fn parse(args: &[&str]) -> Result<PresentArgs, clap::Error> {
        Cli::try_parse_from(std::iter::once("flint").chain(args.iter().copied())).map(|c| c.present)
    }

    #[test]
    fn no_flags_leave_it_to_the_variables() {
        let p = parse(&[]).unwrap();
        assert_eq!((p.vsync, p.frame_latency, p.no_pacing), (None, None, false));
        let mut launch = crate::LaunchOptions::default();
        p.apply(&mut launch);
        assert_eq!(
            (launch.vsync, launch.frame_latency, launch.pacing),
            (None, None, None)
        );
    }

    #[test]
    fn each_flag_reaches_the_launch() {
        let p = parse(&["--vsync", "mailbox", "--frame-latency", "1", "--no-pacing"]).unwrap();
        let mut launch = crate::LaunchOptions::default();
        p.apply(&mut launch);
        assert_eq!(launch.vsync, Some(Vsync::Mailbox));
        assert_eq!(launch.frame_latency, Some(1));
        assert!(!pacing(&launch), "--no-pacing wins over FLINT_PACING");
        let present = present_options(&launch);
        assert_eq!(present.vsync, Vsync::Mailbox);
        assert_eq!(present.frame_latency, 1);
        assert_eq!(present.vsync.present_mode(), wgpu::PresentMode::Mailbox);
    }

    #[test]
    fn vsync_takes_the_variables_words() {
        for (word, want) in [
            ("off", Vsync::Off),
            ("immediate", Vsync::Off),
            ("0", Vsync::Off),
            ("fifo", Vsync::Fifo),
            ("ON", Vsync::Fifo),
            ("Mailbox", Vsync::Mailbox),
        ] {
            assert_eq!(
                parse(&["--vsync", word]).unwrap().vsync,
                Some(want),
                "{word}"
            );
        }
        let e = parse(&["--vsync", "fasle"]).unwrap_err().to_string();
        assert!(e.contains("expected off, mailbox or fifo"), "{e}");
    }

    #[test]
    fn a_frame_latency_outside_1_to_3_is_refused() {
        for bad in ["0", "4", "two"] {
            assert!(parse(&["--frame-latency", bad]).is_err(), "{bad}");
        }
        assert_eq!(
            parse(&["--frame-latency", "3"]).unwrap().frame_latency,
            Some(3)
        );
    }
}
