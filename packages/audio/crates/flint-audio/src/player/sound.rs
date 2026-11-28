//! [`AudioSystem`] as the host's [`Sound`] (#1619): what the player asked
//! of `PlayerApp::audio` before the package was one.

use crate::{AudioSystem, Buses};
use flint_core::project::Project;
use flint_ecs::FlintWorld;
use flint_package::sound::{Meter, Output, Sound};
use std::any::Any;
use std::collections::BTreeMap;

/// The run's sound, opened as the host asks. Opening the sound device can
/// take a second (#1092): a load report's `AudioDevice` stage times it.
pub(super) fn open(output: Output) -> Box<dyn Sound> {
    Box::new(match output {
        Output::Device => flint_core::load_report::timed(
            flint_core::load_report::Stage::AudioDevice,
            String::new,
            AudioSystem::new,
        ),
        Output::Silent => AudioSystem::silent(),
    })
}

/// The page's Begin gesture: the `AudioContext` opens inside it.
pub(super) fn unlock() {
    #[cfg(target_arch = "wasm32")]
    crate::worklet::unlock();
}

/// The page's sleep (#162): hidden, and how many times it has been.
pub(super) fn page() -> (bool, u64) {
    #[cfg(target_arch = "wasm32")]
    {
        crate::page::listen();
        (crate::page::hidden(), crate::page::sleeps())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        (false, 0)
    }
}

impl Sound for AudioSystem {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn start(&mut self, world: &FlintWorld) -> bool {
        AudioSystem::start(self, world)
    }

    fn declare_buses(&mut self, project: &Project) {
        match Buses::for_project(project) {
            Ok(buses) => self.engine.set_buses(&buses),
            Err(e) => tracing::error!(target: "load", "audio buses: {e}"),
        }
    }

    fn hold(&mut self, held: bool) {
        self.engine.set_held(held);
    }

    fn meter(&self) -> Meter {
        Meter {
            buffers: self
                .engine
                .buffer_stats()
                .map(|b| (b.late, b.buffers, b.buffer_ms, b.mix_mean_ms)),
            mix: self.mix_counts(),
        }
    }

    fn set_old_mix(&mut self, old: bool) {
        AudioSystem::set_old_mix(self, old);
    }

    fn old_mix(&self) -> bool {
        self.engine.old_mix()
    }

    fn user_volumes(&self) -> BTreeMap<String, f64> {
        self.engine.user_bus_volumes().clone()
    }

    fn load_user_volumes(&mut self, volumes: BTreeMap<String, f64>) {
        self.engine.load_user_bus_volumes(volumes);
    }

    fn set_user_volume(&mut self, bus: &str, volume: f64) -> bool {
        let before = self.engine.user_bus_volume(bus);
        self.engine.set_user_bus_volume(bus, volume).is_ok()
            && self.engine.user_bus_volume(bus) != before
    }
}
