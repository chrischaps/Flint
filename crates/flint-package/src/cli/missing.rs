//! A command (or a flag of one) that needs a package this binary was built
//! without (#1333). Here so a package's own command can say so of another
//! package (procgen's `gen --strip` of living's) as the kernel's do: one
//! error, which flint-cli's exit path turns into exit 2 naming the package.
//! (flint-cli's `absent` module, until #1557.)

/// What was asked for and the package it needs.
#[derive(Debug)]
pub struct MissingPackage {
    /// What was asked for: `flint music chart play`, `--ai-refine`.
    pub what: String,
    /// The package (and the flint-cli feature) it needs.
    pub package: &'static str,
}

/// The exit code a missing package leaves with: clap's for a command line
/// it can't take, since this one can't be run here either.
pub const EXIT: u8 = 2;

impl std::fmt::Display for MissingPackage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} needs the `{}` package, and this flint was built without it: \
             list `{} = \"*\"` under [packages] in flint.toml and run `flint build` \
             (`flint package list --built` names the packages this one has)",
            self.what, self.package, self.package
        )
    }
}

impl std::error::Error for MissingPackage {}

/// The error a command returns for `what`, which needs `package`.
pub fn missing(what: impl Into<String>, package: &'static str) -> anyhow::Error {
    MissingPackage {
        what: what.into(),
        package,
    }
    .into()
}
