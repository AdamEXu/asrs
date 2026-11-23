//! What ssh2's `lib/protocol` makes of the bytes a server sends before encryption starts: the ident
//! line, packet framing and the key exchange packets up to NEWKEYS. `russh` does the key exchange
//! itself; this only watches it, because which error a client reports, and when, is ssh2's.

const MAX_LINE_LEN: usize = 8192;
const MAX_LINES: usize = 1024;
const MAX_PACKET_SIZE: i32 = 35000;

const DISCONNECT: u8 = 1;
const IGNORE: u8 = 2;
const UNIMPLEMENTED: u8 = 3;
const DEBUG: u8 = 4;
const KEXINIT: u8 = 20;
const NEWKEYS: u8 = 21;
const KEXDH_REPLY: u8 = 31;
const KEXDH_GEX_GROUP: u8 = 31;
const KEXDH_GEX_REPLY: u8 = 33;

const KEX_EXT_INFO: &str = "ext-info-c";
const GROUP_EXCHANGE: &str = "diffie-hellman-group-exchange-sha256";

/// The algorithms ssh2 1.14.0 offers under Electron's BoringSSL, which has no cipher ssh2 could use
/// for chacha20-poly1305. `ext-info-c` is an extension marker, not a key exchange.
pub const OFFERED_KEX: [&str; 12] = [
    "curve25519-sha256@libssh.org",
    "curve25519-sha256",
    "ecdh-sha2-nistp256",
    "ecdh-sha2-nistp384",
    "ecdh-sha2-nistp521",
    GROUP_EXCHANGE,
    "diffie-hellman-group14-sha256",
    "diffie-hellman-group15-sha512",
    "diffie-hellman-group16-sha512",
    "diffie-hellman-group17-sha512",
    "diffie-hellman-group18-sha512",
    KEX_EXT_INFO,
];
pub const OFFERED_HOST_KEYS: [&str; 7] = [
    "ssh-ed25519",
    "ecdsa-sha2-nistp256",
    "ecdsa-sha2-nistp384",
    "ecdsa-sha2-nistp521",
    "rsa-sha2-512",
    "rsa-sha2-256",
    "ssh-rsa",
];
pub const OFFERED_CIPHERS: [&str; 5] = [
    "aes128-gcm@openssh.com",
    "aes256-gcm@openssh.com",
    "aes128-ctr",
    "aes192-ctr",
    "aes256-ctr",
];
pub const OFFERED_MACS: [&str; 6] = [
    "hmac-sha2-256-etm@openssh.com",
    "hmac-sha2-512-etm@openssh.com",
    "hmac-sha1-etm@openssh.com",
    "hmac-sha2-256",
    "hmac-sha2-512",
    "hmac-sha1",
];
pub const OFFERED_COMPRESSION: [&str; 3] = ["none", "zlib@openssh.com", "zlib"];

#[derive(Clone, Debug, PartialEq)]
pub enum HandshakeEvent {
    /// The ident line was read. `openssh_limits` is whether it names OpenSSH or dropbear.
    Header { openssh_limits: bool },
    /// A DISCONNECT packet.
    Disconnect { reason: u32, description: String },
    /// The parser threw: ssh2's `data` listener reports it and ends the socket.
    Thrown(String),
    /// `doFatalError`
    Fatal { message: String, handshake: bool },
}

enum Kex {
    /// Waiting for the server's KEXINIT.
    Init,
    /// Waiting for the group of a group exchange.
    Group,
    /// Waiting for the reply, a packet of this type.
    Reply(u8),
    NewKeys,
}

enum State {
    Header {
        buffer: Option<Vec<u8>>,
    },
    Packets,
    /// Encrypted from here on, or stopped by an error.
    Done,
}

pub struct Handshake {
    state: State,
    kex: Kex,
    host_key: &'static str,
    skip_next: bool,
    length: i32,
    length_bytes: u8,
    packet: Vec<u8>,
}

impl Default for Handshake {
    fn default() -> Handshake {
        Handshake::new()
    }
}

impl Handshake {
    pub fn new() -> Handshake {
        Handshake {
            state: State::Header { buffer: None },
            kex: Kex::Init,
            host_key: "",
            skip_next: false,
            length: 0,
            length_bytes: 0,
            packet: Vec::new(),
        }
    }

    /// Watches one chunk from the socket. `forward` receives what the SSH transport should read: the
    /// ident line without what came before it, then everything up to the packet that failed.
    pub fn feed(&mut self, chunk: &[u8], forward: &mut Vec<u8>) -> Vec<HandshakeEvent> {
        let mut events = Vec::new();
        let mut rest = chunk;
        if matches!(self.state, State::Header { .. }) {
            match self.header(chunk) {
                Ok(None) => return events,
                Ok(Some((ident, used))) => {
                    self.state = State::Packets;
                    events.push(HandshakeEvent::Header {
                        openssh_limits: ident.starts_with(b"SSH-2.0-OpenSSH")
                            || ident.starts_with(b"SSH-2.0-dropbear"),
                    });
                    forward.extend_from_slice(&ident);
                    forward.extend_from_slice(b"\r\n");
                    rest = &chunk[used..];
                }
                Err(message) => {
                    self.state = State::Done;
                    events.push(HandshakeEvent::Thrown(message.to_owned()));
                    return events;
                }
            }
        }
        while !rest.is_empty() && matches!(self.state, State::Packets) {
            match self.packets(rest, &mut events) {
                Ok(used) => {
                    forward.extend_from_slice(&rest[..used]);
                    rest = &rest[used..];
                }
                Err(event) => {
                    self.state = State::Done;
                    events.push(event);
                    return events;
                }
            }
        }
        forward.extend_from_slice(rest);
        events
    }

    /// `parseHeader`: the ident line and how much of `chunk` it used, once there is one.
    fn header(&mut self, chunk: &[u8]) -> Result<Option<(Vec<u8>, usize)>, &'static str> {
        let State::Header { buffer } = &mut self.state else {
            return Ok(None);
        };
        // ssh2 keeps only the first incomplete chunk: the next one is appended to it for this call,
        // but not stored.
        let offset = buffer.as_ref().map_or(0, Vec::len);
        let mut data = buffer.clone().unwrap_or_default();
        data.extend_from_slice(chunk);
        let (mut start, mut end) = (0, 0);
        let (mut need_newline, mut line_len, mut lines) = (false, 0, 0);
        for (p, ch) in data.iter().enumerate() {
            match ch {
                b'\r' => {
                    need_newline = true;
                    continue;
                }
                b'\n' => {
                    if end > start && end - start > 4 && data[start..].starts_with(b"SSH-") {
                        let ident = &data[start..=end];
                        if !valid_ident(ident) {
                            return Err("Invalid identification string");
                        }
                        return Ok(Some((ident.to_vec(), p + 1 - offset)));
                    }
                    lines += 1;
                    if lines > MAX_LINES {
                        return Err("Max greeting lines exceeded");
                    }
                    need_newline = false;
                    start = p + 1;
                    line_len = 0;
                }
                _ if need_newline => return Err("Invalid header: expected newline"),
                _ => {
                    line_len += 1;
                    if line_len >= MAX_LINE_LEN {
                        return Err("Header line too long");
                    }
                }
            }
            end = p;
        }
        if buffer.is_none() {
            *buffer = Some(data);
        }
        Ok(None)
    }

    /// `NullDecipher.decrypt` for one packet or the part of one `data` holds.
    fn packets(
        &mut self,
        data: &[u8],
        events: &mut Vec<HandshakeEvent>,
    ) -> Result<usize, HandshakeEvent> {
        let mut used = 0;
        while self.length_bytes < 4 {
            let Some(byte) = data.get(used) else {
                return Ok(used);
            };
            self.length = (self.length << 8).wrapping_add(i32::from(*byte));
            self.length_bytes += 1;
            used += 1;
            if self.length_bytes == 4
                && (self.length > MAX_PACKET_SIZE || self.length < 8 || (4 + self.length) & 7 != 0)
            {
                return Err(HandshakeEvent::Thrown("Bad packet length".to_owned()));
            }
        }
        let missing = self.length as usize - self.packet.len();
        let part = &data[used..][..missing.min(data.len() - used)];
        self.packet.extend_from_slice(part);
        used += part.len();
        if self.packet.len() < self.length as usize {
            return Ok(used);
        }
        let packet = std::mem::take(&mut self.packet);
        self.length = 0;
        self.length_bytes = 0;
        let padding = usize::from(packet[0]);
        let Some(payload) = packet[1..].get(..(packet.len() - 1).wrapping_sub(padding)) else {
            return Err(HandshakeEvent::Thrown(format!(
                "Invalid typed array length: {}",
                packet.len() as i64 - 1 - padding as i64
            )));
        };
        if !payload.is_empty() {
            self.payload(payload, events)?;
        }
        Ok(used)
    }

    /// `onKEXPayload`
    fn payload(
        &mut self,
        payload: &[u8],
        events: &mut Vec<HandshakeEvent>,
    ) -> Result<(), HandshakeEvent> {
        if std::mem::take(&mut self.skip_next) {
            return Ok(());
        }
        let mut parser = Parser(&payload[1..]);
        match payload[0] {
            DISCONNECT => {
                let reason = parser.u32();
                let description = parser.string();
                match (reason, description, parser.bytes()) {
                    (Some(reason), Some(description), Some(_)) => {
                        events.push(HandshakeEvent::Disconnect {
                            reason,
                            description,
                        });
                        Ok(())
                    }
                    _ => Err(malformed("DISCONNECT")),
                }
            }
            IGNORE => Ok(()),
            UNIMPLEMENTED => parser.u32().map(|_| ()).ok_or(malformed("UNIMPLEMENTED")),
            DEBUG => {
                parser.byte();
                parser.string();
                parser.bytes().map(|_| ()).ok_or(malformed("DEBUG"))
            }
            KEXINIT => match self.kex {
                Kex::Init => self.kexinit(payload),
                _ => Err(fatal("Received extra KEXINIT during handshake")),
            },
            kind if !(20..=49).contains(&kind) => {
                Err(fatal(format!("Received unexpected packet type {kind}")))
            }
            kind => self.kex_packet(kind, parser),
        }
    }

    /// `handleKexInit`
    fn kexinit(&mut self, payload: &[u8]) -> Result<(), HandshakeEvent> {
        let mut parser = Parser(payload.get(17..).unwrap_or_default());
        let mut lists: [Vec<String>; 10] = Default::default();
        for list in &mut lists {
            let text = parser
                .string()
                .ok_or_else(|| fatal("Received malformed KEXINIT"))?;
            *list = text.split(',').map(str::to_owned).collect();
        }
        let first_follows = parser.byte() == Some(1);
        let [
            kex,
            host_key,
            cs_cipher,
            sc_cipher,
            cs_mac,
            sc_mac,
            cs_compression,
            sc_compression,
            _,
            _,
        ] = lists;
        let negotiated = choose(&OFFERED_KEX, &kex, "key exchange algorithm")?;
        if first_follows && kex.first().is_none_or(|first| first != negotiated) {
            self.skip_next = true;
        }
        self.host_key = choose(&OFFERED_HOST_KEYS, &host_key, "host key format")?;
        let cs = choose(&OFFERED_CIPHERS, &cs_cipher, "C->S cipher")?;
        let sc = choose(&OFFERED_CIPHERS, &sc_cipher, "S->C cipher")?;
        if !cs.contains("gcm") {
            choose(&OFFERED_MACS, &cs_mac, "C->S MAC")?;
        }
        if !sc.contains("gcm") {
            choose(&OFFERED_MACS, &sc_mac, "S->C MAC")?;
        }
        choose(&OFFERED_COMPRESSION, &cs_compression, "C->S compression")?;
        choose(&OFFERED_COMPRESSION, &sc_compression, "S->C compression")?;
        self.kex = match negotiated {
            KEX_EXT_INFO => {
                return Err(HandshakeEvent::Thrown(format!(
                    "Unsupported key exchange algorithm: {KEX_EXT_INFO}"
                )));
            }
            GROUP_EXCHANGE => Kex::Group,
            _ => Kex::Reply(KEXDH_REPLY),
        };
        Ok(())
    }

    /// `KeyExchange.parse`
    fn kex_packet(&mut self, kind: u8, mut parser: Parser<'_>) -> Result<(), HandshakeEvent> {
        let wanted = match self.kex {
            Kex::Init => {
                return Err(HandshakeEvent::Thrown(
                    "Cannot read properties of undefined (reading 'parse')".to_owned(),
                ));
            }
            Kex::Group => KEXDH_GEX_GROUP,
            Kex::Reply(kind) => kind,
            Kex::NewKeys => NEWKEYS,
        };
        if kind != wanted {
            return Err(fatal(format!("Received packet {kind} instead of {wanted}")));
        }
        match self.kex {
            Kex::Group => {
                if parser.bytes().is_none() || parser.bytes().is_none() {
                    return Err(fatal("Received malformed KEXDH_GEX_GROUP"));
                }
                self.kex = Kex::Reply(KEXDH_GEX_REPLY);
            }
            Kex::Reply(_) => {
                let (Some(key), Some(_), Some(_)) =
                    (parser.bytes(), parser.bytes(), parser.bytes())
                else {
                    return Err(fatal("Received malformed KEX*_REPLY"));
                };
                let Some(key_type) = Parser(key).string() else {
                    return Err(fatal("Received malformed host public key"));
                };
                let rsa = matches!(self.host_key, "rsa-sha2-256" | "rsa-sha2-512");
                if key_type != self.host_key && !(rsa && key_type == "ssh-rsa") {
                    return Err(fatal("Host key does not match negotiated type"));
                }
                self.kex = Kex::NewKeys;
            }
            Kex::Init | Kex::NewKeys => self.state = State::Done,
        }
        Ok(())
    }
}

fn valid_ident(ident: &[u8]) -> bool {
    [&b"SSH-2.0-"[..], b"SSH-1.99-"].iter().any(|prefix| {
        ident
            .strip_prefix(*prefix)
            .and_then(<[u8]>::first)
            .is_some_and(|first| *first != b' ')
    })
}

fn choose(
    offered: &[&'static str],
    remote: &[String],
    what: &str,
) -> Result<&'static str, HandshakeEvent> {
    offered
        .iter()
        .find(|name| remote.iter().any(|r| r == *name))
        .copied()
        .ok_or_else(|| fatal(format!("Handshake failed: no matching {what}")))
}

fn fatal(message: impl Into<String>) -> HandshakeEvent {
    HandshakeEvent::Fatal {
        message: message.into(),
        handshake: true,
    }
}

fn malformed(packet: &str) -> HandshakeEvent {
    HandshakeEvent::Fatal {
        message: format!("Inbound: Malformed {packet} packet"),
        handshake: false,
    }
}

struct Parser<'a>(&'a [u8]);

impl<'a> Parser<'a> {
    fn byte(&mut self) -> Option<u8> {
        let (byte, rest) = self.0.split_first()?;
        self.0 = rest;
        Some(*byte)
    }

    fn u32(&mut self) -> Option<u32> {
        let (bytes, rest) = self.0.split_first_chunk()?;
        self.0 = rest;
        Some(u32::from_be_bytes(*bytes))
    }

    fn bytes(&mut self) -> Option<&'a [u8]> {
        let len = self.u32()? as usize;
        let (bytes, rest) = self.0.split_at_checked(len)?;
        self.0 = rest;
        Some(bytes)
    }

    fn string(&mut self) -> Option<String> {
        self.bytes()
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
    }
}
