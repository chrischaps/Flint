//! Decoding a sound a little at a time (#124).
//!
//! kira's `StaticSoundData::from_cursor` decodes a whole file in one call:
//! 100 to 241 ms for one of Float's songs in the browser, all inside one
//! frame. Its streaming sounds would spread the work, but they decode on a
//! thread of their own (`std::thread::spawn` in kira's `decode_scheduler`),
//! and wasm32 here has none. So [`Decoding`] does what `from_cursor` does,
//! one packet at a time, and [`DecodeQueue`] runs as many packets as fit in
//! a frame's budget, nearest-needed sound first.

use kira::sound::static_sound::StaticSoundData;
use kira::Frame;
use std::io::Cursor;
use std::mem::MaybeUninit;
use std::sync::Arc;
use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::codecs::Decoder;
use symphonia::core::conv::IntoSample;
use symphonia::core::formats::FormatReader;
use symphonia::core::io::MediaSourceStream;
use web_time::{Duration, Instant};

/// One sound partway through decoding.
pub struct Decoding {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track: u32,
    sample_rate: u32,
    frames: Frames,
}

/// Decoded frames, written straight into the `Arc<[Frame]>` kira plays
/// from, sized by the stream's frame count. Collecting a `Vec` and turning
/// it into an `Arc` at the end copies the whole song in one frame: 26 MB,
/// a 67 ms frame in Chrome. A stream that runs past its stated count spills
/// into a `Vec` and pays that copy.
struct Frames {
    buf: Arc<[MaybeUninit<Frame>]>,
    filled: usize,
    spill: Vec<Frame>,
}

impl Frames {
    fn new(capacity: usize) -> Self {
        Self {
            buf: Arc::new_uninit_slice(capacity),
            filled: 0,
            spill: Vec::new(),
        }
    }

    fn extend(&mut self, frames: impl Iterator<Item = Frame>) {
        let buf = Arc::get_mut(&mut self.buf).expect("only Frames holds the buffer");
        for frame in frames {
            match buf.get_mut(self.filled) {
                Some(slot) => {
                    slot.write(frame);
                    self.filled += 1;
                }
                None => self.spill.push(frame),
            }
        }
    }

    fn finish(self) -> Arc<[Frame]> {
        if self.filled == self.buf.len() && self.spill.is_empty() {
            // SAFETY: every slot up to `filled`, which is all of them, was
            // written by `extend`.
            return unsafe { self.buf.assume_init() };
        }
        self.buf[..self.filled]
            .iter()
            // SAFETY: the slots before `filled` were written by `extend`.
            .map(|f| unsafe { f.assume_init() })
            .chain(self.spill)
            .collect()
    }
}

/// What a [`Decoding::step`] got to. Returned and matched at once, never
/// stored, so the big variant costs nothing.
#[allow(clippy::large_enum_variant)]
pub enum Step {
    /// More packets to go.
    More,
    /// The last packet is in: the sound, ready to play.
    Done(StaticSoundData),
}

fn error(e: impl std::fmt::Display) -> String {
    e.to_string()
}

impl Decoding {
    /// Open an encoded sound (Ogg Vorbis, FLAC, WAV or MP3) held in memory.
    /// Reading its headers is quick; the packets wait for [`step`](Self::step).
    pub fn open(bytes: Vec<u8>) -> Result<Self, String> {
        let mss = MediaSourceStream::new(Box::new(Cursor::new(bytes)), Default::default());
        let format = symphonia::default::get_probe()
            .format(
                &Default::default(),
                mss,
                &Default::default(),
                &Default::default(),
            )
            .map_err(error)?
            .format;
        let track = format.default_track().ok_or("no default track")?;
        let params = &track.codec_params;
        let sample_rate = params.sample_rate.ok_or("unknown sample rate")?;
        let frames = Frames::new(params.n_frames.unwrap_or(0) as usize);
        let decoder = symphonia::default::get_codecs()
            .make(params, &Default::default())
            .map_err(error)?;
        Ok(Self {
            track: track.id,
            format,
            decoder,
            sample_rate,
            frames,
        })
    }

    /// Decode one packet.
    pub fn step(&mut self) -> Result<Step, String> {
        let packet = match self.format.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                let frames = std::mem::replace(&mut self.frames, Frames::new(0));
                return Ok(Step::Done(StaticSoundData {
                    sample_rate: self.sample_rate,
                    frames: frames.finish(),
                    settings: Default::default(),
                    slice: None,
                }));
            }
            Err(e) => return Err(error(e)),
        };
        if packet.track_id() == self.track {
            let buffer = self.decoder.decode(&packet).map_err(error)?;
            push_frames(&buffer, &mut self.frames)?;
        }
        Ok(Step::More)
    }

    /// Decode every packet left, now.
    pub fn finish(mut self) -> Result<StaticSoundData, String> {
        loop {
            if let Step::Done(data) = self.step()? {
                return Ok(data);
            }
        }
    }
}

/// Append a decoded buffer's frames, as kira's own loader does: one channel
/// plays on both sides, two are left and right.
fn push_frames(buffer: &AudioBufferRef, frames: &mut Frames) -> Result<(), String> {
    fn push<S>(
        buffer: &symphonia::core::audio::AudioBuffer<S>,
        frames: &mut Frames,
    ) -> Result<(), String>
    where
        S: symphonia::core::sample::Sample + IntoSample<f32>,
    {
        match buffer.spec().channels.count() {
            1 => frames.extend(
                buffer
                    .chan(0)
                    .iter()
                    .map(|&s| Frame::from_mono(s.into_sample())),
            ),
            2 => frames.extend(
                buffer
                    .chan(0)
                    .iter()
                    .zip(buffer.chan(1))
                    .map(|(&l, &r)| Frame::new(l.into_sample(), r.into_sample())),
            ),
            n => return Err(format!("{n} channels; a sound has one or two")),
        }
        Ok(())
    }
    match buffer {
        AudioBufferRef::U8(b) => push(b, frames),
        AudioBufferRef::U16(b) => push(b, frames),
        AudioBufferRef::U24(b) => push(b, frames),
        AudioBufferRef::U32(b) => push(b, frames),
        AudioBufferRef::S8(b) => push(b, frames),
        AudioBufferRef::S16(b) => push(b, frames),
        AudioBufferRef::S24(b) => push(b, frames),
        AudioBufferRef::S32(b) => push(b, frames),
        AudioBufferRef::F32(b) => push(b, frames),
        AudioBufferRef::F64(b) => push(b, frames),
    }
}

/// How soon a queued sound is wanted. Lower decodes first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Need {
    /// A script asked to play it and it was skipped.
    Asked,
    /// A source that plays as soon as it can be heard (autoplay, above
    /// silence): the sea.
    Audible,
    /// Everything else, smallest file first: one-shots before songs.
    Later,
}

/// A sound out of the queue: decoded, or why not.
pub struct Decoded {
    pub name: String,
    pub result: Result<StaticSoundData, String>,
    /// Encoded size.
    pub size: usize,
    /// Decoding time, and how many calls (frames) it was spread over.
    pub spent: Duration,
    pub runs: u32,
    /// The longest single step (opening, one packet, or the last).
    pub longest: Duration,
}

struct Queued {
    name: String,
    spent: Duration,
    runs: u32,
    longest: Duration,
    need: Need,
    /// Encoded size, the tiebreak within a `Need`.
    size: usize,
    /// The bytes, until the first step opens them.
    bytes: Option<Vec<u8>>,
    decoding: Option<Decoding>,
}

/// Sounds waiting to be decoded, worked through a budget at a time.
#[derive(Default)]
pub struct DecodeQueue {
    queue: Vec<Queued>,
    /// Sounds decoded and frame time spent, since the queue was made.
    pub decoded: usize,
    pub spent: Duration,
}

impl DecodeQueue {
    /// Queue `bytes` under `name`. Already queued: it keeps its place,
    /// moved up if `need` is sooner.
    pub fn push(&mut self, name: &str, bytes: Vec<u8>, need: Need) {
        if let Some(q) = self.queue.iter_mut().find(|q| q.name == name) {
            q.need = q.need.min(need);
            return;
        }
        self.queue.push(Queued {
            name: name.to_string(),
            need,
            size: bytes.len(),
            spent: Duration::ZERO,
            runs: 0,
            longest: Duration::ZERO,
            bytes: Some(bytes),
            decoding: None,
        });
    }

    pub fn contains(&self, name: &str) -> bool {
        self.queue.iter().any(|q| q.name == name)
    }

    /// The encoded size of a queued sound.
    pub fn size_of(&self, name: &str) -> Option<usize> {
        self.queue.iter().find(|q| q.name == name).map(|q| q.size)
    }

    /// Move a queued sound up to `need`.
    pub fn promote(&mut self, name: &str, need: Need) {
        if let Some(q) = self.queue.iter_mut().find(|q| q.name == name) {
            q.need = q.need.min(need);
        }
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Take a queued sound out and decode all of it now.
    pub fn decode_now(&mut self, name: &str) -> Option<Decoded> {
        let i = self.queue.iter().position(|q| q.name == name)?;
        let start = Instant::now();
        let q = self.queue.remove(i);
        let result = match (q.decoding, q.bytes) {
            (Some(d), _) => d.finish(),
            (None, Some(bytes)) => Decoding::open(bytes).and_then(Decoding::finish),
            (None, None) => Err("nothing to decode".into()),
        };
        let took = start.elapsed();
        self.decoded += 1;
        self.spent += took;
        Some(Decoded {
            name: q.name,
            result,
            size: q.size,
            spent: q.spent + took,
            runs: q.runs + 1,
            longest: q.longest.max(took),
        })
    }

    /// Decode packets, soonest-needed sound first, until `budget` is spent
    /// or the queue is empty, and return the sounds that finished (or
    /// failed). One packet is the unit, a few hundred microseconds at most,
    /// so a frame overshoots its budget by about that.
    pub fn run(&mut self, budget: Duration) -> Vec<Decoded> {
        let start = Instant::now();
        let mut done = Vec::new();
        let mut touched: Option<usize> = None;
        while start.elapsed() < budget {
            let Some(i) = (0..self.queue.len()).min_by_key(|&i| {
                let q = &self.queue[i];
                // A sound already open finishes before a peer starts.
                (q.need, q.decoding.is_none(), q.size)
            }) else {
                break;
            };
            let packet = Instant::now();
            let q = &mut self.queue[i];
            if touched != Some(i) {
                q.runs += 1;
                touched = Some(i);
            }
            let step = match q.decoding.as_mut() {
                Some(d) => d.step(),
                None => match Decoding::open(q.bytes.take().unwrap_or_default()) {
                    Ok(d) => {
                        q.decoding = Some(d);
                        Ok(Step::More)
                    }
                    Err(e) => Err(e),
                },
            };
            let took = packet.elapsed();
            q.spent += took;
            q.longest = q.longest.max(took);
            let result = match step {
                Ok(Step::More) => continue,
                Ok(Step::Done(data)) => {
                    self.decoded += 1;
                    Ok(data)
                }
                Err(e) => Err(e),
            };
            let q = self.queue.remove(i);
            touched = None;
            done.push(Decoded {
                name: q.name,
                result,
                size: q.size,
                spent: q.spent,
                runs: q.runs,
                longest: q.longest,
            });
        }
        self.spent += start.elapsed();
        done
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A 16-bit mono WAV of `n` frames of a tone.
    pub(crate) fn wav(n: usize) -> Vec<u8> {
        let tone: Vec<i16> = (0..n)
            .map(|i| ((i as f32 * 0.1).sin() * 16000.0) as i16)
            .collect();
        let data_len = tone.len() as u32 * 2;
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        for v in [
            16u32.to_le_bytes(),
            [1, 0, 1, 0],
            44_100u32.to_le_bytes(),
            88_200u32.to_le_bytes(),
            [2, 0, 16, 0],
        ] {
            wav.extend_from_slice(&v);
        }
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        for s in &tone {
            wav.extend_from_slice(&s.to_le_bytes());
        }
        wav
    }

    #[test]
    fn decodes_the_same_frames_as_kira_a_packet_at_a_time() {
        let bytes = wav(50_000);
        let whole = StaticSoundData::from_cursor(Cursor::new(bytes.clone())).unwrap();
        let mut d = Decoding::open(bytes).unwrap();
        let mut steps = 0;
        let ours = loop {
            steps += 1;
            if let Step::Done(data) = d.step().unwrap() {
                break data;
            }
        };
        assert!(steps > 2, "more than one packet: {steps}");
        assert_eq!(ours.sample_rate, whole.sample_rate);
        assert_eq!(ours.frames.len(), whole.frames.len());
        assert!(ours
            .frames
            .iter()
            .zip(whole.frames.iter())
            .all(|(a, b)| a == b));
    }

    #[test]
    fn frames_fill_in_place_and_spill_or_shrink_when_the_count_is_wrong() {
        let tone = |n: usize| (0..n).map(|i| Frame::from_mono(i as f32));
        let mut exact = Frames::new(5);
        exact.extend(tone(3));
        exact.extend(tone(2));
        assert_eq!(exact.finish().len(), 5);
        let mut over = Frames::new(2);
        over.extend(tone(4));
        let over = over.finish();
        assert_eq!((over.len(), over[3].left), (4, 3.0));
        let mut short = Frames::new(9);
        short.extend(tone(4));
        assert_eq!(short.finish().len(), 4);
        let mut unknown = Frames::new(0);
        unknown.extend(tone(3));
        assert_eq!(unknown.finish().len(), 3);
    }

    #[test]
    fn the_queue_decodes_the_nearest_needed_first_within_its_budget() {
        let mut q = DecodeQueue::default();
        q.push("song", wav(400_000), Need::Later);
        q.push("chirp", wav(2_000), Need::Later);
        q.push("sea", wav(100_000), Need::Audible);
        q.push("sea", wav(10), Need::Later); // already queued: keeps its place
        assert_eq!(q.len(), 3);

        // No budget: one packet at most, nothing done.
        assert!(q.run(Duration::ZERO).is_empty());

        let mut order = Vec::new();
        while !q.is_empty() {
            for d in q.run(Duration::from_micros(200)) {
                d.result.unwrap();
                order.push(d.name);
            }
        }
        assert_eq!(order, ["sea", "chirp", "song"]);
        assert_eq!(q.decoded, 3);
    }

    #[test]
    fn an_asked_for_sound_jumps_the_queue_and_junk_fails_alone() {
        let mut q = DecodeQueue::default();
        q.push("junk", vec![1, 2, 3], Need::Audible);
        q.push("song", wav(100_000), Need::Later);
        q.push("click", wav(500), Need::Later);
        q.promote("song", Need::Asked);
        let d = q.decode_now("click").unwrap();
        assert_eq!(
            (d.name.as_str(), d.result.unwrap().frames.len()),
            ("click", 500)
        );
        let mut order = Vec::new();
        while !q.is_empty() {
            for d in q.run(Duration::from_millis(50)) {
                order.push((d.name, d.result.is_ok()));
            }
        }
        assert_eq!(order, [("song".into(), true), ("junk".into(), false)]);
    }
}
