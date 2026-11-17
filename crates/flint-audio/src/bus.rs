//! Mixer buses as data (#730).
//!
//! Every mixer has `sfx` and `music`, under the main track (where the master
//! low-pass is). A game adds its own in its `flint.toml`, each with a parent
//! and a starting volume:
//!
//! ```toml
//! [audio.buses.voice]          # under the main track
//! volume = 0.9
//!
//! [audio.buses.ui]
//! parent = "sfx"               # turned down with sfx
//! volume = 0.7
//!
//! [audio.buses.music]          # a built-in bus takes a volume, not a parent
//! volume = 0.8
//! ```
//!
//! A [`Bus`] is a bus's number in its [`Buses`] table, which is also the
//! number the browser's worklet knows it by (`protocol::BUS_SFX`,
//! `BUS_MUSIC`, then a `Command::BusNew` each). A bus volume multiplies its
//! children's, as kira's tracks do.

use flint_core::{FlintError, Result};

/// The key in `flint.toml` that holds the audio settings.
pub const MANIFEST_TABLE: &str = "audio";

/// What a parent may be called to mean the main track.
pub const MAIN: &str = "master";

/// The keys `[audio.buses.<name>]` may hold.
const BUS_KEYS: [&str; 2] = ["parent", "volume"];

/// A mixer bus, by its number in the engine's [`Buses`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, PartialOrd, Ord)]
pub struct Bus(u8);

#[allow(non_upper_case_globals)]
impl Bus {
    /// Sound effects, and every placed sound unless it names another bus.
    pub const SFX: Bus = Bus(crate::remote::protocol::BUS_SFX);
    /// Music.
    pub const MUSIC: Bus = Bus(crate::remote::protocol::BUS_MUSIC);

    /// The name `Bus` had while it was an enum of two.
    #[deprecated(since = "0.1.0", note = "buses are data now (#730): use `Bus::SFX`")]
    pub const Sfx: Bus = Bus::SFX;
    /// The name `Bus` had while it was an enum of two.
    #[deprecated(since = "0.1.0", note = "buses are data now (#730): use `Bus::MUSIC`")]
    pub const Music: Bus = Bus::MUSIC;

    /// The built-in names, which every [`Buses`] starts with.
    pub const BUILT_IN: [&'static str; 2] = ["sfx", "music"];

    /// A built-in bus by name ("music"; anything else is sfx). A game's
    /// own buses are known only to its engine: [`Buses::get`].
    #[deprecated(
        since = "0.1.0",
        note = "buses are data now (#730): use `AudioEngine::bus(name)`"
    )]
    pub fn from_name(s: &str) -> Self {
        match s {
            "music" => Bus::MUSIC,
            _ => Bus::SFX,
        }
    }

    /// Its place in the table.
    pub fn index(self) -> usize {
        self.0 as usize
    }

    /// Its number in the worklet's protocol.
    pub(crate) fn code(self) -> u8 {
        self.0
    }
}

/// One bus: its name, the bus it hangs off (`None`: the main track) and
/// the volume it starts at (linear amplitude, 1 = unity).
#[derive(Debug, Clone, PartialEq)]
pub struct BusDecl {
    pub name: String,
    pub parent: Option<Bus>,
    pub volume: f64,
}

/// Every bus a mixer has, by number: `sfx`, `music`, then a game's. A
/// manifest's are numbered by name (TOML keeps a table sorted), each
/// parent before its children; [`declare`](Self::declare) adds one next.
#[derive(Debug, Clone, PartialEq)]
pub struct Buses {
    decls: Vec<BusDecl>,
}

impl Default for Buses {
    fn default() -> Self {
        Self::new()
    }
}

impl Buses {
    /// The two every mixer has, at unity.
    pub fn new() -> Self {
        Self {
            decls: Bus::BUILT_IN
                .iter()
                .map(|&name| BusDecl {
                    name: name.to_string(),
                    parent: None,
                    volume: 1.0,
                })
                .collect(),
        }
    }

    /// The buses a manifest's `[audio]` table declares, over the built-in
    /// two. `file` names the manifest in the errors. A missing `[audio]` or
    /// `[audio.buses]` is the built-in two.
    pub fn from_manifest(tables: &toml::Table, file: &str) -> Result<Self> {
        let mut buses = Self::new();
        let Some(audio) = tables.get(MANIFEST_TABLE) else {
            return Ok(buses);
        };
        let bad = |key: &str, what: &str| {
            FlintError::AudioError(format!("{file}: [audio] {key}: {what}"))
        };
        let audio = audio
            .as_table()
            .ok_or_else(|| bad("", "should be a table"))?;
        if let Some(key) = audio.keys().find(|k| k.as_str() != "buses") {
            return Err(bad(key, "unknown key (the one key is `buses`)"));
        }
        let Some(list) = audio.get("buses") else {
            return Ok(buses);
        };
        let list = list
            .as_table()
            .ok_or_else(|| bad("buses", "should be a table of buses"))?;
        // Read each, then make them parents first, so a bus may name one
        // declared after it.
        let mut wanted: Vec<(String, Option<String>, f64)> = Vec::new();
        for (name, decl) in list {
            let key = format!("buses.{name}");
            let decl = decl
                .as_table()
                .ok_or_else(|| bad(&key, "should be a table (parent, volume)"))?;
            if let Some(k) = decl.keys().find(|k| !BUS_KEYS.contains(&k.as_str())) {
                return Err(bad(
                    &format!("{key}.{k}"),
                    "unknown key (the keys are parent, volume)",
                ));
            }
            let parent = match decl.get("parent") {
                None => None,
                Some(toml::Value::String(p)) => Some(p.clone()),
                Some(_) => return Err(bad(&format!("{key}.parent"), "should be a bus name")),
            };
            let volume = match decl.get("volume") {
                None => 1.0,
                Some(v) => flint_core::toml_util::toml_f64(v)
                    .filter(|v| v.is_finite() && *v >= 0.0)
                    .ok_or_else(|| {
                        bad(&format!("{key}.volume"), "should be a number, 0 or more")
                    })?,
            };
            wanted.push((name.clone(), parent, volume));
        }
        let mut stack = Vec::new();
        for i in 0..wanted.len() {
            buses.make(i, &wanted, &mut stack, &bad)?;
        }
        Ok(buses)
    }

    /// The buses a project's `flint.toml` declares (read through the vfs, so
    /// a bundle's manifest counts), or the built-in two when it has none.
    pub fn for_project(project: &flint_core::project::Project) -> Result<Self> {
        let file = project.manifest_path();
        if !flint_core::vfs::is_file(&file) {
            return Ok(Self::new());
        }
        let shown = flint_core::project::shown(&file);
        let text = flint_core::vfs::read_to_string(&file)
            .map_err(|e| FlintError::AudioError(format!("{shown}: {e}")))?;
        let tables: toml::Table = text
            .parse()
            .map_err(|e| FlintError::AudioError(format!("{shown}: {e}")))?;
        Self::from_manifest(&tables, &shown)
    }

    /// Declare `wanted[i]`, after its parent, once.
    fn make(
        &mut self,
        i: usize,
        wanted: &[(String, Option<String>, f64)],
        stack: &mut Vec<usize>,
        bad: &dyn Fn(&str, &str) -> FlintError,
    ) -> Result<Bus> {
        let (name, parent, volume) = &wanted[i];
        let key = format!("buses.{name}");
        if let Some(bus) = self.get(name) {
            if Bus::BUILT_IN.contains(&name.as_str()) {
                // A built-in takes a volume here, never a new parent.
                if parent.as_deref().is_some_and(|p| p != MAIN) {
                    return Err(bad(
                        &format!("{key}.parent"),
                        "a built-in bus hangs off the main track",
                    ));
                }
                self.decls[bus.index()].volume = *volume;
            }
            return Ok(bus);
        }
        if stack.contains(&i) {
            let mut chain: Vec<&str> = stack.iter().map(|&j| wanted[j].0.as_str()).collect();
            chain.push(name);
            return Err(bad(
                &format!("{key}.parent"),
                &format!("a loop: {}", chain.join(" -> ")),
            ));
        }
        stack.push(i);
        let parent = match parent.as_deref() {
            None | Some(MAIN) => None,
            Some(p) => match self.get(p) {
                Some(bus) => Some(bus),
                None => match wanted.iter().position(|(n, ..)| n == p) {
                    Some(j) => Some(self.make(j, wanted, stack, bad)?),
                    None => {
                        let mut known: Vec<&str> = self.names().collect();
                        known.extend(wanted.iter().map(|(n, ..)| n.as_str()));
                        known.retain(|n| n != name);
                        known.sort();
                        known.dedup();
                        return Err(bad(
                            &format!("{key}.parent"),
                            &format!(
                                "no bus `{p}` (the buses are {}, and `{MAIN}` is the main track)",
                                known.join(", ")
                            ),
                        ));
                    }
                },
            },
        };
        stack.pop();
        self.declare(name, parent, *volume)
            .map_err(|e| bad(&key, &e.to_string()))
    }

    /// Add a bus under `parent` (`None`: the main track), or give one that
    /// exists a new starting volume. Its number is the next one.
    pub fn declare(&mut self, name: &str, parent: Option<Bus>, volume: f64) -> Result<Bus> {
        if let Some(bus) = self.get(name) {
            self.decls[bus.index()].volume = volume;
            return Ok(bus);
        }
        if name.is_empty() || name == MAIN {
            return Err(FlintError::AudioError(format!("`{name}` can't name a bus")));
        }
        if parent.is_some_and(|p| p.index() >= self.decls.len()) {
            return Err(FlintError::AudioError(format!(
                "bus `{name}`: its parent isn't a bus"
            )));
        }
        // u8::MAX is protocol::BUS_MAIN.
        if self.decls.len() >= u8::MAX as usize {
            return Err(FlintError::AudioError(format!(
                "bus `{name}`: a mixer has at most {} buses",
                u8::MAX
            )));
        }
        let bus = Bus(self.decls.len() as u8);
        self.decls.push(BusDecl {
            name: name.to_string(),
            parent,
            volume,
        });
        Ok(bus)
    }

    /// The bus with this name.
    pub fn get(&self, name: &str) -> Option<Bus> {
        self.decls
            .iter()
            .position(|d| d.name == name)
            .map(|i| Bus(i as u8))
    }

    /// The bus numbered `bus`.
    pub fn decl(&self, bus: Bus) -> Option<&BusDecl> {
        self.decls.get(bus.index())
    }

    /// Every bus, by number.
    pub fn iter(&self) -> impl Iterator<Item = (Bus, &BusDecl)> {
        self.decls
            .iter()
            .enumerate()
            .map(|(i, d)| (Bus(i as u8), d))
    }

    /// The buses' names, by number.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.decls.iter().map(|d| d.name.as_str())
    }

    pub fn len(&self) -> usize {
        self.decls.len()
    }

    pub fn is_empty(&self) -> bool {
        self.decls.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &str) -> Result<Buses> {
        Buses::from_manifest(&text.parse().unwrap(), "flint.toml")
    }

    #[test]
    fn a_manifest_adds_buses_parents_first_and_sets_a_built_ins_volume() {
        let buses = read(
            r#"
            [project]
            id = "x"
            [audio.buses.footsteps]
            parent = "foley"
            [audio.buses.foley]
            parent = "sfx"
            volume = 0.5
            [audio.buses.voice]
            volume = 0.9
            [audio.buses.music]
            volume = 0.8
            "#,
        )
        .unwrap();
        let names: Vec<&str> = buses.names().collect();
        assert_eq!(names, ["sfx", "music", "foley", "footsteps", "voice"]);
        let foley = buses.get("foley").unwrap();
        assert_eq!(buses.decl(foley).unwrap().parent, Some(Bus::SFX));
        assert_eq!(
            buses.decl(buses.get("footsteps").unwrap()).unwrap().parent,
            Some(foley)
        );
        assert_eq!(buses.decl(Bus::MUSIC).unwrap().volume, 0.8);
        assert_eq!(
            buses.decl(buses.get("voice").unwrap()).unwrap().parent,
            None
        );
        assert_eq!(read("").unwrap(), Buses::new());
    }

    #[test]
    fn a_wrong_bus_is_an_error_naming_the_file_and_key() {
        let err = |t: &str| read(t).unwrap_err().to_string();
        assert!(err("[audio.buses.ui]\nparent = \"hud\"\n")
            .contains("flint.toml: [audio] buses.ui.parent: no bus `hud`"));
        assert!(
            err("[audio.buses.a]\nparent = \"b\"\n[audio.buses.b]\nparent = \"a\"\n")
                .contains("a loop: a -> b -> a")
        );
        assert!(err("[audio.buses.ui]\nvolum = 1\n").contains("buses.ui.volum: unknown key"));
        assert!(err("[audio.buses.ui]\nvolume = -1\n").contains("0 or more"));
        assert!(err("[audio.buses.sfx]\nparent = \"music\"\n").contains("main track"));
        assert!(err("[audio.buses.master]\n").contains("can't name a bus"));
        assert!(err("[audio]\nbus = 1\n").contains("unknown key"));
        assert!(
            read("[audio.buses.ui]\nvolume = 1\n").is_ok(),
            "an integer volume"
        );
    }
}
