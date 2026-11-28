//! The `music` package's part in the player (#745, ADR 0117 step 5, ADR
//! 0137; its own crate since #1555): a subsystem, built for a scene with a
//! `music_session` component. A launcher lists [`PACKAGE`]; flint-player's
//! build script names it when the player's `music` feature turns this
//! crate's `player` feature on, so the player's source names none of it.
//!
//! - **The music** ([`MusicSubsystem`]): the suite's stems play on the buses
//!   its manifest names, sample-locked on the shared kira manager (ADR
//!   0017), its event script's bus changes fire on the musical grid, and
//!   scripts read where the music is (`music_beat()`, `music_bar()`,
//!   `music_section()`). In play the beat is the audio clock's,
//!   latency-compensated; in a stepped capture (`--frame-step`, `flint
//!   render --scripts`) and with no sound device it is the game clock's, so
//!   a beat lands on the same frame every run.
//! - **The chart judge** ([`ChartJudge`], a registration point): a component
//!   with a `chart` is judged by whatever subsystem the run's packages put
//!   there. The `rhythm` package fills it (`RhythmSubsystem`, its
//!   `ChartSession`, ADR 0018-0021), so this crate never asks whether the
//!   build has rhythm. With the point empty (a build without rhythm), a
//!   charted suite plays as music and says its chart isn't judged; its
//!   `conducted_*` read the neutral state, as a capture's do.
//!
//! Nothing here runs in a scene without the component.

pub mod conducted;

use crate::components::MUSIC_SESSION;
use crate::{Conductor, EventScript, SuiteManifest, SuiteSession};
use flint_core::package::InUse;
use flint_core::project::Project;
use flint_ecs::FlintWorld;
use flint_package::{Package, Registry, SceneCtx, Stage, StageCtx, Subsystem};
use flint_script::MusicClock;
use std::path::{Path, PathBuf};

/// The `music` package's player part: what a launcher lists.
pub static PACKAGE: Music = Music;

/// The `music` package (`packages/music/package.toml`).
pub struct Music;

impl Package for Music {
    fn name(&self) -> &'static str {
        "music"
    }

    /// Its builder, the line a scene with a session hears when the project
    /// leaves it out (#1528), its chart judge (empty until rhythm fills
    /// it), and the `conducted` module (#698): a rhythm session's state,
    /// enabled in a scene whose `music_session` has a chart (`music_beat()`
    /// and the rest are flint-script's core: any scene asks them). The music
    /// plays on under a pause menu, as a loop does: no game state pauses it.
    fn register(&self, registry: &mut Registry) {
        registry
            .scene_subsystem(for_scene)
            .when_left_out(left_out)
            .script_modules(&[&conducted::CONDUCTED])
            .insert_point(ChartJudge::default());
    }
}

/// A charted session's subsystem, made by the package that judges charts:
/// the project-root `base` its paths start from, the component, and the
/// scene.
pub type JudgeChart =
    fn(base: PathBuf, comp: toml::Value, ctx: &mut SceneCtx) -> Box<dyn Subsystem>;

/// Music's registration point for a chart's judge (ADR 0137 section 4):
/// `registry.point_mut::<ChartJudge>()` at a later package's `register`.
/// Empty, a charted suite plays as music.
#[derive(Default, Clone, Copy)]
pub struct ChartJudge {
    pub judge: Option<JudgeChart>,
}

/// What a scene with a `music_session` hears in a project that leaves the
/// package out: the line to add to `[packages]`.
fn left_out(ctx: &SceneCtx) -> Option<String> {
    left_out_line(ctx.packages?, ctx.world)
}

/// [`flint_package::left_out_line`] of the music: a `music_session` that
/// doesn't play.
pub fn left_out_line(in_use: &InUse, world: &FlintWorld) -> Option<String> {
    let uses: &[&str] = if world.entities_with_component(MUSIC_SESSION).is_empty() {
        &[]
    } else {
        &[MUSIC_SESSION]
    };
    flint_package::left_out_line(
        in_use,
        "music",
        uses,
        "doesn't play, and music_beat() reads -1",
    )
}

/// Stems fade out over this when the scene goes: long enough not to click.
pub const STOP_FADE_MS: f64 = 50.0;

/// The first `music_session` component in the world; a second one is
/// warned about and left alone.
pub fn component_in(world: &FlintWorld) -> Option<toml::Value> {
    let mut found = None;
    for entity in world.all_entities() {
        let comp = world
            .get_components(entity.id)
            .and_then(|c| c.get(MUSIC_SESSION).cloned());
        if let Some(comp) = comp {
            if found.is_some() {
                tracing::warn!(
                    "multiple music_session components in scene; using the first \
                     (extra on entity {})",
                    entity.id
                );
            } else {
                found = Some(comp);
            }
        }
    }
    found
}

/// A string field of the component; empty or whitespace counts as absent.
pub fn comp_str(comp: &toml::Value, key: &str) -> Option<String> {
    comp.get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

/// Where the suite's paths start: the project root (ADR 0114), or for one
/// release the root the old guess found, with a warning.
fn base_dir(project: &Project, comp: &toml::Value, scene_path: &str) -> PathBuf {
    let manifest = comp_str(comp, "manifest").unwrap_or_default();
    let scene_dir = Path::new(scene_path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    project.root_holding(MUSIC_SESSION, manifest, scene_dir)
}

/// The scene's music subsystem, if it has a `music_session`: the chart
/// judge's for a component with a `chart` (when a package filled
/// [`ChartJudge`]), the music's otherwise. In a capture (`flint render
/// --scripts`), the music's alone and only for scripts that run: the beat
/// on the game clock, no sound, no chart judged (#1252).
pub fn for_scene(ctx: &mut SceneCtx) -> Option<Box<dyn Subsystem>> {
    if ctx.capture.as_ref().is_some_and(|c| !c.scripts) {
        return None;
    }
    let comp = component_in(ctx.world)?;
    let base = base_dir(ctx.project, &comp, ctx.scene_path);
    if ctx.capture.is_none() && comp_str(&comp, "chart").is_some() {
        match ctx.registry.point::<ChartJudge>().and_then(|j| j.judge) {
            Some(judge) => return Some(judge(base, comp, ctx)),
            None => tracing::warn!(target: "music",
                "this player is built without the `rhythm` package: the suite plays, \
                 its chart isn't judged"
            ),
        }
    }
    match MusicSubsystem::new(&base, &comp) {
        Ok(music) => Some(Box::new(music)),
        Err(e) => {
            tracing::warn!(target: "music", "{e}");
            None
        }
    }
}

/// Where the music is on the game clock, for a capture and with no device:
/// the count-in (the session's pre-roll) and then the suite, as the audio
/// clock would have it with no latency.
pub struct GameClockMusic {
    conductor: Conductor,
    sample_rate: f64,
    preroll_samples: i64,
    /// Game time at the count-in's start, set on the first frame.
    started_at: Option<f64>,
}

impl GameClockMusic {
    pub fn new(manifest: &SuiteManifest) -> Self {
        let bpm = manifest.tempo.first().map_or(120.0, |a| a.bpm);
        let sample_rate = manifest.sample_rate as f64;
        Self {
            conductor: Conductor::new(manifest, None),
            sample_rate,
            preroll_samples: (crate::session::PREROLL_BEATS * sample_rate * 60.0 / bpm).round()
                as i64,
            started_at: None,
        }
    }

    /// The suite sample at this frame's game time. The first frame is the
    /// count-in's first `delta`, as the clock ticked it.
    pub fn sample(&mut self, total_time: f64, delta: f64) -> i64 {
        let start = *self.started_at.get_or_insert(total_time - delta);
        ((total_time - start) * self.sample_rate).round() as i64 - self.preroll_samples
    }

    /// The scripts' view of the music at this frame's game time.
    pub fn clock(&mut self, total_time: f64, delta: f64) -> MusicClock {
        let sample = self.sample(total_time, delta);
        clock_at(&self.conductor, sample)
    }
}

/// The scripts' view of a position.
pub fn clock_at(conductor: &Conductor, sample: i64) -> MusicClock {
    let pos = conductor.position_at_sample(sample);
    MusicClock {
        playing: true,
        beat: pos.beat,
        bar: pos.bar,
        section: conductor
            .section_at_sample(sample)
            .map(|s| s.name.clone())
            .unwrap_or_default(),
    }
}

/// A scene's music: the suite playing, and the beat for scripts.
pub struct MusicSubsystem {
    manifest: SuiteManifest,
    events: Option<EventScript>,
    base: PathBuf,
    latency_ms: Option<f64>,
    /// The suite on the engine's mixer (kira here, or the browser's
    /// worklet, #1216); `None` with no device, where the beat still counts
    /// on the game clock.
    session: Option<SuiteSession>,
    /// The mixer the session was started on (`AudioEngine::mixer_epoch`):
    /// when the mix moves (the worklet failing), its handles went with the
    /// old one and the suite starts again on the new.
    hosted_on: u32,
    /// The beat on the game clock (a stepped capture, no device).
    game_clock: GameClockMusic,
    /// Whether a start has been tried with a device there, so a failed one
    /// isn't tried every frame; a device arriving later (the web's Begin
    /// click) tries once more.
    tried_with_device: bool,
    /// Whether the component has a chart that nothing here judges (a
    /// capture, a player built without `rhythm`): the `conducted` module
    /// is still enabled, reading the neutral state, so the scene's scripts
    /// run as they do in the rhythm game before its session starts (#698).
    charted: bool,
}

impl MusicSubsystem {
    pub const NAME: &'static str = "music";

    /// The suite of a `music_session` component, validated against
    /// flint-music's contract and its stems on disk.
    pub fn new(base: &Path, comp: &toml::Value) -> Result<Self, String> {
        let manifest_rel =
            comp_str(comp, "manifest").ok_or("music_session component: `manifest` is required")?;
        let manifest_path = base.join(&manifest_rel);
        let manifest = SuiteManifest::load(&manifest_path)
            .map_err(|e| format!("music_session: {}: {e}", manifest_path.display()))?;
        let mut issues = crate::validate_manifest(&manifest);
        issues.extend(crate::validate_manifest_assets(&manifest, base));
        if !issues.is_empty() {
            let lines: Vec<String> = issues.iter().map(|i| format!("  {i}")).collect();
            return Err(format!(
                "suite '{}' ({}) failed validation, not playing it:\n{}",
                manifest.id,
                manifest_path.display(),
                lines.join("\n")
            ));
        }
        let events = match comp_str(comp, "events") {
            None => None,
            Some(rel) => {
                let path = base.join(&rel);
                Some(
                    EventScript::load(&path)
                        .map_err(|e| format!("music_session: {}: {e}", path.display()))?,
                )
            }
        };
        let latency_ms = crate::latency::latest_latency_ms(base).map(|(_, ms)| ms);
        Ok(Self {
            game_clock: GameClockMusic::new(&manifest),
            manifest,
            events,
            base: base.to_path_buf(),
            latency_ms,
            session: None,
            hosted_on: 0,
            tried_with_device: false,
            charted: comp_str(comp, "chart").is_some(),
        })
    }

    /// Start the suite on the engine's mixer, once there's a device.
    fn start(&mut self, audio: Option<&mut flint_audio::engine::AudioEngine>) {
        let Some(audio) = audio else { return };
        if self.session.is_some() && audio.mixer_epoch() != self.hosted_on {
            // Its handles went with the mixer they were made on
            tracing::warn!(target: "music",
                "suite '{}': the mix moved to the {}; starting it again there",
                self.manifest.id,
                audio.mixer_name()
            );
            self.session = None;
            self.tried_with_device = false;
        }
        if self.session.is_some() || self.tried_with_device {
            return;
        }
        let stems = crate::session::FileStems::new(&self.base);
        // Hosted on the engine's mixer (#730): not while silent (the web's
        // Begin is still to come); in the browser, in its worklet (#1216).
        let Ok(started) =
            audio.host(|mixer| SuiteSession::start(&self.manifest, mixer, &stems, self.latency_ms))
        else {
            return;
        };
        self.tried_with_device = true;
        self.hosted_on = audio.mixer_epoch();
        match started {
            Ok(mut session) => {
                if let Some(events) = &self.events {
                    for ev in &events.events {
                        session.scheduler.push(ev.clone(), &session.conductor);
                    }
                }
                let buses: Vec<&str> = self.manifest.buses_in_order().map(|(n, _)| n).collect();
                tracing::info!(target: "music",
                    "suite '{}' playing on the {}: {} bus(es) ({}), {}",
                    self.manifest.id,
                    audio.mixer_name(),
                    buses.len(),
                    buses.join(", "),
                    match self.latency_ms {
                        Some(ms) => format!("{ms:.1} ms latency compensated"),
                        None => "no latency on record".to_string(),
                    }
                );
                self.session = Some(session);
            }
            Err(e) => {
                tracing::warn!(target: "music", "suite '{}' failed to start: {e}", self.manifest.id)
            }
        }
    }

    /// The stems fade out; the session's handles go before the manager
    /// (ADR 0017), which this subsystem never owns.
    fn stop(&mut self) {
        if let Some(mut session) = self.session.take() {
            session.stop(STOP_FADE_MS);
            tracing::info!(target: "music", "suite '{}' stopped", self.manifest.id);
        }
    }

    /// Where the music is this frame, and the scheduler's events that fall
    /// due. The game clock answers in a stepped run and with no device.
    fn clock(&mut self, ctx: &StageCtx) -> MusicClock {
        if let Some(session) = &mut self.session {
            // Bus changes enter kira's schedule ahead of time
            session.pump();
        }
        let stepped = ctx.clock.frame_step.is_some();
        match &self.session {
            Some(session) if !stepped => {
                let sample = session.now().sample;
                clock_at(&session.conductor, sample)
            }
            _ => self
                .game_clock
                .clock(ctx.clock.total_time, ctx.clock.delta_time),
        }
    }
}

impl Subsystem for MusicSubsystem {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn script_modules(&self) -> &[&'static str] {
        if self.charted {
            &["conducted"]
        } else {
            &[]
        }
    }

    fn scene_loaded(&mut self, ctx: &mut SceneCtx) {
        self.start(flint_audio::engine::of_mut(ctx.resources));
    }

    fn scene_unloading(&mut self, ctx: &mut SceneCtx) {
        self.stop();
        ctx.script.set_music(MusicClock::default());
    }

    fn update(&mut self, stage: Stage, ctx: &mut StageCtx) {
        match stage {
            // A device that arrived since (the web's Begin click)
            Stage::Clock => self.start(flint_audio::engine::of_mut(ctx.resources)),
            Stage::PreScripts => {
                let clock = self.clock(ctx);
                ctx.script.set_music(clock);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod clock_tests {
    use super::*;

    const SUITE: &str = r#"
schema_version = 0
[suite]
id = "t"
title = "T"
[audio]
sample_rate = 48000
[[tempo]]
sample = 0
bpm = 120.0
time_signature = [4, 4]
[[sections]]
name = "intro"
start_sample = 0
[[sections]]
name = "verse"
start_sample = 192000
[buses.pad]
file = "pad.wav"
"#;

    #[test]
    fn the_game_clock_counts_in_then_plays_the_suite() {
        let manifest = SuiteManifest::parse(SUITE).unwrap();
        let mut music = GameClockMusic::new(&manifest);
        // Frame one at 1/60 s: the count-in is two beats (1 s at 120 bpm)
        let first = music.clock(1.0 / 60.0, 1.0 / 60.0);
        assert!(first.beat < 0.0 && first.beat > -2.0, "{first:?}");
        assert_eq!(first.section, "");
        // 1 s of count-in, then 0.5 s of the suite: beat 1, bar 0, intro
        let at = music.clock(1.5, 1.0 / 60.0);
        assert!((at.beat - 1.0).abs() < 1e-9, "{at:?}");
        assert_eq!((at.bar, at.section.as_str()), (0, "intro"));
        // Bar 2 (4 s into the suite) is the verse, from sample 192000
        let later = music.clock(5.0, 1.0 / 60.0);
        assert_eq!((later.bar, later.section.as_str()), (2, "verse"));
    }
}

#[cfg(test)]
#[path = "left_out_tests.rs"]
mod left_out_tests;
