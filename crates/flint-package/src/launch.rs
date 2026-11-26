//! Flags the player declares for a package (ADR 0137 section 4's player
//! flags, #1558): `--compare` and `--split` are living's, but `flint-player`
//! and `flint play` declare them, so their help and their place in it stay
//! as they were. The launch hands their values to the packages as one
//! resource, by the flag's long name, and the package that reads them
//! parses them itself. A package that brings its own `clap::Args` group is
//! #1572's.

/// The package flags the command line gave, by long name, as given.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaunchFlags {
    given: Vec<(&'static str, String)>,
}

impl LaunchFlags {
    /// `--<flag> <value>` was given; a later value of a flag replaces it.
    pub fn set(&mut self, flag: &'static str, value: impl Into<String>) {
        let value = value.into();
        match self.given.iter_mut().find(|(f, _)| *f == flag) {
            Some((_, was)) => *was = value,
            None => self.given.push((flag, value)),
        }
    }

    /// The value `--<flag>` was given, if it was.
    pub fn get(&self, flag: &str) -> Option<&str> {
        self.given
            .iter()
            .find(|(f, _)| *f == flag)
            .map(|(_, v)| v.as_str())
    }

    /// Whether `--<flag>` was given.
    pub fn has(&self, flag: &str) -> bool {
        self.get(flag).is_some()
    }

    /// The value of `--<flag>`, taken: a flag a package acts on once.
    pub fn take(&mut self, flag: &str) -> Option<String> {
        let at = self.given.iter().position(|(f, _)| *f == flag)?;
        Some(self.given.remove(at).1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flag_reads_back_and_a_later_value_replaces_it() {
        let mut flags = LaunchFlags::default();
        assert_eq!(flags.get("compare"), None);
        flags.set("compare", ",calm");
        flags.set("split", "true");
        flags.set("compare", "a,b");
        assert_eq!(flags.get("compare"), Some("a,b"));
        assert!(flags.has("split"));
        assert_eq!(flags.take("compare").as_deref(), Some("a,b"));
        assert!(!flags.has("compare"));
    }
}
