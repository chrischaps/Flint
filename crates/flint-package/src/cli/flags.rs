//! A package's flags on a command it doesn't own (ADR 0137 "As built: the
//! CLI launcher", #1572): `render --ai-refine` is the ai package's, on the
//! kernel's `render`.
//!
//! The owning command declares where they go: a [`PackageFlags`] field in
//! its derived `Args`, at the place the flags' help heading has always had
//! (so `--help` reads as it did). The package brings the flags, a
//! `clap::Args` group its extension returns from
//! [`flags`](super::CliExtension::flags) as a [`Flags`]. When the command
//! is built, the slot adds every group of the run's packages registered for
//! its place; a build without the package has none of them in the parser,
//! and the launcher names the package for one from its manifest
//! (`[provides] flags`).
//!
//! The parsed line reaches the package through the slot: the command's
//! code hands [`PackageFlags::matches`] to the package's point, which reads
//! its own group from them ([`parsed`]).
//!
//! A command may declare more than one slot, each at its own heading's
//! place: `gen` has three (#1592), the ai package's "AI assistance" and
//! "Cost tracking" groups and its `--style-guide`. Each is named
//! ([`FlagPlace::SLOT`]), and a group says which it goes in
//! ([`Flags::at`]).

use clap::parser::ValueSource;
use clap::{ArgGroup, ArgMatches, Args, Command, FromArgMatches, Id};
use std::cell::RefCell;
use std::marker::PhantomData;
use std::sync::OnceLock;

/// A group of flags a package adds to a command it doesn't own.
#[derive(Clone, Copy)]
pub struct Flags {
    /// The command's words below `flint`: `["render"]`.
    pub on: &'static [&'static str],
    /// Which of the command's slots: empty for a command with one.
    pub slot: &'static str,
    /// The group, as its derived `Args::augment_args`.
    pub augment: fn(Command) -> Command,
}

impl Flags {
    /// The derived group `A` on the command at `on`, in its one slot.
    pub fn of<A: Args>(on: &'static [&'static str]) -> Self {
        Self {
            on,
            slot: "",
            augment: A::augment_args,
        }
    }

    /// The derived group `A` in the slot `P` names: one of several on a
    /// command (`gen`'s).
    pub fn at<P: FlagPlace, A: Args>() -> Self {
        Self {
            on: P::PATH,
            slot: P::SLOT,
            augment: A::augment_args,
        }
    }
}

impl std::fmt::Debug for Flags {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Flags")
            .field("on", &self.on)
            .field("slot", &self.slot)
            .finish()
    }
}

/// Where a [`PackageFlags`] slot is: its command's words, and its name
/// among the command's slots.
pub trait FlagPlace: 'static {
    const PATH: &'static [&'static str];
    /// Empty for a command's one slot.
    const SLOT: &'static str = "";
}

/// Whether a package of the run (or of this thread's scope) brings flags
/// to the slot `P`. Read while a command is built, it lets the owner place
/// a flag of its own by what the slot holds: `gen --print-spec` sits under
/// "AI assistance" when the ai group is there, among the options when it
/// isn't.
pub fn brought<P: FlagPlace>() -> bool {
    !registered(P::PATH, P::SLOT).is_empty()
}

/// The group a slot puts its flags in, so the command's matches say which
/// of the line's flags came through it ([`PackageFlags::given`]).
/// Interned once a slot (clap's ids are `&'static str` in this build).
fn group_of<P: FlagPlace>() -> &'static str {
    static NAMES: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());
    let name = format!("packages {} {}", P::PATH.join(" "), P::SLOT);
    let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(&known) = names.iter().find(|n| **n == name) {
        return known;
    }
    let leaked: &'static str = Box::leak(name.into_boxed_str());
    names.push(leaked);
    leaked
}

/// The slot a command declares for the flags packages add to it: an
/// `Args` whose `augment_args` adds each registered [`Flags`] of its place
/// and whose value is the command's matches, for the packages to read.
pub struct PackageFlags<P> {
    matches: Option<ArgMatches>,
    place: PhantomData<P>,
}

impl<P> PackageFlags<P> {
    /// The command's matches, every package's flags among them; `None` for
    /// a value made by hand.
    pub fn matches(&self) -> Option<&ArgMatches> {
        self.matches.as_ref()
    }
}

impl<P: FlagPlace> PackageFlags<P> {
    /// The flags of this slot the line gave, each as `--its-id` (the
    /// spelling a derived flag's id has): for an owner that refuses a
    /// package's flags where they mean nothing (`gen` of a glyph set).
    pub fn given(&self) -> Vec<String> {
        let Some(matches) = &self.matches else {
            return Vec::new();
        };
        let group = group_of::<P>();
        let Ok(Some(ids)) = matches.try_get_many::<Id>(group) else {
            return Vec::new();
        };
        ids.filter(|id| matches.value_source(id.as_str()) == Some(ValueSource::CommandLine))
            .map(|id| format!("--{}", id.as_str().replace('_', "-")))
            .collect()
    }
}

impl<P> Default for PackageFlags<P> {
    fn default() -> Self {
        Self {
            matches: None,
            place: PhantomData,
        }
    }
}

impl<P> Clone for PackageFlags<P> {
    fn clone(&self) -> Self {
        Self {
            matches: self.matches.clone(),
            place: PhantomData,
        }
    }
}

impl<P> std::fmt::Debug for PackageFlags<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PackageFlags")
    }
}

impl<P: FlagPlace> Args for PackageFlags<P> {
    fn augment_args(cmd: Command) -> Command {
        let before: Vec<Id> = cmd.get_arguments().map(|a| a.get_id().clone()).collect();
        let cmd = registered(P::PATH, P::SLOT)
            .into_iter()
            .fold(cmd, |cmd, flags| (flags.augment)(cmd));
        let added: Vec<Id> = cmd
            .get_arguments()
            .map(|a| a.get_id().clone())
            .filter(|id| !before.contains(id))
            .collect();
        if added.is_empty() {
            return cmd;
        }
        cmd.group(ArgGroup::new(group_of::<P>()).args(added).multiple(true))
    }

    fn augment_args_for_update(cmd: Command) -> Command {
        Self::augment_args(cmd)
    }
}

impl<P> FromArgMatches for PackageFlags<P> {
    fn from_arg_matches(matches: &ArgMatches) -> Result<Self, clap::Error> {
        Ok(Self {
            matches: Some(matches.clone()),
            place: PhantomData,
        })
    }

    fn update_from_arg_matches(&mut self, matches: &ArgMatches) -> Result<(), clap::Error> {
        self.matches = Some(matches.clone());
        Ok(())
    }
}

/// A package's group `A` from a command's matches, when the parser had it:
/// `id` is one of its flags. `None` in a build (or a launcher) without the
/// package.
pub fn parsed<A: FromArgMatches>(matches: Option<&ArgMatches>, id: &str) -> Option<A> {
    let matches = matches?;
    // An id the parser never had is an error in a debug build, absent in
    // a release one
    if !matches!(matches.try_contains_id(id), Ok(true)) {
        return None;
    }
    A::from_arg_matches(matches).ok()
}

static RUN: OnceLock<Vec<Flags>> = OnceLock::new();

thread_local! {
    static SCOPE: RefCell<Option<Vec<Flags>>> = const { RefCell::new(None) };
}

/// The run's flags, every extension's: the launcher sets them once, before
/// the command line is parsed (a second call changes nothing).
pub fn set_run_flags(flags: Vec<Flags>) {
    let _ = RUN.set(flags);
}

/// `build` with `flags` as the packages' flags on this thread, in place of
/// the run's: a launcher made in a test, or one parse of another list.
pub fn with_flags<R>(flags: &[Flags], build: impl FnOnce() -> R) -> R {
    struct Restore(Option<Vec<Flags>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            SCOPE.with(|s| *s.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(SCOPE.with(|s| s.borrow_mut().replace(flags.to_vec())));
    build()
}

/// The flags registered at `path`'s `slot`: this thread's scope's, else
/// the run's.
fn registered(path: &[&str], slot: &str) -> Vec<Flags> {
    let all = SCOPE.with(|s| s.borrow().clone());
    all.or_else(|| RUN.get().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter(|flags| flags.on == path && flags.slot == slot)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Here;
    impl FlagPlace for Here {
        const PATH: &'static [&'static str] = &["here"];
    }

    #[derive(clap::Args)]
    struct Owner {
        #[arg(long)]
        own: bool,
        #[command(flatten)]
        packages: PackageFlags<Here>,
    }

    #[derive(clap::Args)]
    struct Theirs {
        #[arg(long)]
        theirs: Option<u32>,
    }

    fn command() -> Command {
        Owner::augment_args(Command::new("here"))
    }

    #[test]
    fn a_slot_has_the_flags_registered_for_its_place_and_no_others() {
        assert!(command().get_arguments().all(|a| a.get_id() != "theirs"));
        let here = Flags::of::<Theirs>(&["here"]);
        let elsewhere = Flags::of::<Theirs>(&["there"]);
        let cmd = with_flags(&[here], command);
        assert!(cmd.get_arguments().any(|a| a.get_id() == "theirs"));
        let cmd = with_flags(&[elsewhere], command);
        assert!(cmd.get_arguments().all(|a| a.get_id() != "theirs"));
        // The scope ends with the call
        assert!(command().get_arguments().all(|a| a.get_id() != "theirs"));
    }

    #[test]
    fn the_package_reads_its_group_from_the_owners_matches() {
        let flags = [Flags::of::<Theirs>(&["here"])];
        let matches = with_flags(&flags, || {
            command().try_get_matches_from(["here", "--own", "--theirs", "3"])
        })
        .unwrap();
        let owner = Owner::from_arg_matches(&matches).unwrap();
        assert!(owner.own);
        let theirs: Theirs = parsed(owner.packages.matches(), "theirs").unwrap();
        assert_eq!(theirs.theirs, Some(3));
        // A parser without the group: nothing to read
        let bare = command().try_get_matches_from(["here"]).unwrap();
        let owner = Owner::from_arg_matches(&bare).unwrap();
        assert!(parsed::<Theirs>(owner.packages.matches(), "theirs").is_none());
    }

    struct First;
    impl FlagPlace for First {
        const PATH: &'static [&'static str] = &["two"];
        const SLOT: &'static str = "first";
    }
    struct Second;
    impl FlagPlace for Second {
        const PATH: &'static [&'static str] = &["two"];
        const SLOT: &'static str = "second";
    }

    /// `gen`'s shape (#1592): two slots on one command, the owner's own
    /// flag between them.
    #[derive(clap::Args)]
    struct TwoSlots {
        #[command(flatten)]
        first: PackageFlags<First>,
        #[arg(long)]
        own: bool,
        #[command(flatten)]
        second: PackageFlags<Second>,
    }

    #[derive(clap::Args)]
    struct Early {
        #[arg(long)]
        early: bool,
        #[arg(long)]
        early_value: Option<u32>,
    }

    #[derive(clap::Args)]
    struct Late {
        #[arg(long)]
        late: bool,
    }

    #[test]
    fn each_slot_of_a_command_holds_its_own_groups_in_its_place() {
        let flags = [Flags::at::<Second, Late>(), Flags::at::<First, Early>()];
        let cmd = with_flags(&flags, || TwoSlots::augment_args(Command::new("two")));
        let order: Vec<&str> = cmd.get_arguments().map(|a| a.get_id().as_str()).collect();
        assert_eq!(order, ["early", "early_value", "own", "late"]);
        assert!(with_flags(&flags, brought::<First>));
        assert!(!with_flags(&flags[..1], brought::<First>));

        let matches = cmd
            .try_get_matches_from(["two", "--early-value", "2", "--own", "--late"])
            .unwrap();
        let line = TwoSlots::from_arg_matches(&matches).unwrap();
        assert_eq!(line.first.given(), ["--early-value"]);
        assert_eq!(line.second.given(), ["--late"]);
        // A slot nobody filled gives nothing
        let bare = TwoSlots::augment_args(Command::new("two"))
            .try_get_matches_from(["two", "--own"])
            .unwrap();
        let line = TwoSlots::from_arg_matches(&bare).unwrap();
        assert!(line.first.given().is_empty() && line.second.given().is_empty());
    }
}
