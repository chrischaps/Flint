//! The world written to a file at game times (#713): `--dump-world FILE`,
//! `--dump-world-at SECS[,SECS...]`, `--dump-query QUERY`.
//!
//! Each time asked for is taken on the first frame at or after it, at
//! [`Stage::Sync`] (the frame's world is whole: scripts, physics, animation
//! and procgen have run, and `--exit-after`'s check comes after it, so a
//! dump at the exit time is the run's last world). The file is one JSON
//! document, written again whole after each snapshot through a temporary
//! file, so it is always complete:
//!
//! ```json
//! {
//!   "scene": "levels/demo.scene.toml",
//!   "query": "entities",
//!   "snapshots": [
//!     {"at": 2.0, "time": 2.0, "frame": 121, "entities": [
//!       {"id": 3, "name": "door", "archetype": "door", "components": ["door", "transform"],
//!        "data": {"door": {"locked": true}, "transform": {"position": [0, 0, 4]}}}
//!     ]}
//!   ]
//! }
//! ```
//!
//! The entities are those `--dump-query` picks (all of them by default), in
//! name order, each as `flint query --data` and `flint entity show` print
//! one (`flint_query::EntityDetails`). A time the run never reaches makes
//! the run fail (`run_failure`), as a screenshot that was never written
//! does (#639): a run that asked for the world says why it hasn't it.

use super::subsystem::{Lasts, Stage, StageCtx, Subsystem};
use flint_query::{parse_query, Query, Snapshot};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// How far short of a time a stepped clock can be and still have reached
/// it, as `--screenshot-at` allows: 210 steps of 1/60 make a hair under 3.5.
const DUE_TOLERANCE: f64 = 1e-6;

/// The document `--dump-world` writes.
#[derive(Serialize)]
struct Document<'a> {
    scene: &'a str,
    query: &'a str,
    snapshots: &'a [Snapshot],
}

/// `--dump-world` as one of the run's subsystems.
pub struct WorldDump {
    path: PathBuf,
    query_text: String,
    query: Query,
    /// The times asked for, in order.
    times: Vec<f64>,
    /// The next of `times` still to come.
    next: usize,
    snapshots: Vec<Snapshot>,
    /// Frames of the run so far, this one counted.
    frame: u64,
    /// The last frame's game time, so a time never reached can say how far
    /// the run got.
    last_time: Option<f64>,
    /// Why the file couldn't be written, the last time it couldn't.
    failure: Option<String>,
}

impl WorldDump {
    /// A dump of the entities `query` picks to `path` at each of `times`
    /// (game seconds, 0 or more, in any order). Refuses a query that doesn't
    /// parse, or a bad time, before anything runs.
    pub fn new(path: PathBuf, mut times: Vec<f64>, query: &str) -> anyhow::Result<Self> {
        let parsed =
            parse_query(query).map_err(|e| anyhow::anyhow!("--dump-query {query:?}: {e}"))?;
        if let Some(t) = times.iter().find(|t| !t.is_finite() || **t < 0.0) {
            anyhow::bail!("--dump-world-at {t}: a game time in seconds, 0 or later");
        }
        times.sort_by(f64::total_cmp);
        Ok(Self {
            path,
            query_text: query.to_owned(),
            query: parsed,
            times,
            next: 0,
            snapshots: Vec::new(),
            frame: 0,
            last_time: None,
            failure: None,
        })
    }

    /// One frame at game time `now`: a snapshot for every time it has
    /// reached, then the file again. Returns how many it took.
    pub fn frame(&mut self, world: &flint_ecs::FlintWorld, scene: &str, now: f64) -> usize {
        self.frame += 1;
        self.last_time = Some(now);
        let start = self.next;
        while self.next < self.times.len() && self.times[self.next] <= now + DUE_TOLERANCE {
            let at = self.times[self.next];
            self.snapshots
                .push(Snapshot::take(world, &self.query, at, now, self.frame));
            self.next += 1;
        }
        let taken = self.next - start;
        if taken > 0 {
            match self.write(scene) {
                Ok(()) => {
                    self.failure = None;
                    // The player's one log (#716), under `inspect`, which --quiet keeps
                    let entities = self.snapshots.last().map_or(0, |s| s.entities.len());
                    tracing::info!(
                        target: "inspect",
                        path = %self.path.display(),
                        time = now,
                        frame = self.frame,
                        entities,
                        "[dump] {} at t={now:.3}s (frame {}): {entities} entities",
                        self.path.display(),
                        self.frame,
                    );
                }
                Err(why) => {
                    tracing::warn!(target: "inspect", "--dump-world: {why}");
                    self.failure = Some(why);
                }
            }
        }
        taken
    }

    fn write(&self, scene: &str) -> Result<(), String> {
        let document = Document {
            scene,
            query: &self.query_text,
            snapshots: &self.snapshots,
        };
        let text = serde_json::to_string_pretty(&document).map_err(|e| e.to_string())?;
        if let Some(parent) = self.path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        crate::user_files::write(&self.path, &text)
            .map_err(|e| format!("cannot write {}: {e}", self.path.display()))
    }

    /// The file it writes.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Why the run doesn't have every snapshot it asked for, if it hasn't:
    /// the file couldn't be written, or the run ended before a time.
    pub fn missing(&self) -> Option<String> {
        if let Some(why) = &self.failure {
            return Some(format!("--dump-world {}: {why}", self.path.display()));
        }
        let t = self.times.get(self.next)?;
        Some(match self.last_time {
            None => format!(
                "--dump-world {}: no frame ran, so --dump-world-at {t} never came",
                self.path.display()
            ),
            Some(last) => format!(
                "--dump-world {}: the run ended at t={last:.3}s, before --dump-world-at {t}",
                self.path.display()
            ),
        })
    }
}

impl Subsystem for WorldDump {
    fn name(&self) -> &'static str {
        "world dump"
    }

    fn lasts(&self) -> Lasts {
        Lasts::Run
    }

    fn update(&mut self, stage: Stage, ctx: &mut StageCtx) {
        if stage == Stage::Sync {
            self.frame(ctx.world, ctx.scene_path, ctx.clock.total_time);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_methods)] // the test reads back the file the dump wrote
    use super::*;
    use flint_ecs::FlintWorld;

    fn world_with_door(locked: bool) -> FlintWorld {
        let mut world = FlintWorld::new();
        let id = world.spawn("door").unwrap();
        let mut c = world.get_components_mut(id).unwrap();
        c.set("door", toml::toml! { locked = locked }.into());
        world.spawn("lamp").unwrap();
        world
    }

    /// A fresh folder of this test's own.
    fn temp(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("flint_713_{test}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn read(path: &Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn each_time_is_taken_on_the_first_frame_at_or_after_it() {
        let path = temp("times").join("out/world.json");
        let mut dump =
            WorldDump::new(path.clone(), vec![0.05, 0.0], "entities where door == true").unwrap();
        let step = 1.0 / 60.0;
        let mut t = 0.0;
        for frame in 0..6 {
            let world = world_with_door(frame >= 3);
            dump.frame(&world, "demo.scene.toml", t);
            t += step;
        }
        assert!(dump.missing().is_none(), "{:?}", dump.missing());
        let doc = read(&path);
        assert_eq!(doc["scene"], "demo.scene.toml");
        assert_eq!(doc["query"], "entities where door == true");
        let snaps = doc["snapshots"].as_array().unwrap();
        assert_eq!(snaps.len(), 2, "sorted, one a time: {doc}");
        assert_eq!(snaps[0]["at"], 0.0);
        assert_eq!(snaps[0]["frame"], 1);
        assert_eq!(snaps[0]["entities"][0]["data"]["door"]["locked"], false);
        // 3 steps of 1/60 are a hair under 0.05: the frame at 0.05 still counts
        assert_eq!(snaps[1]["frame"], 4);
        assert_eq!(snaps[1]["entities"][0]["data"]["door"]["locked"], true);
        assert_eq!(
            snaps[1]["entities"].as_array().unwrap().len(),
            1,
            "the query picks the door, not the lamp"
        );
    }

    #[test]
    fn a_time_the_run_never_reaches_fails_the_run() {
        let mut dump =
            WorldDump::new(temp("never").join("w.json"), vec![1.0, 9.0], "entities").unwrap();
        assert!(dump.missing().unwrap().contains("no frame ran"));
        dump.frame(&world_with_door(true), "s", 1.0);
        dump.frame(&world_with_door(true), "s", 2.5);
        let why = dump.missing().unwrap();
        assert!(
            why.contains("ended at t=2.500s") && why.contains("--dump-world-at 9"),
            "{why}"
        );
        assert_eq!(read(dump.path())["snapshots"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn a_bad_query_or_time_is_refused_before_the_run() {
        let err = WorldDump::new("w.json".into(), vec![0.0], "entities where")
            .err()
            .unwrap();
        assert!(err.to_string().contains("--dump-query"), "{err}");
        let err = WorldDump::new("w.json".into(), vec![-1.0], "entities")
            .err()
            .unwrap();
        assert!(err.to_string().contains("--dump-world-at -1"), "{err}");
    }
}
