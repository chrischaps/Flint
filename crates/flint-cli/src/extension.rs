//! CLI extensions: subcommands a game binary adds to `flint` (ADR 0133,
//! #1140), as `PlayerExtension` adds hooks to the player (ADR 0071).
//!
//! A game's own `flint` is a binary of its crate that calls
//! [`crate::run_with`] with its extensions. Each extension names the
//! subcommands it adds as `clap::Command`s and runs the one chosen. They
//! parse with the engine's commands in one parser, so the global `--json`
//! and `--project` reach them, `flint --help` lists them, and they leave
//! through the engine's exit path:
//!
//! - under `--json`, print exactly one document ([`document`], [`emit`]),
//!   human lines through [`say`];
//! - a failure is an `Err` (an answer already printed that is a failure:
//!   `Err(failed())`), never `std::process::exit`;
//! - a command that opens a window says so in
//!   [`CliExtension::opens_window`], and `--json` refuses it.
//!
//! [`subcommands`] turns a derived `clap::Subcommand` enum into an
//! extension; implement the trait directly for anything else.

use anyhow::Result;
use clap::{ArgMatches, Command, Subcommand};
use std::collections::BTreeMap;

pub use crate::output::{document, emit, failed, json, Answer, Report};

/// Subcommands an embedding game adds to `flint`, given to
/// [`crate::run_with`]. The engine's own commands keep their names: an
/// extension naming one is refused, as are two extensions naming the same.
pub trait CliExtension {
    /// The subcommands this extension adds, each a `clap::Command` with its
    /// own name, flags and help. Asked once a run, before parsing.
    fn commands(&self) -> Vec<Command>;

    /// Run the command the line chose. `matches` is the whole command
    /// line's: `matches.subcommand()` is one of [`commands`](Self::commands)
    /// with its own matches, and the global flags (`json`, `project`) are
    /// on it too. `--project` is already applied: `Project::current()`
    /// returns it.
    fn run(&mut self, matches: &ArgMatches) -> Result<()>;

    /// The chosen command opens a window (or plays sound, or reads a pad),
    /// which `--json` refuses, as it does `flint play`. No by default.
    fn opens_window(&self, _matches: &ArgMatches) -> bool {
        false
    }
}

/// A human line, as the engine's commands write theirs: on stdout, or on
/// stderr when stdout is the `--json` document.
pub fn say(line: impl std::fmt::Display) {
    if crate::output::stdout_reserved() {
        eprintln!("{line}");
    } else {
        println!("{line}");
    }
}

/// An extension from a derived `clap::Subcommand` enum: each variant is a
/// subcommand, and `run` gets the one the line chose.
///
/// ```ignore
/// #[derive(clap::Subcommand)]
/// enum GameCommands {
///     /// Bake the level's navigation grid
///     BakeNav(BakeNavArgs),
/// }
/// flint_cli::run_with(vec![subcommands(|cmd: GameCommands| match cmd {
///     GameCommands::BakeNav(args) => bake_nav(args),
/// })])
/// ```
///
/// [`Derived::opens_window_when`] names the variants that open a window.
pub fn subcommands<S, F>(run: F) -> Box<Derived<S, F>>
where
    S: Subcommand + 'static,
    F: FnMut(S) -> Result<()> + 'static,
{
    Box::new(Derived {
        run,
        window: None,
        _commands: std::marker::PhantomData,
    })
}

/// What [`subcommands`] makes.
pub struct Derived<S, F> {
    run: F,
    #[allow(clippy::type_complexity)]
    window: Option<Box<dyn Fn(&S) -> bool>>,
    _commands: std::marker::PhantomData<fn() -> S>,
}

impl<S: Subcommand, F> Derived<S, F> {
    /// Which of the enum's commands open a window, refused under `--json`.
    pub fn opens_window_when(
        mut self: Box<Self>,
        window: impl Fn(&S) -> bool + 'static,
    ) -> Box<Self> {
        self.window = Some(Box::new(window));
        self
    }
}

impl<S, F> CliExtension for Derived<S, F>
where
    S: Subcommand + 'static,
    F: FnMut(S) -> Result<()> + 'static,
{
    fn commands(&self) -> Vec<Command> {
        S::augment_subcommands(Command::new("flint"))
            .get_subcommands()
            .cloned()
            .collect()
    }

    fn run(&mut self, matches: &ArgMatches) -> Result<()> {
        let command = S::from_arg_matches(matches)?;
        (self.run)(command)
    }

    fn opens_window(&self, matches: &ArgMatches) -> bool {
        match (&self.window, S::from_arg_matches(matches)) {
            (Some(window), Ok(command)) => window(&command),
            _ => false,
        }
    }
}

/// The extensions of one run, their names checked against the engine's
/// and each other's.
pub(crate) struct Added {
    extensions: Vec<Box<dyn CliExtension>>,
    commands: Vec<Command>,
    /// Each added command's name (and alias) to the extension it is.
    owner: BTreeMap<String, usize>,
}

impl Added {
    /// Collects every extension's commands, refusing a name that is the
    /// engine's (`builtin`'s subcommands, hidden ones included, their
    /// aliases, `help`) or an earlier extension's.
    pub(crate) fn new(extensions: Vec<Box<dyn CliExtension>>, builtin: &Command) -> Result<Self> {
        let mut taken: BTreeMap<String, String> = BTreeMap::new();
        let engine = || "one of flint's own commands".to_string();
        for command in builtin.get_subcommands() {
            taken.insert(command.get_name().into(), engine());
            for alias in command.get_all_aliases() {
                taken.insert(alias.into(), engine());
            }
        }
        taken.insert("help".into(), engine());
        let mut commands = Vec::new();
        let mut owner = BTreeMap::new();
        for (index, extension) in extensions.iter().enumerate() {
            for command in extension.commands() {
                let names = std::iter::once(command.get_name().to_string())
                    .chain(command.get_all_aliases().map(String::from));
                for name in names {
                    if let Some(whose) = taken.get(&name) {
                        anyhow::bail!(
                            "the extension command `{name}` is {whose}: name it something else"
                        );
                    }
                    taken.insert(name.clone(), format!("extension {index}'s too"));
                    owner.insert(name, index);
                }
                commands.push(command);
            }
        }
        Ok(Self {
            extensions,
            commands,
            owner,
        })
    }

    /// The engine's command with every extension's subcommand beside its
    /// own. `flint --help` lists the engine's first, as it always has, then
    /// the extensions' in the order given (each derived set numbers its
    /// own from 0, which would sort them in among the engine's).
    pub(crate) fn augment(&self, builtin: Command) -> Command {
        let after = builtin
            .get_subcommands()
            .map(Command::get_display_order)
            .max()
            .map_or(0, |last| last + 1);
        builtin.subcommands(
            self.commands
                .iter()
                .enumerate()
                .map(|(i, command)| command.clone().display_order(after + i)),
        )
    }

    fn owner(&self, matches: &ArgMatches) -> Option<usize> {
        self.owner.get(matches.subcommand_name()?).copied()
    }

    /// Whether the line chose an extension's command.
    pub(crate) fn owns(&self, matches: &ArgMatches) -> bool {
        self.owner(matches).is_some()
    }

    /// Runs the extension command the line chose (`owns` was true),
    /// refusing a window under `--json`.
    pub(crate) fn run(&mut self, matches: &ArgMatches) -> Result<()> {
        let index = self
            .owner(matches)
            .expect("run is called only for an extension's command");
        let extension = &mut self.extensions[index];
        if extension.opens_window(matches) {
            let name = matches.subcommand_name().unwrap_or_default();
            crate::output::refuse_interactive(name)?;
        }
        extension.run(matches)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, FromArgMatches};
    use std::sync::{Arc, Mutex};

    #[derive(clap::Args, Debug, PartialEq)]
    struct BakeArgs {
        /// The level
        level: String,
        #[arg(long, default_value_t = 0.25)]
        cell: f32,
    }

    #[derive(clap::Subcommand, Debug, PartialEq)]
    enum Game {
        /// Bake the navigation grid
        BakeNav(BakeArgs),
        /// Watch the level in a window
        #[command(alias = "look")]
        Peek,
    }

    fn parse(added: &Added, words: &[&str]) -> ArgMatches {
        let argv = std::iter::once("flint").chain(words.iter().copied());
        added
            .augment(crate::Cli::command())
            .try_get_matches_from(argv)
            .unwrap()
    }

    fn game(seen: &Arc<Mutex<Vec<Game>>>) -> Box<dyn CliExtension> {
        let seen = seen.clone();
        subcommands(move |cmd: Game| {
            seen.lock().unwrap().push(cmd);
            Ok(())
        })
        .opens_window_when(|cmd| matches!(cmd, Game::Peek))
    }

    #[test]
    fn a_derived_enum_runs_beside_the_engines_commands() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut added = Added::new(vec![game(&seen)], &crate::Cli::command()).unwrap();

        let m = parse(&added, &["--json", "bake-nav", "levels/a.scene.toml"]);
        assert!(added.owns(&m));
        assert!(
            m.get_flag("json"),
            "the global flag reaches the root matches"
        );
        added.run(&m).unwrap();
        assert_eq!(
            *seen.lock().unwrap(),
            [Game::BakeNav(BakeArgs {
                level: "levels/a.scene.toml".into(),
                cell: 0.25
            })]
        );

        // An engine command is still the engine's
        let m = parse(&added, &["env"]);
        assert!(!added.owns(&m));
        assert!(crate::Cli::from_arg_matches(&m).is_ok());

        // An alias is the extension's too, and says it opens a window
        let m = parse(&added, &["look"]);
        assert!(added.owns(&m));
        assert!(added.extensions[0].opens_window(&m));
    }

    #[test]
    fn a_name_the_engine_has_is_refused() {
        struct Named(&'static str);
        impl CliExtension for Named {
            fn commands(&self) -> Vec<Command> {
                vec![Command::new(self.0)]
            }
            fn run(&mut self, _: &ArgMatches) -> Result<()> {
                Ok(())
            }
        }
        let builtin = crate::Cli::command();
        for (name, why) in [
            ("render", "one of flint's own"),
            ("help", "one of flint's own"),
            // hidden from the help, still the engine's
            ("serve", "one of flint's own"),
        ] {
            let Err(e) = Added::new(vec![Box::new(Named(name))], &builtin) else {
                panic!("{name} was taken");
            };
            let e = e.to_string();
            assert!(e.contains(&format!("`{name}`")) && e.contains(why), "{e}");
        }
        let two = Added::new(
            vec![Box::new(Named("bake")), Box::new(Named("bake"))],
            &builtin,
        );
        assert!(two.is_err(), "two extensions with one name");
        assert!(Added::new(vec![Box::new(Named("bake"))], &builtin).is_ok());
    }
}
