//! How the player's window opens: its size and place, its frame, whether it
//! stays on top and whether it takes the keyboard (#611). `flint play` and
//! the standalone `flint-player` flatten the same flags.

use winit::dpi::PhysicalPosition;
use winit::window::{WindowAttributes, WindowLevel};

/// The window flags, shared by `flint play` and `flint-player`
#[derive(clap::Args, Clone, Debug)]
pub struct WindowArgs {
    /// Initial window width in physical pixels (ignored with --fullscreen)
    #[arg(long, default_value_t = 1280)]
    pub width: u32,

    /// Initial window height in physical pixels (ignored with --fullscreen)
    #[arg(long, default_value_t = 720)]
    pub height: u32,

    /// Put the window's outer top-left corner at X,Y in physical desktop
    /// pixels. Negative values reach a monitor left of or above the main one
    /// (`--position=-1920,0`). Default: wherever the system puts it
    #[arg(long, value_name = "X,Y", value_parser = parse_position, allow_hyphen_values = true)]
    pub position: Option<(i32, i32)>,

    /// No title bar or frame. Esc still exits, and Alt+F4 still closes it
    #[arg(long)]
    pub borderless: bool,

    /// Keep the window above other windows
    #[arg(long)]
    pub always_on_top: bool,

    /// Open without taking the keyboard from the window that has it (a
    /// companion beside a terminal). Clicking the window still focuses it.
    /// Not with --fullscreen, which takes the foreground on Windows
    #[arg(long, conflicts_with = "fullscreen")]
    pub no_activate: bool,
}

impl Default for WindowArgs {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 720,
            position: None,
            borderless: false,
            always_on_top: false,
            no_activate: false,
        }
    }
}

impl WindowArgs {
    /// The size, at least a pixel each way
    pub fn size(&self) -> (u32, u32) {
        (self.width.max(1), self.height.max(1))
    }

    /// Every flag but the size laid over a window's attributes (the size is
    /// `PlayerApp::window_size`, which games embedding the player set). Only
    /// what's asked for changes, so the defaults open the window as before.
    pub fn apply(&self, attrs: WindowAttributes) -> WindowAttributes {
        let mut attrs = attrs;
        if let Some((x, y)) = self.position {
            attrs = attrs.with_position(PhysicalPosition::new(x, y));
        }
        if self.borderless {
            attrs = attrs.with_decorations(false);
        }
        if self.always_on_top {
            attrs = attrs.with_window_level(WindowLevel::AlwaysOnTop);
        }
        if self.no_activate {
            attrs = attrs.with_active(false);
        }
        attrs
    }
}

/// `X,Y` in whole pixels, either of them negative
fn parse_position(s: &str) -> Result<(i32, i32), String> {
    let (x, y) = s
        .split_once(',')
        .ok_or_else(|| format!("`{s}`: expected X,Y (e.g. 40,60)"))?;
    let num = |v: &str| {
        v.trim()
            .parse::<i32>()
            .map_err(|_| format!("`{s}`: `{}` isn't a whole number of pixels", v.trim()))
    };
    Ok((num(x)?, num(y)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Cli {
        #[arg(long)]
        fullscreen: bool,
        #[command(flatten)]
        window: WindowArgs,
    }

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("flint").chain(args.iter().copied()))
    }

    #[test]
    fn no_flags_open_the_window_as_before() {
        let w = parse(&[]).unwrap().window;
        assert_eq!(w.size(), (1280, 720));
        assert_eq!(w.position, None);
        assert!(!w.borderless && !w.always_on_top && !w.no_activate);
        let attrs = w.apply(WindowAttributes::default());
        let plain = WindowAttributes::default();
        assert!(attrs.decorations && attrs.active);
        assert_eq!(attrs.window_level, plain.window_level);
        assert!(attrs.position.is_none());
        assert_eq!(attrs.inner_size, plain.inner_size);
    }

    #[test]
    fn every_flag_reaches_the_attributes() {
        let w = parse(&[
            "--width",
            "480",
            "--height",
            "480",
            "--position",
            "-1900,40",
            "--borderless",
            "--always-on-top",
            "--no-activate",
        ])
        .unwrap()
        .window;
        assert_eq!(w.size(), (480, 480));
        let attrs = w.apply(WindowAttributes::default());
        assert_eq!(attrs.inner_size, None, "the size is window_size's");
        assert_eq!(
            attrs.position,
            Some(PhysicalPosition::new(-1900, 40).into())
        );
        assert!(!attrs.decorations);
        assert_eq!(attrs.window_level, WindowLevel::AlwaysOnTop);
        assert!(!attrs.active);
    }

    #[test]
    fn a_position_takes_spaces_and_an_equals_sign() {
        assert_eq!(
            parse(&["--position=-10, -20"]).unwrap().window.position,
            Some((-10, -20))
        );
        assert_eq!(
            parse(&["--position", "0,0"]).unwrap().window.position,
            Some((0, 0))
        );
    }

    #[test]
    fn a_bad_position_is_refused() {
        for bad in ["40", "40,", "a,b", "1.5,2", "1,2,3"] {
            assert!(parse(&["--position", bad]).is_err(), "{bad}");
        }
    }

    #[test]
    fn no_activate_refuses_fullscreen() {
        assert!(parse(&["--fullscreen", "--no-activate"]).is_err());
        assert!(parse(&["--fullscreen", "--always-on-top", "--borderless"]).is_ok());
    }

    #[test]
    fn a_zero_size_is_a_pixel() {
        assert_eq!(
            parse(&["--width", "0", "--height", "0"])
                .unwrap()
                .window
                .size(),
            (1, 1)
        );
    }
}
