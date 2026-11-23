//! The client half of ssh2's `lib/protocol/SFTP.js` that the downloader uses: `readdir`, and
//! `fastGet` with its default 64 requests of 32768 bytes. Sans-IO: bytes from the channel go into
//! [`Sftp::push`], and everything the session does comes out of [`Sftp::take`] in order.
//!
//! Node's `fs` calls complete asynchronously, so a local file operation is an output whose result
//! comes back through [`Sftp::fs_done`]. The methods return `Err` with the message where ssh2 throws
//! out of the call, which its client turns into an `error` event.

use std::collections::{BTreeMap, HashMap, VecDeque};

use super::downloader::{DirEntry, Thrown};

const CHUNK_SIZE: f64 = 32768.0;
const CONCURRENCY: u32 = 64;
const MAX_PACKET_LEN: i32 = 256 * 1024;
const OVERHEAD: u32 = 2 * 1024;
const NO_RESPONSE: &str = "No response from server";

const INIT: u8 = 1;
const OPEN: u8 = 3;
const CLOSE: u8 = 4;
const READ: u8 = 5;
const FSTAT: u8 = 8;
const OPENDIR: u8 = 11;
const READDIR: u8 = 12;
const STAT: u8 = 17;
const EXTENDED: u8 = 200;
const VERSION: u8 = 2;
const STATUS: u8 = 101;
const HANDLE: u8 = 102;
const DATA: u8 = 103;
const NAME: u8 = 104;
const ATTRS: u8 = 105;
const EXTENDED_REPLY: u8 = 201;

const STATUS_OK: u32 = 0;
const STATUS_EOF: u32 = 1;
const LIMITS: &str = "limits@openssh.com";

pub type Fd = u64;

#[derive(Clone, Debug, PartialEq)]
pub enum Output {
    /// Channel data.
    Send(Vec<u8>),
    /// `CHANNEL_CLOSE`
    Close,
    Ready,
    /// An `error` event that has a listener, which it has until the session is ready.
    Error(String),
    Readdir {
        id: u32,
        result: Result<Vec<DirEntry>, String>,
    },
    Step {
        id: u32,
        total: f64,
        chunk: f64,
        size: f64,
    },
    Got {
        id: u32,
        result: Result<(), String>,
    },
    /// `fs.open(path, "w")`
    FsOpen {
        path: String,
    },
    FsWrite {
        fd: Fd,
        data: Vec<u8>,
        position: f64,
    },
    FsClose {
        fd: Fd,
    },
}

struct SftpError {
    message: String,
    code: Option<u32>,
}

impl SftpError {
    fn no_response() -> SftpError {
        SftpError {
            message: NO_RESPONSE.to_owned(),
            code: None,
        }
    }
}

struct Attrs {
    size: Option<f64>,
    mode: Option<u32>,
}

enum Reply {
    Ok,
    Handle(Vec<u8>),
    Data(Vec<u8>),
    Names(Vec<DirEntry>),
    Attrs(Attrs),
    Extended { max_read: f64 },
}

struct Read {
    id: u32,
    handle: Vec<u8>,
    /// Bytes of earlier answers, when the server's read limit splits the request.
    data: Vec<u8>,
    position: f64,
    len: f64,
    overflow: f64,
    write_position: f64,
    chunk: f64,
}

enum Callback {
    Limits,
    Opendir(u32),
    Readdir {
        id: u32,
        handle: Vec<u8>,
        entries: Vec<DirEntry>,
    },
    Closedir {
        id: u32,
        result: Result<Vec<DirEntry>, String>,
    },
    Open(u32),
    Fstat(u32),
    Stat {
        id: u32,
        fstat: SftpError,
    },
    Read(Read),
    CloseDone(u32),
    CloseAfterError(u32),
}

enum FsCallback {
    Open(u32),
    Write {
        id: u32,
        len: f64,
        position: f64,
        chunk: f64,
    },
    CloseDone(u32),
    CloseAfterError(u32),
}

struct Transfer {
    remote: String,
    local: String,
    size: f64,
    next: f64,
    total: f64,
    had_error: bool,
    remote_handle: Option<Vec<u8>>,
    fd: Option<Fd>,
    closing: i32,
    error: Option<String>,
}

pub struct Sftp {
    version: Option<u32>,
    open: bool,
    /// Whether the client's `ready` and `error` listeners are attached.
    listening: bool,
    failed: bool,
    max_read: f64,
    length: i32,
    length_bytes: u8,
    packet: Vec<u8>,
    request_id: u32,
    requests: BTreeMap<u32, Callback>,
    transfers: HashMap<u32, Transfer>,
    fs_queue: VecDeque<FsCallback>,
    out: Vec<Output>,
}

struct Parser<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(data: &'a [u8]) -> Parser<'a> {
        Parser { data, pos: 1 }
    }

    fn u32(&mut self) -> Option<u32> {
        let bytes = self.data.get(self.pos..)?.first_chunk()?;
        self.pos += 4;
        Some(u32::from_be_bytes(*bytes))
    }

    /// ssh2 adds the bytes up as doubles.
    fn u64(&mut self) -> Option<f64> {
        let bytes: &[u8; 8] = self.data.get(self.pos..)?.first_chunk()?;
        self.pos += 8;
        let weights = [
            72057594037927940.0,
            281474976710656.0,
            1099511627776.0,
            4294967296.0,
            16777216.0,
            65536.0,
            256.0,
            1.0,
        ];
        Some(
            bytes
                .iter()
                .zip(weights)
                .fold(0.0, |sum, (byte, weight)| sum + f64::from(*byte) * weight),
        )
    }

    fn bytes(&mut self) -> Option<&'a [u8]> {
        let len = self.u32()? as usize;
        let bytes = self.data.get(self.pos..)?.get(..len)?;
        self.pos += len;
        Some(bytes)
    }

    fn string(&mut self) -> Option<String> {
        self.bytes()
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
    }

    fn attrs(&mut self) -> Option<Attrs> {
        let flags = self.u32()?;
        let mut attrs = Attrs {
            size: None,
            mode: None,
        };
        if flags & 1 != 0 {
            attrs.size = Some(self.u64()?);
        }
        if flags & 2 != 0 {
            self.u32();
            self.u32()?;
        }
        if flags & 4 != 0 {
            attrs.mode = Some(self.u32()?);
        }
        if flags & 8 != 0 {
            self.u32();
            self.u32()?;
        }
        if flags & 0x8000_0000 != 0 {
            for _ in 0..self.u32()? {
                self.string();
                self.bytes()?;
            }
        }
        Some(attrs)
    }
}

fn status_text(code: u32) -> &'static str {
    match code {
        0 => "No error",
        1 => "End of file",
        2 => "No such file or directory",
        3 => "Permission denied",
        4 => "Failure",
        5 => "Bad message",
        6 => "No connection",
        7 => "Connection lost",
        8 => "Operation unsupported",
        _ => "Unknown status",
    }
}

impl Sftp {
    /// `openssh_limits`: whether the server's ident names OpenSSH or dropbear, for which ssh2 uses
    /// larger packets.
    pub fn new(openssh_limits: bool) -> Sftp {
        let max_packet = if openssh_limits { 256 * 1024 } else { 34000 };
        Sftp {
            version: None,
            open: true,
            listening: true,
            failed: false,
            max_read: f64::from(max_packet - OVERHEAD),
            length: 0,
            length_bytes: 0,
            packet: Vec::new(),
            request_id: u32::MAX,
            requests: BTreeMap::new(),
            transfers: HashMap::new(),
            fs_queue: VecDeque::new(),
            out: Vec::new(),
        }
    }

    pub fn take(&mut self) -> Vec<Output> {
        std::mem::take(&mut self.out)
    }

    /// The client stopped listening for `ready` and `error` (the channel ended while starting).
    pub fn stop_listening(&mut self) {
        self.listening = false;
    }

    pub fn init(&mut self) {
        self.write(vec![0, 0, 0, 5, INIT, 0, 0, 0, 3]);
    }

    fn write(&mut self, packet: Vec<u8>) {
        if self.open {
            self.out.push(Output::Send(packet));
        }
    }

    fn request(&mut self, kind: u8, body: &[u8], callback: Callback) {
        self.request_id = self.request_id.wrapping_add(1);
        let mut packet = Vec::with_capacity(body.len() + 9);
        packet.extend_from_slice(&(body.len() as u32 + 5).to_be_bytes());
        packet.push(kind);
        packet.extend_from_slice(&self.request_id.to_be_bytes());
        packet.extend_from_slice(body);
        self.requests.insert(self.request_id, callback);
        self.write(packet);
    }

    pub fn readdir(&mut self, id: u32, path: &str) {
        self.request(OPENDIR, &string(path.as_bytes()), Callback::Opendir(id));
    }

    pub fn fast_get(&mut self, id: u32, remote: &str, local: &str) {
        self.transfers.insert(
            id,
            Transfer {
                remote: remote.to_owned(),
                local: local.to_owned(),
                size: 0.0,
                next: 0.0,
                total: 0.0,
                had_error: false,
                remote_handle: None,
                fd: None,
                closing: 0,
                error: None,
            },
        );
        let mut body = string(remote.as_bytes());
        body.extend_from_slice(&1u32.to_be_bytes());
        body.extend_from_slice(&0u32.to_be_bytes());
        self.request(OPEN, &body, Callback::Open(id));
    }

    /// Channel EOF or close: `push(null)`.
    pub fn end(&mut self) -> Thrown {
        self.cleanup()
    }

    fn cleanup(&mut self) -> Thrown {
        for callback in std::mem::take(&mut self.requests).into_values() {
            self.call(callback, Err(SftpError::no_response()))?;
        }
        Ok(())
    }

    fn fatal(&mut self, message: &str) -> Thrown {
        self.failed = true;
        if !self.listening {
            return Err(message.to_owned());
        }
        self.listening = false;
        self.out.push(Output::Error(message.to_owned()));
        if self.open {
            self.open = false;
            self.out.push(Output::Close);
        }
        self.cleanup()
    }

    /// Channel data.
    pub fn push(&mut self, mut data: &[u8]) -> Thrown {
        while !data.is_empty() && !self.failed {
            while self.length_bytes < 4 {
                let Some((byte, rest)) = data.split_first() else {
                    return Ok(());
                };
                self.length = (self.length << 8).wrapping_add(i32::from(*byte));
                self.length_bytes += 1;
                data = rest;
                if self.length_bytes == 4 {
                    if self.length == 0 {
                        return self.fatal("Invalid packet length");
                    }
                    if self.length > MAX_PACKET_LEN {
                        let message = format!(
                            "Packet length {} exceeds max length of {MAX_PACKET_LEN}",
                            self.length
                        );
                        return self.fatal(&message);
                    }
                }
            }
            if data.is_empty() {
                return Ok(());
            }
            if self.length < 0 {
                self.failed = true;
                return Err("Cannot read properties of undefined (reading '0')".to_owned());
            }
            let missing = self.length as usize - self.packet.len();
            let (part, rest) = data.split_at(missing.min(data.len()));
            self.packet.extend_from_slice(part);
            data = rest;
            if self.packet.len() < self.length as usize {
                return Ok(());
            }
            let packet = std::mem::take(&mut self.packet);
            self.length = 0;
            self.length_bytes = 0;
            self.packet_received(&packet)?;
        }
        Ok(())
    }

    fn packet_received(&mut self, packet: &[u8]) -> Thrown {
        let kind = packet[0];
        if ![VERSION, STATUS, HANDLE, DATA, NAME, ATTRS, EXTENDED_REPLY].contains(&kind) {
            return self.fatal(&format!("Unknown packet type {kind}"));
        }
        if self.version.is_none() && kind != VERSION {
            return self.fatal(&format!("Expected VERSION packet, got {kind}"));
        }
        let mut parser = Parser::new(packet);
        if kind == VERSION {
            return self.version_received(&mut parser);
        }
        let id = parser.u32();
        match kind {
            STATUS => {
                let code = parser.u32();
                let text = parser.string();
                let Some(callback) = id.and_then(|id| self.requests.remove(&id)) else {
                    return Ok(());
                };
                if code == Some(STATUS_OK) {
                    return self.call(callback, Ok(Reply::Ok));
                }
                let message = text
                    .filter(|text| !text.is_empty())
                    .unwrap_or_else(|| code.map_or("Unknown status", status_text).to_owned());
                self.call(callback, Err(SftpError { message, code }))
            }
            HANDLE => {
                let handle = parser.bytes();
                let callback = id.and_then(|id| self.requests.remove(&id));
                match (handle, callback) {
                    (None, _) => self.fatal("Malformed HANDLE packet"),
                    (Some(handle), Some(callback)) => {
                        self.call(callback, Ok(Reply::Handle(handle.to_vec())))
                    }
                    (Some(_), None) => Ok(()),
                }
            }
            DATA => {
                let data = parser.bytes();
                let callback = id.and_then(|id| self.requests.remove(&id));
                match (data, callback) {
                    (None, _) => self.fatal("Malformed DATA packet"),
                    (Some(data), Some(callback)) => {
                        self.call(callback, Ok(Reply::Data(data.to_vec())))
                    }
                    (Some(_), None) => Ok(()),
                }
            }
            NAME => {
                let callback = id.and_then(|id| self.requests.remove(&id));
                let names = names(&mut parser);
                match (names, callback) {
                    (None, _) => self.fatal("Malformed NAME packet"),
                    (Some(names), Some(callback)) => self.call(callback, Ok(Reply::Names(names))),
                    (Some(_), None) => Ok(()),
                }
            }
            ATTRS => {
                let callback = id.and_then(|id| self.requests.remove(&id));
                match (parser.attrs(), callback) {
                    (None, _) => self.fatal("Malformed ATTRS packet"),
                    (Some(attrs), Some(callback)) => self.call(callback, Ok(Reply::Attrs(attrs))),
                    (Some(_), None) => Ok(()),
                }
            }
            _ => {
                let Some(id) = id else {
                    return self.fatal("Malformed EXTENDED_REPLY packet");
                };
                let Some(callback) = self.requests.remove(&id) else {
                    return Ok(());
                };
                if !matches!(callback, Callback::Limits) {
                    return self.call(callback, Ok(Reply::Ok));
                }
                let limits = (parser.u64(), parser.u64(), parser.u64(), parser.u64());
                match limits {
                    (_, Some(max_read), _, Some(_)) => {
                        self.call(callback, Ok(Reply::Extended { max_read }))
                    }
                    _ => self.fatal("Malformed EXTENDED_REPLY packet"),
                }
            }
        }
    }

    fn version_received(&mut self, parser: &mut Parser<'_>) -> Thrown {
        if self.version.is_some() {
            return self.fatal("Duplicate VERSION packet");
        }
        let mut version = parser.u32();
        let mut limits = None;
        while parser.pos < parser.data.len() {
            let name = parser.string();
            let Some(value) = parser.string() else {
                version = None;
                break;
            };
            if name.as_deref() == Some(LIMITS) {
                limits = Some(value);
            }
        }
        if version.is_none() {
            return self.fatal("Malformed VERSION packet");
        }
        self.version = version;
        if limits.as_deref() == Some("1") {
            self.request(EXTENDED, &string(LIMITS.as_bytes()), Callback::Limits);
        } else {
            self.ready();
        }
        Ok(())
    }

    fn ready(&mut self) {
        if self.listening {
            self.listening = false;
            self.out.push(Output::Ready);
        }
    }

    fn call(&mut self, callback: Callback, reply: Result<Reply, SftpError>) -> Thrown {
        match callback {
            Callback::Limits => {
                match reply {
                    Ok(Reply::Extended { max_read }) => {
                        if max_read > 0.0 {
                            self.max_read = max_read;
                        }
                    }
                    Ok(_) => {
                        return Err(
                            "Cannot read properties of undefined (reading 'maxPktLen')".to_owned()
                        );
                    }
                    Err(_) => {}
                }
                self.ready();
            }
            Callback::Opendir(id) => match reply {
                Ok(Reply::Handle(handle) | Reply::Data(handle)) => {
                    self.reread(id, handle, Vec::new());
                }
                Ok(_) => return Err("missing directory handle or path".to_owned()),
                Err(e) => self.readdir_done(id, Err(e.message)),
            },
            Callback::Readdir {
                id,
                handle,
                mut entries,
            } => match reply {
                Ok(Reply::Names(names)) => {
                    entries.extend(
                        names
                            .into_iter()
                            .filter(|entry| entry.filename != "." && entry.filename != ".."),
                    );
                    self.reread(id, handle, entries);
                }
                Ok(_) => {
                    return Err("Cannot read properties of undefined (reading 'length')".to_owned());
                }
                Err(e) => {
                    let result = if e.code == Some(STATUS_EOF) {
                        Ok(entries)
                    } else {
                        Err(e.message)
                    };
                    self.request(CLOSE, &string(&handle), Callback::Closedir { id, result });
                }
            },
            Callback::Closedir { id, result } => {
                let result = match (result, reply) {
                    (Ok(_), Err(e)) => Err(e.message),
                    (result, _) => result,
                };
                self.readdir_done(id, result);
            }
            Callback::Open(id) => match reply {
                Ok(Reply::Handle(handle) | Reply::Data(handle)) => {
                    let body = string(&handle);
                    self.transfer(id).remote_handle = Some(handle);
                    self.request(FSTAT, &body, Callback::Fstat(id));
                }
                Ok(_) => return Err("handle is not a Buffer".to_owned()),
                Err(e) => self.transfer_error(id, Some(e.message)),
            },
            Callback::Fstat(id) => match reply {
                Ok(reply) => self.stat_received(id, reply)?,
                Err(fstat) => {
                    let body = string(self.transfer(id).remote.as_bytes());
                    self.request(STAT, &body, Callback::Stat { id, fstat });
                }
            },
            Callback::Stat { id, fstat } => match reply {
                Ok(reply) => self.stat_received(id, reply)?,
                Err(_) => self.transfer_error(id, Some(fstat.message)),
            },
            Callback::Read(read) => self.read_done(read, reply),
            Callback::CloseDone(id) => {
                self.transfer(id).remote_handle = None;
                match reply {
                    Err(e) => self.transfer_error(id, Some(e.message)),
                    Ok(_) => self.out.push(Output::Got { id, result: Ok(()) }),
                }
            }
            Callback::CloseAfterError(id) => self.closed_after_error(id),
        }
        Ok(())
    }

    fn reread(&mut self, id: u32, handle: Vec<u8>, entries: Vec<DirEntry>) {
        let body = string(&handle);
        self.request(
            READDIR,
            &body,
            Callback::Readdir {
                id,
                handle,
                entries,
            },
        );
    }

    fn readdir_done(&mut self, id: u32, result: Result<Vec<DirEntry>, String>) {
        self.out.push(Output::Readdir { id, result });
    }

    fn transfer(&mut self, id: u32) -> &mut Transfer {
        self.transfers.get_mut(&id).expect("a started transfer")
    }

    fn stat_received(&mut self, id: u32, reply: Reply) -> Thrown {
        let Reply::Attrs(attrs) = reply else {
            return Err("Cannot read properties of undefined (reading 'size')".to_owned());
        };
        let transfer = self.transfer(id);
        transfer.size = attrs.size.unwrap_or(f64::NAN);
        let path = transfer.local.clone();
        self.fs_queue.push_back(FsCallback::Open(id));
        self.out.push(Output::FsOpen { path });
        Ok(())
    }

    /// The oldest [`Output::FsOpen`], [`Output::FsWrite`] or [`Output::FsClose`] without a result
    /// finished. An open's value is its descriptor; the others' is not used.
    pub fn fs_done(&mut self, result: Result<Fd, String>) {
        match self.fs_queue.pop_front().expect("a pending fs call") {
            FsCallback::Open(id) => match result {
                Err(message) => self.transfer_error(id, Some(message)),
                Ok(fd) => self.local_opened(id, fd),
            },
            FsCallback::Write {
                id,
                len,
                position,
                chunk,
            } => match result {
                Err(message) => self.transfer_error(id, Some(message)),
                Ok(_) => self.written(id, len, position, chunk),
            },
            FsCallback::CloseDone(id) => {
                let transfer = self.transfer(id);
                transfer.fd = None;
                let handle = transfer.remote_handle.clone();
                match (result, handle) {
                    (Err(message), _) => self.transfer_error(id, Some(message)),
                    (Ok(_), Some(handle)) => {
                        self.request(CLOSE, &string(&handle), Callback::CloseDone(id));
                    }
                    (Ok(_), None) => {}
                }
            }
            FsCallback::CloseAfterError(id) => self.closed_after_error(id),
        }
    }

    fn local_opened(&mut self, id: u32, fd: Fd) {
        let transfer = self.transfer(id);
        transfer.fd = Some(fd);
        let size = transfer.size;
        if size <= 0.0 {
            self.transfer_error(id, None);
            return;
        }
        let mut concurrency = CONCURRENCY;
        while CHUNK_SIZE * f64::from(concurrency) > size && concurrency > 1 {
            concurrency -= 1;
        }
        for _ in 0..concurrency {
            if !self.read_next(id) {
                break;
            }
        }
    }

    /// Requests the next chunk nobody asked for yet.
    fn read_next(&mut self, id: u32) -> bool {
        let transfer = self.transfer(id);
        let position = transfer.next;
        if position >= transfer.size || transfer.size.is_nan() {
            return false;
        }
        let chunk = if position + CHUNK_SIZE > transfer.size {
            transfer.size - position
        } else {
            CHUNK_SIZE
        };
        transfer.next += chunk;
        self.single_read(id, position, chunk);
        true
    }

    fn single_read(&mut self, id: u32, position: f64, chunk: f64) {
        let handle = self.transfer(id).remote_handle.clone().unwrap_or_default();
        self.read(Read {
            id,
            handle,
            data: Vec::new(),
            position,
            len: chunk,
            overflow: 0.0,
            write_position: position,
            chunk,
        });
    }

    fn read(&mut self, mut read: Read) {
        read.overflow = 0.0;
        if read.len > self.max_read {
            read.overflow = read.len - self.max_read;
            read.len = self.max_read;
        }
        let mut body = string(&read.handle);
        body.extend_from_slice(&(read.position as u64).to_be_bytes());
        body.extend_from_slice(&(read.len as u32).to_be_bytes());
        self.request(READ, &body, Callback::Read(read));
    }

    fn read_done(&mut self, mut read: Read, reply: Result<Reply, SftpError>) {
        match reply {
            Err(e) if e.code != Some(STATUS_EOF) => {
                self.transfer_error(read.id, Some(e.message));
                return;
            }
            Ok(Reply::Data(data)) => {
                let len = data.len().min(read.len as usize);
                read.data.extend_from_slice(&data[..len]);
                if len as f64 == read.len && read.overflow > 0.0 {
                    read.position += len as f64;
                    read.len = read.overflow;
                    self.read(read);
                    return;
                }
            }
            _ => {}
        }
        let Some(fd) = self.transfer(read.id).fd else {
            return;
        };
        self.fs_queue.push_back(FsCallback::Write {
            id: read.id,
            len: read.data.len() as f64,
            position: read.write_position,
            chunk: read.chunk,
        });
        self.out.push(Output::FsWrite {
            fd,
            data: read.data,
            position: read.write_position,
        });
    }

    fn written(&mut self, id: u32, len: f64, position: f64, chunk: f64) {
        let transfer = self.transfer(id);
        transfer.total += len;
        let (total, size) = (transfer.total, transfer.size);
        self.out.push(Output::Step {
            id,
            total,
            chunk: len,
            size,
        });
        if len < chunk {
            self.single_read(id, position + len, chunk - len);
        } else if total == size {
            if let Some(fd) = self.transfer(id).fd {
                self.fs_queue.push_back(FsCallback::CloseDone(id));
                self.out.push(Output::FsClose { fd });
            }
        } else {
            self.read_next(id);
        }
    }

    /// `fastXfer`'s `onerror`. An empty file ends here too, without a message.
    fn transfer_error(&mut self, id: u32, message: Option<String>) {
        let open = self.open;
        let transfer = self.transfer(id);
        if transfer.had_error {
            return;
        }
        transfer.had_error = true;
        let remote = transfer.remote_handle.clone();
        let fd = transfer.fd;
        if remote.is_none() && fd.is_none() {
            self.got(id, message);
            return;
        }
        let remote = remote.filter(|_| open);
        transfer.closing = i32::from(remote.is_some()) + i32::from(fd.is_some());
        transfer.error = message;
        if let Some(handle) = remote {
            self.request(CLOSE, &string(&handle), Callback::CloseAfterError(id));
        }
        if let Some(fd) = fd {
            self.fs_queue.push_back(FsCallback::CloseAfterError(id));
            self.out.push(Output::FsClose { fd });
        }
    }

    fn closed_after_error(&mut self, id: u32) {
        let transfer = self.transfer(id);
        transfer.closing -= 1;
        if transfer.closing == 0 {
            let message = transfer.error.take();
            self.got(id, message);
        }
    }

    fn got(&mut self, id: u32, message: Option<String>) {
        self.out.push(Output::Got {
            id,
            result: message.map_or(Ok(()), Err),
        });
    }
}

fn string(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + 4);
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(bytes);
    out
}

fn names(parser: &mut Parser<'_>) -> Option<Vec<DirEntry>> {
    let mut names = Vec::new();
    for _ in 0..parser.u32()? {
        let filename = parser.string();
        parser.string();
        let attrs = parser.attrs()?;
        names.push(DirEntry {
            filename: filename?,
            mode: attrs.mode,
            size: attrs.size,
        });
    }
    Some(names)
}
