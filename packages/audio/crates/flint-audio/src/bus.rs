//! Mixer buses as data (#730).
//!
//! Every mixer has `sfx` and `music`, under the main track (where the master
//! low-pass is). A game adds its own in its `flint.toml`, each with a parent
//! and a starting volume, and any bus may start muffled or duck under
//! another (#1218):
//!
//! ```toml
//! [audio.buses.voice]          # under the main track
//! volume = 0.9
//!
//! [audio.buses.ui]
//! parent = "sfx"               # turned down with sfx
//! volume = 0.7
//! lowpass = 6000               # its own low-pass, Hz (20000: open)
//!
//! [audio.buses.music]          # a built-in bus takes no parent
//! volume = 0.8
//! duck = { under = "voice", by_db = -9, attack = 0.08, release = 0.6 }
//! ```
//!
//! A [`Bus`] is a bus's number in its [`Buses`] table, which is also the
//! number the browser's worklet knows it by (`protocol::BUS_SFX`,
//! `BUS_MUSIC`, then a `Command::BusNew` each). A bus volume multiplies its
//! children's, as kira's tracks do; so does a duck, which is a volume.

use flint_core::{FlintError, Result};

/// The key in `flint.toml` that holds the audio settings.
pub const MANIFEST_TABLE: &str = "audio";

/// What a parent may be called to mean the main track.
pub const MAIN: &str = "master";

/// A bus low-pass's cutoff when it is open, Hz: every bus starts here
/// unless its table says `lowpass`.
pub const LOWPASS_OPEN: f64 = 20_000.0;

/// The lowest cutoff a bus low-pass takes, Hz.
pub const LOWPASS_MIN: f64 = 10.0;

/// The keys `[audio.buses.<name>]` may hold.
const BUS_KEYS: [&str; 4] = ["parent", "volume", "lowpass", "duck"];

/// The keys a `duck` table may hold.
const DUCK_KEYS: [&str; 4] = ["under", "by_db", "attack", "release"];

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

/// How a bus dips while anything plays on another (#1218): `by_db` down
/// (0 or less) over `attack` seconds once a sound starts on `under` (or on
/// a bus under it), and back over `release` once the last one there ends.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Duck {
    pub under: Bus,
    pub by_db: f64,
    pub attack: f64,
    pub release: f64,
}

impl Duck {
    /// The defaults a `duck` table leaves out: the music-under-dialogue
    /// dip, nine decibels in 80 ms and back over 0.6 s.
    pub const BY_DB: f64 = -9.0;
    pub const ATTACK: f64 = 0.08;
    pub const RELEASE: f64 = 0.6;

    /// What the duck multiplies the bus's volume by while it is down.
    pub fn gain(&self) -> f64 {
        10f64.powf(self.by_db / 20.0)
    }
}

/// One bus: its name, the bus it hangs off (`None`: the main track), the
/// volume it starts at (linear amplitude, 1 = unity), its low-pass's
/// starting cutoff (Hz, [`LOWPASS_OPEN`] for none) and its duck, if any.
#[derive(Debug, Clone, PartialEq)]
pub struct BusDecl {
    pub name: String,
    pub parent: Option<Bus>,
    pub volume: f64,
    pub lowpass: f64,
    pub duck: Option<Duck>,
}

/// A bus as a manifest asks for it, before the names are numbers.
struct Wanted {
    name: String,
    parent: Option<String>,
    volume: f64,
    lowpass: f64,
    /// The bus it ducks under, by name, and the rest of the duck.
    duck: Option<(String, Duck)>,
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

/// A number from a table: finite and passing `ok`.
fn number(v: &toml::Value, ok: impl Fn(f64) -> bool) -> Option<f64> {
    flint_core::toml_util::toml_f64(v).filter(|v| v.is_finite() && ok(*v))
}

impl Buses {
    /// The two every mixer has, at unity and open.
    pub fn new() -> Self {
        Self {
            decls: Bus::BUILT_IN
                .iter()
                .map(|&name| BusDecl {
                    name: name.to_string(),
                    parent: None,
                    volume: 1.0,
                    lowpass: LOWPASS_OPEN,
                    duck: None,
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
        let mut wanted: Vec<Wanted> = Vec::new();
        for (name, decl) in list {
            let key = format!("buses.{name}");
            let decl = decl
                .as_table()
                .ok_or_else(|| bad(&key, "should be a table (parent, volume, lowpass, duck)"))?;
            if let Some(k) = decl.keys().find(|k| !BUS_KEYS.contains(&k.as_str())) {
                return Err(bad(
                    &format!("{key}.{k}"),
                    "unknown key (the keys are parent, volume, lowpass, duck)",
                ));
            }
            let parent = match decl.get("parent") {
                None => None,
                Some(toml::Value::String(p)) => Some(p.clone()),
                Some(_) => return Err(bad(&format!("{key}.parent"), "should be a bus name")),
            };
            let volume = match decl.get("volume") {
                None => 1.0,
                Some(v) => number(v, |v| v >= 0.0).ok_or_else(|| {
                    bad(&format!("{key}.volume"), "should be a number, 0 or more")
                })?,
            };
            let lowpass = match decl.get("lowpass") {
                None => LOWPASS_OPEN,
                Some(v) => number(v, |v| (LOWPASS_MIN..=LOWPASS_OPEN).contains(&v))
                    .ok_or_else(|| {
                        bad(
                            &format!("{key}.lowpass"),
                            &format!(
                                "should be a cutoff in Hz, from {LOWPASS_MIN} to {LOWPASS_OPEN} (open)"
                            ),
                        )
                    })?,
            };
            let duck = match decl.get("duck") {
                None => None,
                Some(d) => Some(read_duck(d, &format!("{key}.duck"), &bad)?),
            };
            wanted.push(Wanted {
                name: name.clone(),
                parent,
                volume,
                lowpass,
                duck,
            });
        }
        let mut stack = Vec::new();
        for i in 0..wanted.len() {
            buses.make(i, &wanted, &mut stack, &bad)?;
        }
        // Every bus is made: each duck's `under` is a number now.
        for w in &wanted {
            let Some((under, duck)) = &w.duck else {
                continue;
            };
            let key = format!("buses.{}.duck.under", w.name);
            let bus = buses.get(&w.name).expect("made above");
            let Some(under) = buses.get(under) else {
                let known: Vec<&str> = buses.names().filter(|n| *n != w.name).collect();
                return Err(bad(
                    &key,
                    &format!("no bus `{under}` (the buses are {})", known.join(", ")),
                ));
            };
            buses
                .set_duck(bus, Some(Duck { under, ..*duck }))
                .map_err(|e| bad(&key, &e.to_string()))?;
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
        wanted: &[Wanted],
        stack: &mut Vec<usize>,
        bad: &dyn Fn(&str, &str) -> FlintError,
    ) -> Result<Bus> {
        let w = &wanted[i];
        let name = &w.name;
        let key = format!("buses.{name}");
        if let Some(bus) = self.get(name) {
            if Bus::BUILT_IN.contains(&name.as_str()) {
                // A built-in takes a volume and a low-pass here, never a
                // new parent.
                if w.parent.as_deref().is_some_and(|p| p != MAIN) {
                    return Err(bad(
                        &format!("{key}.parent"),
                        "a built-in bus hangs off the main track",
                    ));
                }
                let decl = &mut self.decls[bus.index()];
                decl.volume = w.volume;
                decl.lowpass = w.lowpass;
            }
            return Ok(bus);
        }
        if stack.contains(&i) {
            let mut chain: Vec<&str> = stack.iter().map(|&j| wanted[j].name.as_str()).collect();
            chain.push(name);
            return Err(bad(
                &format!("{key}.parent"),
                &format!("a loop: {}", chain.join(" -> ")),
            ));
        }
        stack.push(i);
        let parent = match w.parent.as_deref() {
            None | Some(MAIN) => None,
            Some(p) => match self.get(p) {
                Some(bus) => Some(bus),
                None => match wanted.iter().position(|o| o.name == p) {
                    Some(j) => Some(self.make(j, wanted, stack, bad)?),
                    None => {
                        let mut known: Vec<&str> = self.names().collect();
                        known.extend(wanted.iter().map(|o| o.name.as_str()));
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
        let bus = self
            .declare(name, parent, w.volume)
            .map_err(|e| bad(&key, &e.to_string()))?;
        self.decls[bus.index()].lowpass = w.lowpass;
        Ok(bus)
    }

    /// Add a bus under `parent` (`None`: the main track), open and with no
    /// duck, or give one that exists a new starting volume. Its number is
    /// the next one.
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
            lowpass: LOWPASS_OPEN,
            duck: None,
        });
        Ok(bus)
    }

    /// Give `bus` a starting low-pass cutoff, Hz (clamped to
    /// [`LOWPASS_MIN`]..=[`LOWPASS_OPEN`]).
    pub fn set_lowpass(&mut self, bus: Bus, hz: f64) {
        if let Some(d) = self.decls.get_mut(bus.index()) {
            d.lowpass = clamp_lowpass(hz);
        }
    }

    /// Make `bus` duck under `duck.under`, or not (`None`). A bus can't
    /// duck under itself, nor under a bus above or below it: its own
    /// sounds would count as the other's and turn it down.
    pub fn set_duck(&mut self, bus: Bus, duck: Option<Duck>) -> Result<()> {
        if bus.index() >= self.decls.len() {
            return Err(FlintError::AudioError(format!("no bus {}", bus.index())));
        }
        if let Some(d) = &duck {
            let under = self
                .decl(d.under)
                .ok_or_else(|| FlintError::AudioError(format!("no bus {}", d.under.index())))?;
            if d.under == bus {
                return Err(FlintError::AudioError(
                    "a bus can't duck under itself".into(),
                ));
            }
            if self.is_under(bus, d.under) || self.is_under(d.under, bus) {
                return Err(FlintError::AudioError(format!(
                    "`{}` is above or below `{}`, so its sounds would duck it too",
                    under.name,
                    self.decls[bus.index()].name
                )));
            }
        }
        self.decls[bus.index()].duck = duck;
        Ok(())
    }

    /// Whether `bus` hangs off `above`, at any depth.
    pub fn is_under(&self, bus: Bus, above: Bus) -> bool {
        let mut at = self.decl(bus).and_then(|d| d.parent);
        while let Some(p) = at {
            if p == above {
                return true;
            }
            at = self.decl(p).and_then(|d| d.parent);
        }
        false
    }

    /// `bus` and every bus above it, nearest first.
    pub fn with_parents(&self, bus: Bus) -> impl Iterator<Item = Bus> + '_ {
        std::iter::successors(Some(bus).filter(|b| self.decl(*b).is_some()), |b| {
            self.decl(*b).and_then(|d| d.parent)
        })
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

/// A cutoff a bus low-pass can take: [`LOWPASS_MIN`]..=[`LOWPASS_OPEN`]
/// Hz, open for anything not a number.
pub fn clamp_lowpass(hz: f64) -> f64 {
    if hz.is_nan() {
        return LOWPASS_OPEN;
    }
    hz.clamp(LOWPASS_MIN, LOWPASS_OPEN)
}

/// A `duck` table: the bus it ducks under (by name, made a number once
/// every bus is) and the rest, each checked.
fn read_duck(
    value: &toml::Value,
    key: &str,
    bad: &dyn Fn(&str, &str) -> FlintError,
) -> Result<(String, Duck)> {
    let table = value
        .as_table()
        .ok_or_else(|| bad(key, "should be a table (under, by_db, attack, release)"))?;
    if let Some(k) = table.keys().find(|k| !DUCK_KEYS.contains(&k.as_str())) {
        return Err(bad(
            &format!("{key}.{k}"),
            "unknown key (the keys are under, by_db, attack, release)",
        ));
    }
    let under = match table.get("under") {
        Some(toml::Value::String(u)) if u != MAIN => u.clone(),
        Some(toml::Value::String(_)) => {
            return Err(bad(
                &format!("{key}.under"),
                "should be a bus, not the main track",
            ))
        }
        Some(_) => return Err(bad(&format!("{key}.under"), "should be a bus name")),
        None => {
            return Err(bad(
                &format!("{key}.under"),
                "missing: the bus whose sounds duck this one",
            ))
        }
    };
    let read = |k: &str, default: f64, ok: fn(f64) -> bool, what: &str| match table.get(k) {
        None => Ok(default),
        Some(v) => number(v, ok).ok_or_else(|| bad(&format!("{key}.{k}"), what)),
    };
    let by_db = read(
        "by_db",
        Duck::BY_DB,
        |v| v <= 0.0,
        "should be decibels, 0 or less (a duck turns a bus down)",
    )?;
    let attack = read(
        "attack",
        Duck::ATTACK,
        |v| v >= 0.0,
        "should be seconds, 0 or more",
    )?;
    let release = read(
        "release",
        Duck::RELEASE,
        |v| v >= 0.0,
        "should be seconds, 0 or more",
    )?;
    Ok((
        under,
        Duck {
            under: Bus::SFX,
            by_db,
            attack,
            release,
        },
    ))
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

    /// #1218: a built-in bus ducks under a game's, named before it is
    /// made, and any bus may start muffled.
    #[test]
    fn a_bus_ducks_under_another_and_starts_at_its_lowpass() {
        let buses = read(
            r#"
            [audio.buses.music]
            duck = { under = "voice", by_db = -12, attack = 0.1 }
            [audio.buses.radio]
            parent = "sfx"
            lowpass = 3000
            [audio.buses.voice]
            "#,
        )
        .unwrap();
        let voice = buses.get("voice").unwrap();
        let music = buses.decl(Bus::MUSIC).unwrap();
        assert_eq!(
            music.duck,
            Some(Duck {
                under: voice,
                by_db: -12.0,
                attack: 0.1,
                release: Duck::RELEASE,
            })
        );
        assert!((music.duck.unwrap().gain() - 0.2512).abs() < 1e-3);
        assert_eq!(music.lowpass, LOWPASS_OPEN);
        let radio = buses.get("radio").unwrap();
        assert_eq!(buses.decl(radio).unwrap().lowpass, 3000.0);
        assert_eq!(
            buses.with_parents(radio).collect::<Vec<_>>(),
            [radio, Bus::SFX]
        );
        assert!(buses.is_under(radio, Bus::SFX) && !buses.is_under(Bus::SFX, radio));
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
        // #1218's keys.
        assert!(
            err("[audio.buses.ui]\nlowpass = 5\n").contains("buses.ui.lowpass: should be a cutoff")
        );
        assert!(err("[audio.buses.ui]\nlowpass = 30000\n").contains("from 10 to 20000"));
        assert!(err("[audio.buses.music]\nduck = { under = \"vox\" }\n")
            .contains("buses.music.duck.under: no bus `vox`"));
        assert!(err("[audio.buses.music]\nduck = { by_db = -3 }\n").contains("duck.under: missing"));
        assert!(
            err("[audio.buses.music]\nduck = { under = \"sfx\", by_db = 3 }\n")
                .contains("duck.by_db: should be decibels, 0 or less")
        );
        assert!(
            err("[audio.buses.music]\nduck = { under = \"sfx\", hold = 1 }\n")
                .contains("duck.hold: unknown key")
        );
        assert!(err("[audio.buses.music]\nduck = { under = \"music\" }\n")
            .contains("can't duck under itself"));
        assert!(
            err("[audio.buses.ui]\nparent = \"sfx\"\nduck = { under = \"sfx\" }\n")
                .contains("`sfx` is above or below `ui`")
        );
        assert!(err("[audio.buses.music]\nduck = { under = \"master\" }\n")
            .contains("not the main track"));
    }
}
