//! Binary message protocol over Unix sockets.
//!
//! Wire format: an 8-byte header { tag: u8, len: u32 LE, 3 pad bytes }
//! followed by `len` payload bytes. The 8-byte size (rather than 5) is kept
//! for compatibility with the original wire format, which external tooling
//! and the bats integration suite encode by hand.

use std::io::{self, Write};
use std::os::fd::RawFd;

pub const HEADER_LEN: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Tag {
    Input = 0,
    Output = 1,
    Resize = 2,
    Detach = 3,
    DetachAll = 4,
    Kill = 5,
    Info = 6,
    Init = 7,
    History = 8,
    Run = 9,
    Ack = 10,
}

impl Tag {
    pub fn from_u8(v: u8) -> Option<Tag> {
        Some(match v {
            0 => Tag::Input,
            1 => Tag::Output,
            2 => Tag::Resize,
            3 => Tag::Detach,
            4 => Tag::DetachAll,
            5 => Tag::Kill,
            6 => Tag::Info,
            7 => Tag::Init,
            8 => Tag::History,
            9 => Tag::Run,
            10 => Tag::Ack,
            _ => return None,
        })
    }
}

/// Window size payload for Init/Resize messages: rows then cols, both u16 LE.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Resize {
    pub rows: u16,
    pub cols: u16,
}

impl Resize {
    pub const WIRE_LEN: usize = 4;

    pub fn encode(&self) -> [u8; Self::WIRE_LEN] {
        let mut buf = [0u8; Self::WIRE_LEN];
        buf[0..2].copy_from_slice(&self.rows.to_le_bytes());
        buf[2..4].copy_from_slice(&self.cols.to_le_bytes());
        buf
    }

    pub fn decode(payload: &[u8]) -> Option<Resize> {
        if payload.len() != Self::WIRE_LEN {
            return None;
        }
        Some(Resize {
            rows: u16::from_le_bytes([payload[0], payload[1]]),
            cols: u16::from_le_bytes([payload[2], payload[3]]),
        })
    }
}

pub const MAX_CMD_LEN: usize = 256;
pub const MAX_CWD_LEN: usize = 256;

/// Session metadata returned for Info requests. Fixed 528-byte layout:
/// clients_len u64 LE, pid i32 LE, cmd_len u16 LE, cwd_len u16 LE,
/// cmd[256], cwd[256].
#[derive(Debug, Clone)]
pub struct Info {
    pub clients_len: u64,
    pub pid: i32,
    pub cmd: Vec<u8>,
    pub cwd: Vec<u8>,
}

impl Info {
    pub const WIRE_LEN: usize = 8 + 4 + 2 + 2 + MAX_CMD_LEN + MAX_CWD_LEN;

    pub fn encode(&self) -> Vec<u8> {
        let mut buf = vec![0u8; Self::WIRE_LEN];
        buf[0..8].copy_from_slice(&self.clients_len.to_le_bytes());
        buf[8..12].copy_from_slice(&self.pid.to_le_bytes());
        let cmd_len = self.cmd.len().min(MAX_CMD_LEN);
        let cwd_len = self.cwd.len().min(MAX_CWD_LEN);
        buf[12..14].copy_from_slice(&(cmd_len as u16).to_le_bytes());
        buf[14..16].copy_from_slice(&(cwd_len as u16).to_le_bytes());
        buf[16..16 + cmd_len].copy_from_slice(&self.cmd[..cmd_len]);
        buf[16 + MAX_CMD_LEN..16 + MAX_CMD_LEN + cwd_len].copy_from_slice(&self.cwd[..cwd_len]);
        buf
    }

    pub fn decode(payload: &[u8]) -> Option<Info> {
        if payload.len() != Self::WIRE_LEN {
            return None;
        }
        let clients_len = u64::from_le_bytes(payload[0..8].try_into().unwrap());
        let pid = i32::from_le_bytes(payload[8..12].try_into().unwrap());
        let cmd_len = u16::from_le_bytes([payload[12], payload[13]]) as usize;
        let cwd_len = u16::from_le_bytes([payload[14], payload[15]]) as usize;
        let cmd_len = cmd_len.min(MAX_CMD_LEN);
        let cwd_len = cwd_len.min(MAX_CWD_LEN);
        Some(Info {
            clients_len,
            pid,
            cmd: payload[16..16 + cmd_len].to_vec(),
            cwd: payload[16 + MAX_CMD_LEN..16 + MAX_CMD_LEN + cwd_len].to_vec(),
        })
    }
}

fn encode_header(tag: Tag, len: usize) -> [u8; HEADER_LEN] {
    let mut hdr = [0u8; HEADER_LEN];
    hdr[0] = tag as u8;
    hdr[1..5].copy_from_slice(&(len as u32).to_le_bytes());
    hdr
}

/// Total message length (header + payload) once enough bytes are buffered to
/// know it, or None if the header is still incomplete.
pub fn expected_length(data: &[u8]) -> Option<usize> {
    if data.len() < HEADER_LEN {
        return None;
    }
    let len = u32::from_le_bytes(data[1..5].try_into().unwrap()) as usize;
    Some(HEADER_LEN + len)
}

/// Append a framed message to an outgoing buffer.
pub fn append_message(buf: &mut Vec<u8>, tag: Tag, payload: &[u8]) {
    buf.extend_from_slice(&encode_header(tag, payload.len()));
    buf.extend_from_slice(payload);
}

/// Blocking write of a complete framed message to a socket fd.
pub fn send(fd: RawFd, tag: Tag, payload: &[u8]) -> io::Result<()> {
    let mut msg = Vec::with_capacity(HEADER_LEN + payload.len());
    append_message(&mut msg, tag, payload);
    write_all(fd, &msg)
}

fn write_all(fd: RawFd, data: &[u8]) -> io::Result<()> {
    let mut index = 0;
    while index < data.len() {
        let n = unsafe {
            libc::write(
                fd,
                data[index..].as_ptr() as *const libc::c_void,
                data.len() - index,
            )
        };
        if n < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "write returned 0"));
        }
        index += n as usize;
    }
    Ok(())
}

#[derive(Debug)]
pub struct SocketMsg {
    pub tag: Tag,
    pub payload: Vec<u8>,
}

/// Accumulates raw socket reads and yields complete framed messages.
pub struct SocketBuffer {
    buf: Vec<u8>,
    head: usize,
}

impl Default for SocketBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl SocketBuffer {
    pub fn new() -> SocketBuffer {
        SocketBuffer {
            buf: Vec::with_capacity(4096),
            head: 0,
        }
    }

    /// Read once from fd into the buffer. Returns Ok(0) on EOF; propagates
    /// WouldBlock and other errors to the caller.
    pub fn read(&mut self, fd: RawFd) -> io::Result<usize> {
        if self.head > 0 {
            self.buf.drain(..self.head);
            self.head = 0;
        }
        let mut tmp = [0u8; 4096];
        let n = unsafe { libc::read(fd, tmp.as_mut_ptr() as *mut libc::c_void, tmp.len()) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        let n = n as usize;
        self.buf.extend_from_slice(&tmp[..n]);
        Ok(n)
    }

    /// Feed bytes directly (for tests).
    #[cfg(test)]
    pub fn feed(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// Pop the next complete message, or None when more bytes are needed.
    /// Messages with an unknown tag are skipped.
    pub fn next(&mut self) -> Option<SocketMsg> {
        loop {
            let available = &self.buf[self.head..];
            let total = expected_length(available)?;
            if available.len() < total {
                return None;
            }
            let tag = Tag::from_u8(available[0]);
            let payload = available[HEADER_LEN..total].to_vec();
            self.head += total;
            match tag {
                Some(tag) => return Some(SocketMsg { tag, payload }),
                None => continue,
            }
        }
    }
}

/// Buffered stdout writer helper used by CLI subcommands.
pub fn print_stdout(data: &str) {
    let stdout = io::stdout();
    let mut lock = stdout.lock();
    let _ = lock.write_all(data.as_bytes());
    let _ = lock.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_is_eight_bytes_with_le_length() {
        let mut buf = Vec::new();
        append_message(&mut buf, Tag::Input, b"hi");
        assert_eq!(buf.len(), HEADER_LEN + 2);
        assert_eq!(&buf[..HEADER_LEN], &[0, 2, 0, 0, 0, 0, 0, 0]);
        assert_eq!(&buf[HEADER_LEN..], b"hi");
    }

    #[test]
    fn socket_buffer_yields_complete_messages() {
        let mut sb = SocketBuffer::new();
        let mut msg = Vec::new();
        append_message(&mut msg, Tag::Init, &Resize { rows: 24, cols: 80 }.encode());
        // Feed in two halves: nothing until complete.
        sb.feed(&msg[..5]);
        assert!(sb.next().is_none());
        sb.feed(&msg[5..]);
        let got = sb.next().expect("complete message");
        assert_eq!(got.tag, Tag::Init);
        let resize = Resize::decode(&got.payload).unwrap();
        assert_eq!(resize, Resize { rows: 24, cols: 80 });
        assert!(sb.next().is_none());
    }

    #[test]
    fn truncated_control_message_is_not_dispatched() {
        // A 5-byte DetachAll (shorter than the 8-byte header) never completes.
        let mut sb = SocketBuffer::new();
        sb.feed(&[4, 0, 0, 0, 0]);
        assert!(sb.next().is_none());
    }

    #[test]
    fn info_roundtrip() {
        let info = Info {
            clients_len: 3,
            pid: 4242,
            cmd: b"bash -lc x".to_vec(),
            cwd: b"/home/user".to_vec(),
        };
        let encoded = info.encode();
        assert_eq!(encoded.len(), Info::WIRE_LEN);
        let decoded = Info::decode(&encoded).unwrap();
        assert_eq!(decoded.clients_len, 3);
        assert_eq!(decoded.pid, 4242);
        assert_eq!(decoded.cmd, b"bash -lc x");
        assert_eq!(decoded.cwd, b"/home/user");
    }

    #[test]
    fn resize_wire_order_is_rows_then_cols() {
        let r = Resize {
            rows: 50,
            cols: 200,
        };
        assert_eq!(r.encode(), [50, 0, 200, 0]);
    }
}
