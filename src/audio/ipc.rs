use anyhow::{bail, ensure, Context, Result};
use serde_json::Value;
use std::{
    collections::VecDeque,
    io::{ErrorKind, Read, Write},
    os::{fd::AsRawFd, unix::net::UnixStream},
    time::{Duration, Instant},
};

pub(super) const VERSION: u32 = 1;
pub(super) const MAX_MESSAGE: usize = 1024 * 1024;
const MAX_PENDING: usize = 128;

/// Never use read_exact on a nonblocking framed stream: partial headers/payloads
/// must survive WouldBlock, including when a helper stalls mid-message.
pub(super) struct Connection {
    stream: UnixStream,
    header: [u8; 4],
    header_read: usize,
    body: Vec<u8>,
    body_read: usize,
    outgoing: VecDeque<(Vec<u8>, usize, Instant)>,
    queued_bytes: usize,
    frame_started: Option<Instant>,
}

impl Connection {
    pub fn new(stream: UnixStream, child_pid: u32) -> Result<Self> {
        let mut peer = libc::ucred {
            pid: 0,
            uid: 0,
            gid: 0,
        };
        let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: peer/length point to correctly sized writable storage; fd is live.
        let result = unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&mut peer as *mut libc::ucred).cast(),
                &mut length,
            )
        };
        ensure!(
            result == 0,
            "Cannot authenticate audio host: {}",
            std::io::Error::last_os_error()
        );
        // SAFETY: geteuid takes no arguments and does not access Rust memory.
        ensure!(
            peer.uid == unsafe { libc::geteuid() } && peer.pid > 0 && peer.pid as u32 == child_pid,
            "Unexpected audio-host peer"
        );
        stream.set_nonblocking(true)?;
        Ok(Self {
            stream,
            header: [0; 4],
            header_read: 0,
            body: Vec::new(),
            body_read: 0,
            outgoing: VecDeque::new(),
            queued_bytes: 0,
            frame_started: None,
        })
    }

    pub fn queue(&mut self, value: &Value) -> Result<()> {
        let body = serde_json::to_vec(value)?;
        ensure!(
            !body.is_empty() && body.len() <= MAX_MESSAGE,
            "Audio IPC message exceeds limit"
        );
        ensure!(
            self.outgoing.len() < MAX_PENDING && self.queued_bytes + body.len() + 4 <= MAX_MESSAGE,
            "Audio host command queue is full"
        );
        let mut frame = Vec::with_capacity(body.len() + 4);
        frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
        frame.extend(body);
        self.queued_bytes += frame.len();
        self.outgoing.push_back((frame, 0, Instant::now()));
        Ok(())
    }

    pub fn flush(&mut self) -> Result<()> {
        while let Some((frame, offset, started)) = self.outgoing.front_mut() {
            ensure!(
                started.elapsed() < Duration::from_secs(5),
                "Audio host stopped reading commands"
            );
            match self.stream.write(&frame[*offset..]) {
                Ok(0) => bail!("Audio host closed its command stream"),
                Ok(n) => *offset += n,
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) => return Err(error).context("Writing audio-host command"),
            }
            if *offset == frame.len() {
                self.queued_bytes -= frame.len();
                self.outgoing.pop_front();
            }
        }
        Ok(())
    }

    pub fn receive(&mut self) -> Result<Option<Value>> {
        loop {
            if let Some(start) = self.frame_started {
                ensure!(
                    start.elapsed() < Duration::from_secs(5),
                    "Audio host stalled inside an IPC message"
                );
            }
            let reading_header = self.header_read < 4;
            let target = if reading_header {
                &mut self.header[self.header_read..]
            } else {
                &mut self.body[self.body_read..]
            };
            match self.stream.read(target) {
                Ok(0) => bail!("Audio host disconnected"),
                Ok(n) => {
                    self.frame_started.get_or_insert_with(Instant::now);
                    if reading_header {
                        self.header_read += n;
                    } else {
                        self.body_read += n;
                    }
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => return Ok(None),
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) => return Err(error).context("Reading audio-host status"),
            }
            if self.header_read == 4 && self.body.is_empty() {
                let size = u32::from_be_bytes(self.header) as usize;
                ensure!(
                    size > 0 && size <= MAX_MESSAGE,
                    "Invalid audio IPC frame length"
                );
                self.body.resize(size, 0);
            }
            if !self.body.is_empty() && self.body_read == self.body.len() {
                let result = serde_json::from_slice(&self.body).context("Invalid audio-host JSON");
                self.header_read = 0;
                self.body.clear();
                self.body_read = 0;
                self.frame_started = None;
                return result.map(Some);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pair() -> (Connection, UnixStream) {
        let (a, b) = UnixStream::pair().unwrap();
        (Connection::new(a, std::process::id()).unwrap(), b)
    }

    #[test]
    fn fragmented_frames_and_multiple_messages_are_preserved() {
        let (mut connection, mut peer) = pair();
        let body = serde_json::to_vec(&json!({"type":"hello"})).unwrap();
        let header = (body.len() as u32).to_be_bytes();
        peer.write_all(&header[..2]).unwrap();
        assert!(connection.receive().unwrap().is_none());
        peer.write_all(&header[2..]).unwrap();
        peer.write_all(&body[..3]).unwrap();
        assert!(connection.receive().unwrap().is_none());
        peer.write_all(&body[3..]).unwrap();
        peer.write_all(&header).unwrap();
        peer.write_all(&body).unwrap();
        assert_eq!(connection.receive().unwrap().unwrap()["type"], "hello");
        assert_eq!(connection.receive().unwrap().unwrap()["type"], "hello");
        assert!(connection.receive().unwrap().is_none());
    }

    #[test]
    fn rejects_wrong_peer_oversized_zero_and_malformed_frames() {
        let (a, _) = UnixStream::pair().unwrap();
        assert!(Connection::new(a, u32::MAX).is_err());
        for size in [0, MAX_MESSAGE as u32 + 1] {
            let (mut connection, mut peer) = pair();
            peer.write_all(&size.to_be_bytes()).unwrap();
            assert!(connection.receive().is_err());
        }
        let (mut connection, mut peer) = pair();
        peer.write_all(&1u32.to_be_bytes()).unwrap();
        peer.write_all(b"{").unwrap();
        assert!(connection.receive().is_err());
    }

    #[test]
    fn outgoing_queue_is_bounded_and_serializes_network_order() {
        let (mut connection, mut peer) = pair();
        for _ in 0..MAX_PENDING {
            connection.queue(&json!({"type":"pause"})).unwrap();
        }
        assert!(connection.queue(&json!({})).is_err());
        connection.flush().unwrap();
        let mut header = [0; 4];
        peer.read_exact(&mut header).unwrap();
        let mut body = vec![0; u32::from_be_bytes(header) as usize];
        peer.read_exact(&mut body).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap()["type"],
            "pause"
        );
    }
}
