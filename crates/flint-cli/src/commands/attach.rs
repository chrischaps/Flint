//! Asking a running game (#713): `--attach IP:PORT` on `flint query` and
//! `flint entity show` sends the question to a player started with
//! `--inspect-listen IP:PORT` and prints its answer as the command prints
//! one about a file. The protocol, one JSON line each way, is
//! `flint_player`'s `player_app/inspect.rs`.

use anyhow::{bail, Context, Result};
use flint_query::Ask;
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

/// How long to wait for the game to take the connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// How long to wait for its answer: past the player's own 10 s wait for a
/// frame, so its message about a stalled frame arrives first.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(15);

/// What a running game said, and when in its run.
pub struct LiveAnswer {
    pub scene: String,
    pub time: f64,
    pub frame: u64,
    /// What `flint query` or `flint entity show` prints for the question.
    pub answer: Value,
}

/// `IP:PORT`, or a bare port on 127.0.0.1.
fn address(addr: &str) -> Result<SocketAddr> {
    if let Ok(port) = addr.parse::<u16>() {
        return Ok(SocketAddr::from(([127, 0, 0, 1], port)));
    }
    addr.parse()
        .with_context(|| format!("--attach {addr}: expected IP:PORT, like 127.0.0.1:7878"))
}

/// Ask the game listening at `addr` and wait for its answer. A refusal (a
/// query that doesn't parse, an entity that isn't there) is an error with
/// the game's reason.
pub fn ask(addr: &str, question: &Ask) -> Result<LiveAnswer> {
    let at = address(addr)?;
    let stream = TcpStream::connect_timeout(&at, CONNECT_TIMEOUT).map_err(|e| {
        anyhow::anyhow!(
            "--attach {at}: no game is listening there ({e}); start one with \
             `flint play <scene> --inspect-listen {at}`"
        )
    })?;
    stream.set_read_timeout(Some(ANSWER_TIMEOUT))?;
    let mut writer = stream.try_clone()?;
    let line = serde_json::to_string(question)?;
    writer
        .write_all(format!("{line}\n").as_bytes())
        .with_context(|| format!("--attach {at}: sending the question"))?;
    let mut reply = String::new();
    BufReader::new(stream)
        .read_line(&mut reply)
        .with_context(|| format!("--attach {at}: no answer"))?;
    if reply.trim().is_empty() {
        bail!("--attach {at}: the game closed the connection without answering");
    }
    let reply: Value = serde_json::from_str(&reply)
        .with_context(|| format!("--attach {at}: the answer isn't JSON"))?;
    if reply["ok"] != Value::Bool(true) {
        let why = reply["error"].as_str().unwrap_or("no reason given");
        bail!("{why}");
    }
    let answer = LiveAnswer {
        scene: reply["scene"].as_str().unwrap_or("").to_owned(),
        time: reply["time"].as_f64().unwrap_or(0.0),
        frame: reply["frame"].as_u64().unwrap_or(0),
        answer: reply["answer"].clone(),
    };
    // stderr: stdout is the answer alone, the same as a file's
    eprintln!(
        "live: {} at t={:.3}s (frame {})",
        answer.scene, answer.time, answer.frame
    );
    Ok(answer)
}

/// `value` with every null left out, which TOML has no way to write.
pub fn without_nulls(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k, without_nulls(v)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(without_nulls).collect()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_port_is_on_loopback() {
        assert_eq!(address("7878").unwrap().to_string(), "127.0.0.1:7878");
        assert_eq!(address("[::1]:9").unwrap().to_string(), "[::1]:9");
        assert!(address("localhost:7878").is_err());
    }

    #[test]
    fn nobody_listening_says_how_to_start_one() {
        // Bind and drop: the port is free and nothing listens on it.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let err = ask(&port.to_string(), &Ask::parse("entities").unwrap())
            .err()
            .unwrap()
            .to_string();
        assert!(err.contains("--inspect-listen"), "{err}");
    }
}
