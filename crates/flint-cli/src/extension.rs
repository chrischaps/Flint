//! CLI extensions: subcommands a package or a game binary adds to `flint`
//! (ADR 0133, #1140; ADR 0137, #1553), as `PlayerExtension` adds hooks to
//! the player (ADR 0071).
//!
//! The trait and the output helpers are `flint_package::cli`'s, so a
//! package's CLI side needs no flint-cli; this module re-exports them, so a
//! game binary written against ADR 0133 compiles unchanged. A game's own
//! `flint` is a binary of its crate that calls [`crate::run_with`] with its
//! extensions, after [`crate::stock`] if it wants the packages' commands
//! too. Each extension names the subcommands it adds as `clap::Command`s,
//! beside the engine's or under one of its commands
//! ([`CliExtension::under`]), and runs the one chosen. They parse with the
//! engine's commands in one parser, so the global `--json` and `--project`
//! reach them, `flint --help` lists them, and they leave through the
//! engine's exit path (`flint_package::cli`'s module docs).
//!
//! [`subcommands`] turns a derived `clap::Subcommand` enum into an
//! extension; implement the trait directly for anything else.

use anyhow::Result;
use clap::{ArgMatches, Command};
use std::collections::BTreeMap;

pub use flint_package::cli::{
    document, emit, failed, json, say, subcommands, Answer, CliExtension, Derived, Report,
};

/// The extensions of one run, their names checked against the engine's
/// and each other's at each place.
pub(crate) struct Added {
    extensions: Vec<Box<dyn CliExtension>>,
    /// Each place (the words of the engine command they go under, empty
    /// for the top level) and the commands added there, in the order given.
    places: Vec<(Vec<String>, Vec<Command>)>,
    /// Each added command's path (`asset generate`, an alias's too) to the
    /// extension it is.
    owner: BTreeMap<String, usize>,
}

/// The engine's command at `words` below `root`.
fn find<'a>(root: &'a Command, words: &[String]) -> Option<&'a Command> {
    words
        .iter()
        .try_fold(root, |command, word| command.find_subcommand(word))
}

/// `words` and `name` as one path: `asset generate`.
fn path(words: &[String], name: &str) -> String {
    words
        .iter()
        .map(String::as_str)
        .chain(std::iter::once(name))
        .collect::<Vec<_>>()
        .join(" ")
}

impl Added {
    /// Collects every extension's commands, refusing a place that is no
    /// command of the engine's or of an earlier extension's (the rhythm
    /// package's go under the music package's `music`, #1555), and at each
    /// place a name that is the engine's (`builtin`'s subcommands there,
    /// hidden ones included, their aliases, `help`) or an earlier
    /// extension's.
    pub(crate) fn new(extensions: Vec<Box<dyn CliExtension>>, builtin: &Command) -> Result<Self> {
        let mut taken: BTreeMap<String, String> = BTreeMap::new();
        let mut places: Vec<(Vec<String>, Vec<Command>)> = Vec::new();
        let mut owner = BTreeMap::new();
        // The engine's tree with each earlier extension's commands added
        let mut tree = builtin.clone();
        for (index, extension) in extensions.iter().enumerate() {
            let words: Vec<String> = extension.under().iter().map(|w| w.to_string()).collect();
            let Some(parent) = find(&tree, &words) else {
                anyhow::bail!(
                    "extension {index} puts its commands under `flint {}`, \
                     which is no command of flint's",
                    words.join(" ")
                );
            };
            let engine = || match words.is_empty() {
                true => "one of flint's own commands".to_string(),
                false => format!("one of `flint {}`'s own commands", words.join(" ")),
            };
            for command in parent.get_subcommands() {
                taken
                    .entry(path(&words, command.get_name()))
                    .or_insert_with(engine);
                for alias in command.get_all_aliases() {
                    taken.entry(path(&words, alias)).or_insert_with(engine);
                }
            }
            taken.entry(path(&words, "help")).or_insert_with(engine);
            let at = match places.iter().position(|(w, _)| *w == words) {
                Some(at) => at,
                None => {
                    places.push((words.clone(), Vec::new()));
                    places.len() - 1
                }
            };
            let commands = extension.commands();
            for command in &commands {
                let names = std::iter::once(command.get_name().to_string())
                    .chain(command.get_all_aliases().map(String::from));
                for name in names {
                    let full = path(&words, &name);
                    if let Some(whose) = taken.get(&full) {
                        anyhow::bail!(
                            "the extension command `{full}` is {whose}: name it something else"
                        );
                    }
                    taken.insert(full.clone(), format!("extension {index}'s too"));
                    owner.insert(full, index);
                }
                places[at].1.push(command.clone());
            }
            tree = add_at(tree, &words, &commands);
        }
        Ok(Self {
            extensions,
            places,
            owner,
        })
    }

    /// The engine's command with every extension's subcommands beside its
    /// own at each place. `--help` lists the engine's first, as it always
    /// has, then the extensions' in the order given (each derived set
    /// numbers its own from 0, which would sort them in among the
    /// engine's).
    pub(crate) fn augment(&self, builtin: Command) -> Command {
        self.places.iter().fold(builtin, |root, (words, commands)| {
            add_at(root, words, commands)
        })
    }

    /// The extension the line chose, and the matches its command sits
    /// under: the deepest one, so a command an extension added under
    /// another's (`music chart`, under `music`) is its own.
    fn owner<'m>(&self, matches: &'m ArgMatches) -> Option<(usize, &'m ArgMatches)> {
        let mut words: Vec<String> = Vec::new();
        let mut at = matches;
        let mut found = None;
        while let Some((name, below)) = at.subcommand() {
            if let Some(&index) = self.owner.get(&path(&words, name)) {
                found = Some((index, at));
            }
            words.push(name.to_string());
            at = below;
        }
        found
    }

    /// Whether the line chose an extension's command.
    pub(crate) fn owns(&self, matches: &ArgMatches) -> bool {
        self.owner(matches).is_some()
    }

    /// Runs the extension command the line chose (`owns` was true),
    /// refusing a window under `--json`.
    pub(crate) fn run(&mut self, matches: &ArgMatches) -> Result<()> {
        let (index, at) = self
            .owner(matches)
            .expect("run is called only for an extension's command");
        let extension = &mut self.extensions[index];
        if extension.opens_window(at) {
            crate::output::refuse_interactive(&chosen_words(matches).join(" "))?;
        }
        extension.run(at)
    }
}

/// `commands` added to the engine command at `words` below `root`.
fn add_at(root: Command, words: &[String], commands: &[Command]) -> Command {
    match words.split_first() {
        None => {
            let after = root
                .get_subcommands()
                .map(Command::get_display_order)
                .max()
                .map_or(0, |last| last + 1);
            root.subcommands(
                commands
                    .iter()
                    .enumerate()
                    .map(|(i, command)| command.clone().display_order(after + i)),
            )
        }
        Some((first, rest)) => root.mut_subcommand(first, |below| add_at(below, rest, commands)),
    }
}

/// The subcommand words the line chose: `asset generate`.
fn chosen_words(matches: &ArgMatches) -> Vec<String> {
    let mut words = Vec::new();
    let mut at = matches;
    while let Some((name, below)) = at.subcommand() {
        words.push(name.to_string());
        at = below;
    }
    words
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
        struct Named(&'static str, &'static [&'static str]);
        impl CliExtension for Named {
            fn commands(&self) -> Vec<Command> {
                vec![Command::new(self.0)]
            }
            fn under(&self) -> &[&'static str] {
                self.1
            }
            fn run(&mut self, _: &ArgMatches) -> Result<()> {
                Ok(())
            }
        }
        let builtin = crate::Cli::command();
        for (name, under, why) in [
            ("render", &[][..], "one of flint's own"),
            ("help", &[], "one of flint's own"),
            // hidden from the help, still the engine's
            ("serve", &[], "one of flint's own"),
            // under a command, its own subcommands are taken
            ("import", &["asset"], "one of `flint asset`'s own"),
            ("help", &["asset"], "one of `flint asset`'s own"),
        ] {
            let Err(e) = Added::new(vec![Box::new(Named(name, under))], &builtin) else {
                panic!("{name} under {under:?} was taken");
            };
            let e = e.to_string();
            assert!(e.contains(&format!("`{name}`")) || e.contains(name), "{e}");
            assert!(e.contains(why), "{e}");
        }
        let two = Added::new(
            vec![Box::new(Named("bake", &[])), Box::new(Named("bake", &[]))],
            &builtin,
        );
        assert!(two.is_err(), "two extensions with one name");
        assert!(Added::new(vec![Box::new(Named("bake", &[]))], &builtin).is_ok());
        // `render` is the engine's at the top, free under `asset`
        assert!(Added::new(vec![Box::new(Named("render", &["asset"]))], &builtin).is_ok());
        // A place that is no command
        let Err(e) = Added::new(vec![Box::new(Named("x", &["nope"]))], &builtin) else {
            panic!("no such place");
        };
        assert!(e.to_string().contains("`flint nope`"), "{e}");
    }

    #[derive(clap::Subcommand, Debug, PartialEq)]
    enum Under {
        /// Make one
        Make {
            #[arg(long)]
            name: String,
        },
    }

    /// A package's commands under a kernel command (`flint asset generate`,
    /// #1553): parsed in the one parser, listed after the command's own,
    /// run with the matches they sit under, the globals on them.
    #[test]
    fn an_extension_under_an_engine_command_runs_there() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let held = seen.clone();
        let extension = subcommands(move |cmd: Under| {
            held.lock().unwrap().push(cmd);
            Ok(())
        })
        .under(&["asset"]);
        let mut added = Added::new(vec![extension], &crate::Cli::command()).unwrap();

        let m = parse(&added, &["asset", "make", "--name", "moss", "--json"]);
        assert!(added.owns(&m));
        let (_, at) = added.owner(&m).unwrap();
        assert!(at.get_flag("json"), "the global flag reaches its matches");
        added.run(&m).unwrap();
        assert_eq!(
            *seen.lock().unwrap(),
            [Under::Make {
                name: "moss".into()
            }]
        );

        // The command's own subcommands are still the engine's
        let m = parse(&added, &["asset", "list"]);
        assert!(!added.owns(&m));
        assert!(crate::Cli::from_arg_matches(&m).is_ok());

        // Listed last under `asset`, and nowhere else
        let tree = added.augment(crate::Cli::command());
        let asset = tree.find_subcommand("asset").unwrap();
        let mut names: Vec<_> = asset.get_subcommands().collect();
        names.sort_by_key(|c| c.get_display_order());
        assert_eq!(names.last().unwrap().get_name(), "make");
        assert!(tree.find_subcommand("make").is_none());
    }
}
