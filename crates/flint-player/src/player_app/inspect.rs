//! The running game's world, asked over a loopback socket (#713).
//!
//! `--inspect-listen 127.0.0.1:<port>` takes one question a line, a JSON
//! object or the bare text of a query:
//!
//! ```text
//! entities where door.locked == true
//! {"query": "entities where archetype == 'player'", "data": true}
//! {"entity": "player"}
//! ```
//!
//! and answers each with one line of JSON:
//!
//! ```text
//! {"ok": true, "scene": "levels/demo.scene.toml", "time": 12.5, "frame": 750, "answer": [...]}
//! {"ok": false, "scene": "...", "time": 12.5, "frame": 750, "error": "Entity 'x' not found"}
//! ```
//!
//! `answer` is what `flint query` or `flint entity show` prints for the
//! same question about a scene file (`flint_query::answer`, the one code
//! for both), so `flint query --attach <addr>` and `flint entity show
//! --attach <addr>` print the live world exactly as they print a file.
//!
//! Like `--expression-listen` it binds loopback only, and the network is
//! read and written on threads of its own ([`super::loopback`]). The world
//! is read on the frame thread, at [`Stage::Sync`] (the frame's world is
//! whole: scripts, physics, animation and procgen have run), at most
//! [`MAX_PER_FRAME`] questions a frame; a reader thread waits up to
//! [`ANSWER_TIMEOUT`] for its answer and says so if none comes. Past
//! [`QUEUE_CAPACITY`] waiting questions a new one is refused as busy, never
//! queued without bound.

use super::loopback::{self, read_capped_line, trim, Line, MAX_LINE_BYTES};
use super::subsystem::{Lasts, Stage, StageCtx, Subsystem};
use flint_ecs::FlintWorld;
use flint_query::Ask;
use serde_json::json;
use std::io::{BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TrySendError};
use std::sync::Arc;
use std::time::Duration;

/// The most questions a frame answers. The rest wait for the next one.
pub const MAX_PER_FRAME: usize = 16;
/// The most questions waiting for a frame. Past this a new one is refused.
pub const QUEUE_CAPACITY: usize = 64;
/// How long a reader waits for the frame to answer.
pub const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);

/// A question on its way to the frame, and where its answer goes.
struct Pending {
    ask: Ask,
    reply: Sender<String>,
}

/// The listening socket: its reader threads hand questions to the frame
/// through a bounded queue.
pub struct InspectListener {
    asks: Receiver<Pending>,
    addr: SocketAddr,
    closed: Arc<AtomicBool>,
}

impl Drop for InspectListener {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Relaxed);
    }
}

impl InspectListener {
    /// Bind `addr` (e.g. `127.0.0.1:7878`; port 0 picks one) and start
    /// accepting. Anything but a loopback address is refused.
    pub fn bind(addr: &str) -> anyhow::Result<Self> {
        let listener = loopback::bind("--inspect-listen", addr)?;
        let addr = listener.local_addr()?;
        let (to_frame, asks) = mpsc::sync_channel(QUEUE_CAPACITY);
        let closed = Arc::new(AtomicBool::new(false));
        let readers_closed = closed.clone();
        loopback::accept_on_thread(
            listener,
            "inspect-listen",
            closed.clone(),
            move |stream, peer| serve(stream, &to_frame, peer, &readers_closed),
        )?;
        // The player's one log (#716), under `inspect`, which --quiet keeps
        tracing::info!(
            target: "inspect",
            addr = %addr,
            "[inspect] listening on {addr}: flint query --attach {addr} \"entities\""
        );
        Ok(Self { asks, addr, closed })
    }

    /// Where it's listening (the real port when bound to port 0).
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// Answer the questions waiting, at most `max`, oldest first, with
    /// `answer`. Returns how many it answered. Waits on nothing.
    pub fn answer_waiting(&self, max: usize, mut answer: impl FnMut(&Ask) -> String) -> usize {
        let mut n = 0;
        while n < max {
            let Ok(pending) = self.asks.try_recv() else {
                break;
            };
            // A reader that gave up has gone; its answer goes nowhere.
            let _ = pending.reply.send(answer(&pending.ask));
            n += 1;
        }
        n
    }

    /// What a frame does: answer what waits (at most [`MAX_PER_FRAME`])
    /// from `world` as it stands at game time `time`, frame `frame` of the
    /// run. A host of its own (a test, a tool stepping a world) calls this
    /// once a frame, as the player's subsystem does.
    pub fn answer_from(&self, world: &FlintWorld, scene: &str, time: f64, frame: u64) -> usize {
        self.answer_waiting(MAX_PER_FRAME, |ask| reply(world, ask, scene, time, frame))
    }
}

/// One line of the socket's answer to `ask` about `world`, as the frame
/// sees it at game time `time`, frame `frame` of the run.
pub fn reply(world: &FlintWorld, ask: &Ask, scene: &str, time: f64, frame: u64) -> String {
    match flint_query::answer(world, ask) {
        Ok(answer) => json!({
            "ok": true, "scene": scene, "time": time, "frame": frame, "answer": answer,
        }),
        Err(error) => json!({
            "ok": false, "scene": scene, "time": time, "frame": frame, "error": error,
        }),
    }
    .to_string()
}

/// An answer that never reached the frame.
fn refusal(why: impl Into<String>) -> String {
    json!({ "ok": false, "error": why.into() }).to_string()
}

/// Read questions from one connection until it hangs up or the player goes
/// away, answering each with one line.
fn serve(stream: TcpStream, to_frame: &SyncSender<Pending>, peer: &str, closed: &AtomicBool) {
    let mut writer = match stream.try_clone() {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!(target: "inspect", "{peer}: {e}");
            return;
        }
    };
    let mut reader = BufReader::new(stream);
    let mut line = Vec::new();
    while !closed.load(Ordering::Relaxed) {
        line.clear();
        let answer = match read_capped_line(&mut reader, &mut line) {
            Ok(Line::Eof) => return,
            Ok(Line::TooLong) => refusal(format!("a line longer than {MAX_LINE_BYTES} bytes")),
            Ok(Line::Ready) => {
                let text = trim(&line);
                if text.is_empty() {
                    continue;
                }
                match std::str::from_utf8(text)
                    .map_err(|_| "not UTF-8 text".to_string())
                    .and_then(Ask::parse)
                {
                    Ok(ask) => ask_the_frame(to_frame, ask),
                    Err(why) => refusal(why),
                }
            }
            Err(e) => {
                tracing::warn!(target: "inspect", "{peer}: {e}");
                return;
            }
        };
        if writer
            .write_all(answer.as_bytes())
            .and_then(|()| writer.write_all(b"\n"))
            .and_then(|()| writer.flush())
            .is_err()
        {
            return;
        }
    }
}

/// Hand `ask` to the frame and wait for its answer.
fn ask_the_frame(to_frame: &SyncSender<Pending>, ask: Ask) -> String {
    let (reply, answer) = mpsc::channel();
    match to_frame.try_send(Pending { ask, reply }) {
        Ok(()) => answer.recv_timeout(ANSWER_TIMEOUT).unwrap_or_else(|_| {
            refusal(format!(
                "no frame answered within {} s: the game may be stopped in a debugger or not drawing",
                ANSWER_TIMEOUT.as_secs()
            ))
        }),
        Err(TrySendError::Full(_)) => refusal(format!(
            "busy: {QUEUE_CAPACITY} questions already wait for a frame"
        )),
        Err(TrySendError::Disconnected(_)) => refusal("the game has stopped"),
    }
}

/// `--inspect-listen` as one of the run's subsystems: it answers at
/// [`Stage::Sync`] of every frame, in every scene.
pub(crate) struct InspectSubsystem {
    listener: InspectListener,
    /// Frames of the run so far, this one counted.
    frame: u64,
}

impl InspectSubsystem {
    pub(crate) fn new(listener: InspectListener) -> Self {
        Self { listener, frame: 0 }
    }
}

impl Subsystem for InspectSubsystem {
    fn name(&self) -> &'static str {
        "inspect"
    }

    fn lasts(&self) -> Lasts {
        Lasts::Run
    }

    fn update(&mut self, stage: Stage, ctx: &mut StageCtx) {
        if stage != Stage::Sync {
            return;
        }
        self.frame += 1;
        self.listener
            .answer_from(ctx.world, ctx.scene_path, ctx.clock.total_time, self.frame);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;
    use std::time::Instant;

    fn world() -> FlintWorld {
        let mut world = FlintWorld::new();
        let id = world.spawn("front_door").unwrap();
        let mut c = world.get_components_mut(id).unwrap();
        c.set_archetype(Some("door".into()));
        c.set("door", toml::toml! { locked = true }.into());
        world
    }

    /// Send `lines`, answering on this thread as frames would, and read
    /// back one answer a line.
    fn ask(listener: &InspectListener, lines: &[&str]) -> Vec<serde_json::Value> {
        let world = world();
        let mut s = TcpStream::connect(listener.local_addr()).unwrap();
        for line in lines {
            writeln!(s, "{line}").unwrap();
        }
        s.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
        let mut reader = BufReader::new(s);
        let mut got = Vec::new();
        let mut line = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut frame = 0;
        while got.len() < lines.len() && Instant::now() < deadline {
            frame += 1;
            listener.answer_waiting(MAX_PER_FRAME, |ask| {
                reply(&world, ask, "demo.scene.toml", 0.5, frame)
            });
            // A timed-out read keeps what it read in `line` for the next.
            let _ = reader.read_until(b'\n', &mut line);
            if line.last() == Some(&b'\n') {
                got.push(serde_json::from_slice(&line).unwrap());
                line.clear();
            }
        }
        got
    }

    #[test]
    fn refuses_anything_but_loopback() {
        let err = InspectListener::bind("0.0.0.0:0")
            .err()
            .unwrap()
            .to_string();
        assert!(
            err.contains("loopback") && err.contains("--inspect-listen"),
            "{err}"
        );
    }

    #[test]
    fn a_query_and_an_entity_are_answered_from_the_frames_world() {
        let listener = InspectListener::bind("127.0.0.1:0").unwrap();
        let got = ask(
            &listener,
            &[
                "entities where door.locked == true",
                r#"{"entity": "front_door"}"#,
                r#"{"entity": "back_door"}"#,
                "entities where",
            ],
        );
        assert_eq!(got.len(), 4, "{got:?}");
        assert_eq!(got[0]["ok"], true);
        assert_eq!(got[0]["time"], 0.5);
        assert_eq!(got[0]["scene"], "demo.scene.toml");
        assert_eq!(got[0]["answer"][0]["name"], "front_door");
        assert_eq!(got[1]["answer"]["data"]["door"]["locked"], true);
        assert_eq!(got[2]["ok"], false);
        assert_eq!(got[2]["error"], "Entity 'back_door' not found");
        assert!(
            got[3]["error"].as_str().unwrap().contains("parse"),
            "{got:?}"
        );
    }

    #[test]
    fn a_line_that_isnt_a_question_is_refused_without_the_frame() {
        let listener = InspectListener::bind("127.0.0.1:0").unwrap();
        let mut s = TcpStream::connect(listener.local_addr()).unwrap();
        writeln!(s, r#"{{"what": 1}}"#).unwrap();
        let mut line = String::new();
        BufReader::new(s).read_line(&mut line).unwrap();
        let got: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(got["ok"], false);
        assert!(got.get("time").is_none(), "no frame saw it: {got}");
    }
}
