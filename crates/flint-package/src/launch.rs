//! A package's own flags on the player's command line (ADR 0137 section 4's
//! player flags, #1585): `--compare` and `--split` are living's. A package
//! registers a `clap::Args` group ([`Registry::player_args`]), the launchers
//! add it to `flint-player` and `flint play` at run time
//! ([`Registry::augment_player_args`]), so the kernel's `PlayerArgs` names
//! none of it, and the parsed value goes into each run's resources
//! ([`PackageFlags::put_in`]), where the package's builder reads it by its
//! own type. A build without the package has no such flag: clap refuses it
//! as it refuses any other.
//!
//! [`Registry::player_args`]: crate::Registry::player_args
//! [`Registry::augment_player_args`]: crate::Registry::augment_player_args

use crate::Resources;
use std::sync::Arc;

/// One package's flags, as the command line gave them: what puts the
/// parsed value into a run's resources.
type PutIn = Arc<dyn Fn(&mut Resources) + Send + Sync>;

/// Every package's flags, parsed (`Registry::read_player_args`): one value
/// per registered group, each a type of its package's own. Empty when no
/// package brings flags or no launcher parsed them (a test, the web).
#[derive(Clone, Default)]
pub struct PackageFlags {
    parsed: Vec<(&'static str, PutIn)>,
}

impl PackageFlags {
    /// `package`'s group, parsed as `value`.
    pub fn add<A: Clone + Send + Sync + 'static>(&mut self, package: &'static str, value: A) {
        self.parsed.push((
            package,
            Arc::new(move |resources: &mut Resources| {
                resources.insert(value.clone());
            }),
        ));
    }

    /// Put each package's parsed flags in `resources`, by its own type:
    /// what a host does before the first scene is built.
    pub fn put_in(&self, resources: &mut Resources) {
        for (_, put) in &self.parsed {
            put(resources);
        }
    }

    /// The packages whose flags were parsed, in order.
    pub fn packages(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.parsed.iter().map(|(p, _)| *p)
    }
}

impl std::fmt::Debug for PackageFlags {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.packages()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Registry;
    use clap::{CommandFactory, FromArgMatches};

    #[derive(clap::Args, Clone, Debug, Default, PartialEq)]
    struct Swing {
        /// How far the bob swings
        #[arg(long)]
        reach: Option<f32>,
        #[arg(long, requires = "reach")]
        damped: bool,
    }

    #[derive(clap::Parser, Debug)]
    struct Kernel {
        #[arg(long)]
        fullscreen: bool,
    }

    struct Toy;
    impl crate::Package for Toy {
        fn name(&self) -> &'static str {
            "toy"
        }
        fn register(&self, registry: &mut Registry) {
            registry.player_args::<Swing>();
        }
    }

    fn parse(registry: &Registry, line: &[&str]) -> Result<(Kernel, PackageFlags), clap::Error> {
        let matches = registry
            .augment_player_args(Kernel::command())
            .try_get_matches_from(line)?;
        Ok((
            Kernel::from_arg_matches(&matches)?,
            registry.read_player_args(&matches)?,
        ))
    }

    /// A package's group joins the kernel's command and lands in the
    /// resources by its own type; without the package the flag is clap's
    /// "unexpected argument".
    #[test]
    fn a_packages_flags_reach_its_resources_and_nowhere_else() {
        let registry = Registry::of(&[&Toy]);
        let (kernel, flags) = parse(
            &registry,
            &["player", "--fullscreen", "--reach", "1.5", "--damped"],
        )
        .unwrap();
        assert!(kernel.fullscreen);
        assert_eq!(flags.packages().collect::<Vec<_>>(), ["toy"]);
        let mut resources = Resources::default();
        flags.put_in(&mut resources);
        assert_eq!(
            resources.get::<Swing>(),
            Some(&Swing {
                reach: Some(1.5),
                damped: true
            })
        );
        // Its own `requires` holds
        assert!(parse(&registry, &["player", "--damped"]).is_err());
        // Not given: the group's defaults, still in
        let (_, flags) = parse(&registry, &["player"]).unwrap();
        let mut resources = Resources::default();
        flags.put_in(&mut resources);
        assert_eq!(resources.get::<Swing>(), Some(&Swing::default()));

        let bare = Registry::default();
        let refused = parse(&bare, &["player", "--reach", "1"]).unwrap_err();
        assert_eq!(refused.kind(), clap::error::ErrorKind::UnknownArgument);
    }
}
