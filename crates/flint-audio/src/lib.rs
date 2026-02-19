//! Flint Audio — Spatial audio system (Kira backend)
//!
//! Provides audio playback for the Flint engine:
//! - `AudioEngine` — wraps Kira AudioManager, sound loading, listener management
//! - `AudioSync` — bridges TOML `audio_source` components to Kira spatial tracks
//! - `AudioTrigger` — maps GameEvents to sound playback (collision, interaction)
//! - `AudioSystem` — implements `RuntimeSystem` for game loop integration

pub mod engine;
pub mod sync;
pub mod trigger;

use engine::AudioEngine;
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
                        if let Err(e) = self.engine.play_at_position(&sound, pos, volume) {
                            eprintln!("Audio: {:?}", e);
                        }
                    } else if let Err(e) =
                        self.engine.play_non_spatial(&sound, volume, 1.0, false)
                    {
                        // One-shot non-spatial: handle not needed
                        eprintln!("Audio: {:?}", e);
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
            eprintln!("Audio: no device, running silent");
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
