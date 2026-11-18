//! The player's loopback sockets: what `--expression-listen` (#518) and
//! `--inspect-listen` (#713) share. Each binds 127.0.0.1 or ::1 only, takes
//! newline-delimited lines no longer than [`MAX_LINE_BYTES`], keeps at most
//! [`MAX_CONNECTIONS`] open, and does all of its reading and writing on
//! threads of its own, never on the frame's.

use std::io::{BufRead, Read};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

/// The longest line a socket takes, in bytes, newline excluded.
pub const MAX_LINE_BYTES: usize = 64 * 1024;
/// The most connections a socket keeps open at once.
pub const MAX_CONNECTIONS: usize = 8;

/// Bind `addr` for the flag `flag` (e.g. `127.0.0.1:7777`; port 0 picks
/// one). Anything but a loopback address is refused: these sockets talk to
/// the same machine, never to a network.
pub fn bind(flag: &str, addr: &str) -> anyhow::Result<TcpListener> {
    let wanted = check(flag, addr)?;
    TcpListener::bind(wanted).map_err(|e| anyhow::anyhow!("{flag} {addr}: {e}"))
}

/// `addr` as [`bind`] would take it, without binding: an IP:port whose IP is
/// loopback. Whether the port is free is known only by binding it.
pub fn check(flag: &str, addr: &str) -> anyhow::Result<SocketAddr> {
    let wanted: SocketAddr = addr.parse().map_err(|e| {
        anyhow::anyhow!("{flag} {addr}: expected IP:port, like 127.0.0.1:7777 ({e})")
    })?;
    if !wanted.ip().is_loopback() {
        anyhow::bail!("{flag} {addr}: only a loopback address (127.0.0.1 or ::1) can listen");
    }
    Ok(wanted)
}

/// Accept on a thread named `name` until `closed` is set, handing each
/// connection, with its peer's address, to `serve` on a thread of its own.
/// Past [`MAX_CONNECTIONS`] open at once a new one is refused. Returns how
/// many are open now, which the reader threads keep (the F3 Expression
/// panel shows it, #525).
pub fn accept_on_thread(
    listener: TcpListener,
    name: &'static str,
    closed: Arc<AtomicBool>,
    serve: impl Fn(TcpStream, &str) + Send + Sync + 'static,
) -> std::io::Result<Arc<AtomicUsize>> {
    let serve = Arc::new(serve);
    let open = Arc::new(AtomicUsize::new(0));
    let counted = open.clone();
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            for stream in listener.incoming() {
                if closed.load(Ordering::Relaxed) {
                    return;
                }
                let stream = match stream {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!(target: "loopback", "{name}: accept failed: {e}");
                        continue;
                    }
                };
                let peer = stream
                    .peer_addr()
                    .map(|a| a.to_string())
                    .unwrap_or_else(|_| "?".into());
                if open.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
                    tracing::warn!(
                        target: "loopback",
                        "{name}: refused {peer}: already {MAX_CONNECTIONS} connections"
                    );
                    continue;
                }
                open.fetch_add(1, Ordering::SeqCst);
                let still_open = open.clone();
                let serve = serve.clone();
                let spawned = std::thread::Builder::new()
                    .name(format!("{name}-reader"))
                    .spawn(move || {
                        tracing::info!(target: "loopback", "{name}: {peer} connected");
                        serve(stream, &peer);
                        tracing::info!(target: "loopback", "{name}: {peer} disconnected");
                        still_open.fetch_sub(1, Ordering::SeqCst);
                    });
                if let Err(e) = spawned {
                    open.fetch_sub(1, Ordering::SeqCst);
                    tracing::warn!(target: "loopback", "{name}: no reader thread: {e}");
                }
            }
        })
        .map(|_| counted)
}

pub enum Line {
    Ready,
    TooLong,
    Eof,
}

/// Read one line into `buf` (newline excluded), holding at most
/// [`MAX_LINE_BYTES`] of it. A longer line is read through to its end and
/// thrown away. A last line without a newline still counts.
pub fn read_capped_line<R: BufRead>(reader: &mut R, buf: &mut Vec<u8>) -> std::io::Result<Line> {
    let n = reader
        .by_ref()
        .take(MAX_LINE_BYTES as u64 + 1)
        .read_until(b'\n', buf)?;
    if n == 0 {
        return Ok(Line::Eof);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
        return Ok(Line::Ready);
    }
    if buf.len() <= MAX_LINE_BYTES {
        return Ok(Line::Ready); // the stream ended mid-line
    }
    // Too long: skip to the end of it without holding any more.
    loop {
        let (done, used) = {
            let chunk = reader.fill_buf()?;
            if chunk.is_empty() {
                return Ok(Line::TooLong);
            }
            match chunk.iter().position(|&b| b == b'\n') {
                Some(i) => (true, i + 1),
                None => (false, chunk.len()),
            }
        };
        reader.consume(used);
        if done {
            return Ok(Line::TooLong);
        }
    }
}

/// `bytes` without the ASCII whitespace at either end (a `\r` included).
pub fn trim(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map_or(start, |i| i + 1);
    &bytes[start..end]
}
