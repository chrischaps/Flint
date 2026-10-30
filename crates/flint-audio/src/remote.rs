//! The engine's side of a mixer that runs elsewhere (#136): in the browser,
//! kira inside an `AudioWorklet` (`flint-audio-worklet`).
//!
//! The engine writes each call as a [`Command`] into a [`Link`]'s outbox and
//! queues each sound once, as samples. Whoever owns the other end takes the
//! outbox when it likes: on the web, a microtask after the first write of a
//! frame posts it all as one message (`crate::worklet`), so a frame's calls
//! cross together. Ids are made here, so a call returns its handle at once.
//! Nothing here is platform code; the tests run it against the worklet's
//! mixer natively.

use std::sync::{Arc, Mutex};

use kira::Frame;

#[path = "../../flint-audio-worklet/src/protocol.rs"]
pub mod protocol;

pub use protocol::{Command, Target};

/// A sound on its way to the mixer: one channel when both sides are equal
/// (every Float sound is mono), else two interleaved.
#[derive(Debug, Clone, PartialEq)]
pub struct SoundLoad {
    pub id: u32,
    pub sample_rate: u32,
    pub channels: u32,
    pub samples: Vec<f32>,
}

impl SoundLoad {
    pub fn new(id: u32, sample_rate: u32, frames: &[Frame]) -> Self {
        let mono = frames.iter().all(|f| f.left == f.right);
        let samples = if mono {
            frames.iter().map(|f| f.left).collect()
        } else {
            frames.iter().flat_map(|f| [f.left, f.right]).collect()
        };
        Self {
            id,
            sample_rate,
            channels: if mono { 1 } else { 2 },
            samples,
        }
    }
}

/// What has been written since the last [`Link::take`]: sounds first, then
/// the commands, which may name them.
#[derive(Debug, Default)]
pub struct Outbox {
    pub loads: Vec<SoundLoad>,
    pub batch: Vec<u8>,
    pub commands: usize,
}

impl Outbox {
    pub fn is_empty(&self) -> bool {
        self.loads.is_empty() && self.batch.is_empty()
    }
}

/// Called when an empty outbox gets its first write.
pub type Kick = Box<dyn FnMut() + Send>;

struct Shared {
    outbox: Outbox,
    kick: Option<Kick>,
    next_id: u32,
}

/// The engine's line to a remote mixer. Clones share one outbox.
#[derive(Clone)]
pub struct Link(Arc<Mutex<Shared>>);

impl Default for Link {
    fn default() -> Self {
        Self::new(None)
    }
}

impl Link {
    /// A link whose outbox is taken by hand, or, with a `kick`, by whoever
    /// the kick wakes.
    pub fn new(kick: Option<Kick>) -> Self {
        Self(Arc::new(Mutex::new(Shared {
            outbox: Outbox::default(),
            kick,
            next_id: 1,
        })))
    }

    fn write(&self, f: impl FnOnce(&mut Outbox)) {
        let Ok(mut s) = self.0.lock() else { return };
        let was_empty = s.outbox.is_empty();
        f(&mut s.outbox);
        if was_empty {
            if let Some(kick) = &mut s.kick {
                kick();
            }
        }
    }

    pub fn send(&self, command: Command) {
        self.write(|o| {
            command.encode(&mut o.batch);
            o.commands += 1;
        });
    }

    pub fn load(&self, load: SoundLoad) {
        self.write(|o| o.loads.push(load));
    }

    /// Everything written since the last take.
    pub fn take(&self) -> Outbox {
        self.0
            .lock()
            .map(|mut s| std::mem::take(&mut s.outbox))
            .unwrap_or_default()
    }

    /// A fresh id for a sound, voice or track.
    pub fn id(&self) -> u32 {
        let Ok(mut s) = self.0.lock() else { return 0 };
        let id = s.next_id;
        s.next_id = s.next_id.wrapping_add(1).max(1);
        id
    }
}

/// A sound playing in the remote mixer. Dropping it lets the sound play on,
/// as dropping kira's handle does.
pub struct RemoteVoice {
    pub(crate) id: u32,
    pub(crate) link: Link,
}

impl Drop for RemoteVoice {
    fn drop(&mut self) {
        self.link.send(Command::VoiceDrop { voice: self.id });
    }
}

/// A spatial track in the remote mixer. Dropping it removes the track and
/// stops what plays on it, as dropping kira's handle does.
pub struct RemoteTrack {
    pub(crate) id: u32,
    pub(crate) link: Link,
}

impl Drop for RemoteTrack {
    fn drop(&mut self) {
        self.link.send(Command::TrackDrop { track: self.id });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_sides_go_as_one_channel_and_unequal_as_two() {
        let mono = [Frame::from_mono(0.5), Frame::from_mono(-0.25)];
        let load = SoundLoad::new(1, 44_100, &mono);
        assert_eq!((load.channels, load.samples), (1, vec![0.5, -0.25]));
        let stereo = [
            Frame::from_mono(0.5),
            Frame {
                left: 0.1,
                right: 0.2,
            },
        ];
        let load = SoundLoad::new(2, 48_000, &stereo);
        assert_eq!((load.channels, load.samples), (2, vec![0.5, 0.5, 0.1, 0.2]));
    }

    #[test]
    fn the_kick_comes_once_per_take() {
        let kicks = Arc::new(Mutex::new(0));
        let k = kicks.clone();
        let link = Link::new(Some(Box::new(move || *k.lock().unwrap() += 1)));
        link.send(Command::ClearOneShots);
        link.send(Command::ClearOneShots);
        link.load(SoundLoad::new(1, 1, &[]));
        assert_eq!(*kicks.lock().unwrap(), 1);
        let out = link.take();
        assert_eq!((out.commands, out.loads.len()), (2, 1));
        link.send(Command::ClearOneShots);
        assert_eq!(*kicks.lock().unwrap(), 2);
        assert_ne!(link.id(), link.id());
    }
}
