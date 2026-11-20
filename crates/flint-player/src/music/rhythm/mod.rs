//! The rhythm game (the `rhythm` feature, ADR 0117 decision 2): a
//! `music_session` with a `chart`, judged by flint-rhythm's `ChartSession`
//! as the suite plays (ADR 0019).
//!
//! Where it sits in the frame:
//!
//! - [`Stage::Clock`], before the clock ticks: the session's capture drain
//!   replaces the player's pad polling (it holds the pads, [`Pads::take`]),
//!   judges what came in and writes the frame's lean and pulse into input
//!   (ADR 0018). A session that ends on its own gives the pads back, and
//!   with `quit_on_finish` ends the run.
//! - [`Stage::PreScripts`]: the `conducted_*` snapshot and the music clock,
//!   for this frame's scripts (ADR 0020).
//! - [`Stage::Procgen`], before the renderer's sync: the ladder's blur,
//!   chromatic aberration and desaturation fill what scripts left of the
//!   frame's post values (ADR 0021); after the session, one restore.
//! - The scene unloading: the stems stop and the capture guard drops here,
//!   before the player clears its audio (ADR 0017).

#[cfg(feature = "debug-hud")]
pub(crate) mod guide_panel;
pub(crate) mod session;
#[cfg(feature = "debug-hud")]
pub(crate) mod timeline_panel;

use crate::music::conducted::ConductedSnapshot;
use crate::player_app::subsystem::{Pads, SceneCtx, Stage, StageCtx, Subsystem};
use flint_script::MusicClock;
use session::{LadderPostBase, MusicSession};
use std::path::PathBuf;

/// One scene's chart session, and what it changed.
pub(crate) struct RhythmSubsystem {
    base: PathBuf,
    comp: toml::Value,
    session: Option<MusicSession>,
    /// The scene's authored values of the three ladder-driven post params:
    /// the merge's base, and what a finished session restores (ADR 0021).
    post_base: LadderPostBase,
    post_restore: Option<LadderPostBase>,
    /// A start was tried with a device there; a device arriving later (the
    /// web's Begin click) tries once more.
    tried_with_device: bool,
    /// The guide and map panels are to be added, at the next stage.
    #[cfg(feature = "debug-hud")]
    panels_due: bool,
}

impl RhythmSubsystem {
    pub(crate) const NAME: &'static str = "rhythm";

    pub(crate) fn new(base: PathBuf, comp: toml::Value, ctx: &mut SceneCtx) -> Self {
        let authored = ctx
            .scene
            .post_process
            .as_ref()
            .map(|d| crate::player_app::scene_loading::post_process_config_from_def(d, ctx.styles))
            .unwrap_or_default();
        Self {
            base,
            comp,
            session: None,
            post_base: LadderPostBase {
                radial_blur: authored.radial_blur,
                chromatic_aberration: authored.chromatic_aberration,
                desaturate: authored.desaturate,
            },
            post_restore: None,
            tried_with_device: false,
            #[cfg(feature = "debug-hud")]
            panels_due: false,
        }
    }

    /// Start the session on the shared manager once there's a device, and
    /// hand it the pads.
    fn start(
        &mut self,
        audio: Option<&mut flint_audio::engine::AudioEngine>,
        pads: Option<&mut Pads>,
    ) {
        if self.session.is_some() || self.tried_with_device {
            return;
        }
        let Some(audio) = audio else { return };
        if audio.can_host().is_err() {
            return;
        }
        self.tried_with_device = true;
        match MusicSession::start(&self.base, &self.comp, audio) {
            Ok(Some(session)) => {
                self.session = Some(session);
                // The capture thread's rumble engine owns the motors now
                if let Some(pads) = pads {
                    pads.take();
                }
                tracing::info!(target: "music",
                    "gamepad handed to capture thread \
                     (player polling suspended for the session)"
                );
                #[cfg(feature = "debug-hud")]
                {
                    self.panels_due = true;
                }
            }
            Ok(None) => {}
            Err(e) => tracing::warn!(target: "music", "session failed to start: {e:#}"),
        }
    }

    /// The session is over: the stems stop with a fade, the capture guard
    /// drops, the pads go back to the player, and the scripts' view of it
    /// goes neutral.
    fn stop(&mut self, pads: Option<&mut Pads>, script: &mut flint_script::ScriptSystem) {
        let Some(session) = self.session.take() else {
            return;
        };
        session.stop();
        if let Some(pads) = pads {
            pads.give_back();
        }
        script.set_module_state(ConductedSnapshot::default());
        script.set_music(MusicClock::default());
        tracing::info!(target: "music", "session ended — gamepad returned to player polling");
    }

    /// The Music Guide (Backquote) and Manifest Map (Backslash), registered
    /// closed while a session can feed them (ADR 0035).
    #[cfg(feature = "debug-hud")]
    fn add_panels(&mut self, panels: &mut Vec<Box<dyn flint_debug_ui::DebugPanel>>) {
        if !std::mem::take(&mut self.panels_due) {
            return;
        }
        let Some(session) = &self.session else { return };
        if !panels
            .iter()
            .any(|p| p.name() == guide_panel::MUSIC_GUIDE_PANEL)
        {
            panels.push(Box::new(guide_panel::MusicGuidePanel::new()));
            tracing::info!(target: "music", "debug guide available — press ` (Backquote)");
        }
        if !panels
            .iter()
            .any(|p| p.name() == timeline_panel::MANIFEST_MAP_PANEL)
        {
            let mut panel = timeline_panel::ManifestMapPanel::new();
            panel.set_map(session.timeline_map());
            panels.push(Box::new(panel));
            tracing::info!(target: "music", "manifest map available — press \\ (Backslash)");
        }
    }

    /// Feed the open panels; a closed one costs nothing.
    #[cfg(feature = "debug-hud")]
    fn feed_panels(&self, panels: &mut [Box<dyn flint_debug_ui::DebugPanel>]) {
        let Some(session) = &self.session else { return };
        for panel in panels {
            if !panel.is_open() {
                continue;
            }
            if let Some(p) = panel
                .as_any_mut()
                .downcast_mut::<guide_panel::MusicGuidePanel>()
            {
                p.set_data(
                    session.visual_frame(),
                    session.guide_frame(guide_panel::GUIDE_HORIZON_BEATS),
                );
            } else if let Some(p) = panel
                .as_any_mut()
                .downcast_mut::<timeline_panel::ManifestMapPanel>()
            {
                p.set_frame(session.timeline_frame());
            }
        }
    }

    /// The frame's first step, before the clock ticks.
    fn drain(&mut self, ctx: &mut StageCtx) {
        self.start(ctx.audio.as_deref_mut(), ctx.pads.as_mut());
        #[cfg(feature = "debug-hud")]
        self.add_panels(ctx.panels);
        let Some(session) = &mut self.session else {
            return;
        };
        if session.tick(ctx.input) {
            // A natural mid-scene finish: the pads come back, the scene runs
            // on, unless the component asked to quit
            let quit = session.quit_on_finish();
            self.stop(ctx.pads.as_mut(), ctx.script);
            self.post_restore = Some(self.post_base);
            #[cfg(feature = "debug-hud")]
            ctx.panels.retain(|p| {
                p.name() != guide_panel::MUSIC_GUIDE_PANEL
                    && p.name() != timeline_panel::MANIFEST_MAP_PANEL
            });
            if quit {
                tracing::info!(target: "music", "quit_on_finish — exiting player");
                if let Some(flag) = ctx.quit.as_deref_mut() {
                    *flag = true;
                }
            }
            return;
        }
        #[cfg(feature = "debug-hud")]
        self.feed_panels(ctx.panels);
    }
}

impl Subsystem for RhythmSubsystem {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    /// The session's state for scripts (`music/conducted.rs`, #698).
    fn script_modules(&self) -> &[&'static str] {
        &["conducted"]
    }

    fn scene_loaded(&mut self, ctx: &mut SceneCtx) {
        self.start(ctx.audio.as_deref_mut(), ctx.pads.as_mut());
    }

    fn scene_unloading(&mut self, ctx: &mut SceneCtx) {
        // Before the player clears its audio (ADR 0017's order)
        self.stop(ctx.pads.as_mut(), ctx.script);
        // The restore belongs to this scene: the next one's config replaces
        // the renderer's whole
        self.post_restore = None;
    }

    fn update(&mut self, stage: Stage, ctx: &mut StageCtx) {
        match stage {
            Stage::Clock => self.drain(ctx),
            Stage::PreScripts => {
                if let Some(session) = &self.session {
                    let snapshot = session.conducted_snapshot();
                    ctx.script.set_music(MusicClock {
                        playing: true,
                        beat: snapshot.beat,
                        bar: snapshot.bar,
                        section: snapshot.section.clone(),
                    });
                    ctx.script.set_module_state(snapshot);
                }
            }
            // Before the renderer's sync takes the frame's post values.
            // Scripts' values win; the merge writes base + rung every frame,
            // so a recovered ladder restores the authored look
            Stage::Procgen => {
                let post = &mut ctx.requests.post;
                let mut blur = post.f32("radial_blur");
                let mut aberration = post.f32("chromatic_aberration");
                let mut desaturate = post.f32("desaturate");
                if let Some(session) = &self.session {
                    session::merge_ladder_postprocess(
                        &mut blur,
                        &mut aberration,
                        &mut desaturate,
                        &session.visual_frame(),
                        self.post_base,
                    );
                } else if let Some(base) = self.post_restore.take() {
                    session::restore_ladder_postprocess(
                        &mut blur,
                        &mut aberration,
                        &mut desaturate,
                        base,
                    );
                }
                post.set_f32("radial_blur", blur);
                post.set_f32("chromatic_aberration", aberration);
                post.set_f32("desaturate", desaturate);
            }
            _ => {}
        }
    }

    #[cfg(feature = "debug-hud")]
    fn debug_keys(&self) -> &'static [flint_runtime::DebugKey] {
        DEBUG_KEYS
    }

    #[cfg(feature = "debug-hud")]
    fn on_debug_key(&mut self, action: &str, key: &str, ctx: &mut StageCtx) {
        let panel = match action {
            MUSIC_GUIDE => guide_panel::MUSIC_GUIDE_PANEL,
            MANIFEST_MAP => timeline_panel::MANIFEST_MAP_PANEL,
            // Force the full reintegration (rewind, seam, reassembly) with
            // no fail to play to (ADR 0064 follow-up)
            FORCE_FAIL => {
                if let Some(session) = self.session.as_mut() {
                    session.debug_force_fail();
                    tracing::info!(target: "music", "DEBUG: full-fail forced — rewind incoming");
                } else {
                    ctx.note
                        .say(key, "the music session has ended — nothing to fail");
                }
                return;
            }
            _ => return,
        };
        let mut found = false;
        for p in ctx.panels.iter_mut().filter(|p| p.name() == panel) {
            p.toggle();
            found = true;
        }
        if !found {
            ctx.note
                .say(key, "the music session has ended — nothing to show");
        }
    }
}

#[cfg(feature = "debug-hud")]
const MUSIC_GUIDE: &str = "music_guide";
#[cfg(feature = "debug-hud")]
const MANIFEST_MAP: &str = "manifest_map";
#[cfg(feature = "debug-hud")]
const FORCE_FAIL: &str = "music_force_fail";

/// A charted session's debug keys (#717; ADR 0035's debug surface), the
/// `DEBUG_KEYS` of `player_app/subsystems/music.rs`.
#[cfg(feature = "debug-hud")]
pub(crate) const DEBUG_KEYS: &[flint_runtime::DebugKey] = &[
    flint_runtime::DebugKey {
        action: MUSIC_GUIDE,
        key: "Backquote",
        does: "The Music Guide panel",
    },
    flint_runtime::DebugKey {
        action: MANIFEST_MAP,
        key: "Backslash",
        does: "The Manifest Map panel",
    },
    flint_runtime::DebugKey {
        action: FORCE_FAIL,
        key: "F9",
        does: "Force the full reintegration: rewind, seam, reassembly",
    },
];
