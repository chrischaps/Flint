//! Flint Audio — Spatial audio system (Kira backend)
//!
//! Provides audio playback for the Flint engine:
//! - `AudioEngine` — wraps Kira AudioManager, sound loading, listener management
//! - `AudioSync` — bridges TOML `audio_source` components to Kira spatial tracks
//! - `AudioTrigger` — maps GameEvents to sound playback (collision, interaction)
//! - `AudioSystem` — implements `RuntimeSystem` for game loop integration
//!
//! In the browser the mix runs in an `AudioWorklet`, off the main thread
//! (#136): `remote` writes the engine's calls for it, `worklet` opens it,
//! and `web` is the main-thread stream it falls back to. `page` suspends
//! both while the page is hidden (#162).

pub mod decode;
pub mod engine;
pub mod handle;
pub mod meter;
pub mod page;
pub mod remote;
pub mod sync;
pub mod trigger;
#[cfg(target_arch = "wasm32")]
pub mod web;
#[cfg(target_arch = "wasm32")]
pub mod worklet;

use engine::AudioEngine;
pub use engine::Bus;
use flint_core::{Result, Vec3};
use flint_ecs::FlintWorld;
use flint_runtime::{GameEvent, RuntimeSystem};
use sync::AudioSync;
use trigger::{AudioCommand, AudioTrigger};

/// Top-level audio system integrating engine, sync, and triggers
pub struct AudioSystem {
    pub engine: AudioEngine,
    pub sync: AudioSync,
    pub triggers: AudioTrigger,
}

impl Default for AudioSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioSystem {
    pub fn new() -> Self {
        Self {
            engine: AudioEngine::new(),
            sync: AudioSync::new(),
            triggers: AudioTrigger::new(),
        }
    }

    /// Open the audio device if it isn't open, and pick up what
    /// [`initialize`](RuntimeSystem::initialize) skipped while silent: the
    /// listener, the trigger rules and every `audio_source` already in the
    /// world, so looping sources begin from the click. In the browser, call
    /// it from the user gesture that begins the game; until then the game
    /// runs silent. Says whether there is sound.
    pub fn start(&mut self, world: &FlintWorld) -> bool {
        if !self.engine.start() {
            return false;
        }
        if !self.engine.has_listener() {
            if let Err(e) = self.engine.create_listener(Vec3::new(0.0, 1.0, 0.0)) {
                tracing::warn!("{e:?}");
            }
            self.triggers.load_rules(world);
            self.sync.sync_new_sources(world, &mut self.engine);
        }
        true
    }

    /// Clear sync and trigger state for a scene transition.
    /// Preserves the AudioEngine and its sound_cache (reusable across scenes).
    pub fn clear(&mut self) {
        self.sync.clear();
        self.triggers = AudioTrigger::new();
        self.engine.clear_oneshots();
    }

    /// Update the listener position (called from PlayerApp after camera update)
    pub fn update_listener(&mut self, position: Vec3, yaw: f32, pitch: f32) {
        self.engine.update_listener(position, yaw, pitch);
    }

    /// Set the low-pass filter cutoff frequency on the master bus (Hz)
    pub fn set_filter_cutoff(&mut self, hz: f32) {
        self.engine.set_filter_cutoff(hz);
    }

    /// Set a mixer bus volume (linear amplitude; 0.0 mutes the bus)
    pub fn set_bus_volume(&mut self, bus: Bus, volume: f64) {
        self.engine.set_bus_volume(bus, volume);
    }

    /// Process game events and execute resulting audio commands
    pub fn process_events(&mut self, events: &[GameEvent], world: &FlintWorld) {
        if !self.engine.is_available() {
            return;
        }
        let commands = self.triggers.process_events(events, world);

        for cmd in commands {
            match cmd {
                AudioCommand::Play {
                    sound,
                    position,
                    volume,
                } => {
                    if let Some(pos) = position {
                        if let Err(e) = self.engine.play_at_position(&sound, pos, volume, 1.0) {
                            tracing::warn!("{:?}", e);
                        }
                    } else if let Err(e) =
                        self.engine
                            .play_non_spatial(&sound, volume, 1.0, false, Bus::Sfx)
                    {
                        // One-shot non-spatial: handle not needed
                        tracing::warn!("{:?}", e);
                    }
                }
                AudioCommand::Stop { entity: _ } => {
                    // Track-level stop would require keeping sound handles;
                    // for now one-shot sounds just play to completion
                }
            }
        }
    }
}

impl RuntimeSystem for AudioSystem {
    fn initialize(&mut self, world: &mut FlintWorld) -> Result<()> {
        if !self.engine.is_available() {
            // No device, or (in the browser) no click yet: `start` picks up
            // the rest.
            tracing::warn!("No sound yet, running silent");
            return Ok(());
        }

        // Create listener at origin (will be repositioned by PlayerApp)
        self.engine.create_listener(Vec3::new(0.0, 1.0, 0.0))?;

        // Load trigger rules
        self.triggers.load_rules(world);

        // Initial sync of audio sources
        self.sync.sync_new_sources(world, &mut self.engine);

        Ok(())
    }

    fn fixed_update(&mut self, _world: &mut FlintWorld, _dt: f64) -> Result<()> {
        // Audio doesn't need fixed timestep
        Ok(())
    }

    fn update(&mut self, world: &mut FlintWorld, _dt: f64) -> Result<()> {
        if !self.engine.is_available() {
            return Ok(());
        }

        // The browser's worklet failed after it started and the main thread
        // took over (#136): what played there is gone, so start it again.
        self.engine.poll();
        if self.engine.take_restarted() {
            tracing::warn!("Audio mix restarted on the main thread; replaying sources");
            self.sync.clear();
            if let Err(e) = self.engine.create_listener(Vec3::new(0.0, 1.0, 0.0)) {
                tracing::warn!("{e:?}");
            }
        }

        // Sync any new audio source entities
        self.sync.sync_new_sources(world, &mut self.engine);

        // Update spatial positions for moving sources
        self.sync.update_positions(world);

        // Propagate per-frame pitch/volume changes from ECS to Kira handles
        self.sync.update_parameters(world);

        // Reload trigger rules for any new entities
        self.triggers.load_rules(world);

        // Clean up finished one-shot spatial sounds
        self.engine.cleanup_finished_oneshots();

        Ok(())
    }

    fn shutdown(&mut self) -> Result<()> {
        // Kira cleans up when AudioManager is dropped
        Ok(())
    }

    fn name(&self) -> &str {
        "audio"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audio_system_graceful_degradation() {
        // AudioSystem should work even without an audio device
        // (CI environments typically have no audio)
        let mut system = AudioSystem::new();
        let mut world = FlintWorld::new();

        // Initialize should not error
        let result = system.initialize(&mut world);
        assert!(result.is_ok());

        // Update should not error
        let result = system.update(&mut world, 0.016);
        assert!(result.is_ok());

        // Shutdown should not error
        let result = system.shutdown();
        assert!(result.is_ok());

        assert_eq!(system.name(), "audio");
    }

    #[test]
    fn test_process_events_no_crash() {
        let system = AudioSystem::new();
        let world = FlintWorld::new();

        let events = vec![
            GameEvent::ActionPressed("jump".into()),
            GameEvent::CollisionStarted {
                entity_a: flint_core::EntityId::new(),
                entity_b: flint_core::EntityId::new(),
            },
        ];

        // Should not crash even with no loaded sounds
        let commands = system.triggers.process_events(&events, &world);
        assert!(commands.is_empty());
    }
}
