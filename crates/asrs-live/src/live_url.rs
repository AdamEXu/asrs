//! The `http:` and `ws:` URLs the live sources build from an address, parsed as Chromium's URL
//! library does. It differs from the WHATWG parser in the `url` crate: a space or `*` in a host is
//! escaped, an ASCII `xn--` label is not checked, and paths escape `|` and `^`.

use idna::uts46::{AsciiDenyList, DnsLength, Hyphens, Uts46};
use url::{Host, Url};

/// Chromium's restricted ports: a request to one fails without a connection.
const BLOCKED_PORTS: [u16; 81] = [
    0, 1, 7, 9, 11, 13, 15, 17, 19, 20, 21, 22, 23, 25, 37, 42, 43, 53, 69, 77, 79, 87, 95, 101,
    102, 103, 104, 109, 110, 111, 113, 115, 117, 119, 123, 135, 137, 139, 143, 161, 179, 389, 427,
    465, 512, 513, 514, 515, 526, 530, 531, 532, 540, 548, 554, 556, 563, 587, 601, 636, 989, 990,
    993, 995, 1719, 1720, 1723, 2049, 3659, 4045, 5060, 5061, 6000, 6566, 6665, 6666, 6667, 6668,
    6669, 6697, 10080,
];

const DEFAULT_PORT: u16 = 80;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LiveUrl {
    pub(crate) scheme: &'static str,
    /// What a connection resolves: no brackets around an IPv6 address.
    pub(crate) host: String,
    pub(crate) port: u16,
    /// The `Host` header.
    pub(crate) authority: String,
    /// The path and query of the request line.
    pub(crate) target: String,
    pub(crate) fragment: Option<String>,
    pub(crate) has_credentials: bool,
    /// A line break or tab was dropped from a URL with a `<`, which `fetch` refuses.
    pub(crate) dangling_markup: bool,
}

impl LiveUrl {
    pub(crate) fn parse(input: &str) -> Option<Self> {
        let is_space = |c: char| c <= ' ';
        let trimmed = input.trim_matches(is_space);
        let text: String = trimmed
            .chars()
            .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
            .collect();
        let dangling_markup = text.len() != trimmed.len() && text.contains('<');
        let (scheme, rest) = text.split_once(':')?;
        let scheme = ["http", "ws"]
            .into_iter()
            .find(|s| scheme.eq_ignore_ascii_case(s))?;
        let rest = rest.trim_start_matches(['/', '\\']);
        let (authority, tail) = rest.split_at(rest.find(['/', '\\', '?', '#']).unwrap_or(rest.len()));
        let (user_info, host_port) = authority.rsplit_once('@').unwrap_or(("", authority));
        let (host, port) = split_port(host_port);
        let host = canonical_host(host)?;
        let port = match port {
            "" => DEFAULT_PORT,
            digits => parse_port(digits)?,
        };
        let authority = match port {
            DEFAULT_PORT => host.clone(),
            port => format!("{host}:{port}"),
        };
        let (target, fragment) = request_target(tail)?;
        let host = match host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
            Some(address) => address.to_owned(),
            None => host,
        };
        Some(Self {
            scheme,
            host,
            port,
            authority,
            target,
            fragment,
            has_credentials: !matches!(user_info, "" | ":"),
            dangling_markup,
        })
    }

    pub(crate) fn port_is_blocked(&self) -> bool {
        BLOCKED_PORTS.binary_search(&self.port).is_ok()
    }

    /// `response.url`: without the fragment.
    pub(crate) fn href(&self) -> String {
        format!("{}://{}{}", self.scheme, self.authority, self.target)
    }
}

fn split_port(host_port: &str) -> (&str, &str) {
    let after_address = match host_port.find(']') {
        Some(end) if host_port.starts_with('[') => end,
        _ => 0,
    };
    match host_port[after_address..].find(':') {
        Some(i) => {
            let i = after_address + i;
            (&host_port[..i], &host_port[i + 1..])
        }
        None => (host_port, ""),
    }
}

fn parse_port(digits: &str) -> Option<u16> {
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    match digits.trim_start_matches('0') {
        "" => Some(0),
        significant => significant.parse().ok(),
    }
}

fn percent_decode(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let hex = |i: usize| {
        let digit = char::from(*bytes.get(i)?).to_digit(16)?;
        u8::try_from(digit).ok()
    };
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        match (b, hex(i + 1), hex(i + 2)) {
            (b'%', Some(high), Some(low)) => {
                out.push(high * 16 + low);
                i += 3;
            }
            _ => {
                out.push(b);
                i += 1;
            }
        }
    }
    out
}

fn canonical_host(host: &str) -> Option<String> {
    if host.is_empty() {
        return None;
    }
    if host.starts_with('[') {
        return canonical_ipv6(&String::from_utf8(percent_decode(host)).ok()?);
    }
    let mut name = canonical_bytes(&percent_decode(host))?;
    if !name.is_ascii() {
        let ascii = Uts46::new()
            .to_ascii(
                &name,
                AsciiDenyList::EMPTY,
                Hyphens::Allow,
                DnsLength::Ignore,
            )
            .ok()?;
        name = canonical_bytes(&percent_decode(&ascii))?;
    }
    let name = String::from_utf8(name).ok().filter(|name| name.is_ascii())?;
    if !ends_in_a_number(&name) {
        return Some(name);
    }
    match Host::parse(&name) {
        Ok(Host::Ipv4(address)) => Some(address.to_string()),
        _ => None,
    }
}

fn canonical_bytes(host: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(host.len());
    for &b in host {
        match b {
            b' ' => out.extend_from_slice(b"%20"),
            b'*' => out.extend_from_slice(b"%2A"),
            b'#' | b'%' | b'/' | b':' | b'<' | b'>' | b'?' | b'@' | b'[' | b'\\' | b']' | b'^'
            | b'|' => return None,
            _ if b.is_ascii_control() => return None,
            _ => out.push(b.to_ascii_lowercase()),
        }
    }
    Some(out)
}

/// The WHATWG "ends in a number" check: such a host is an IPv4 address or invalid.
fn ends_in_a_number(name: &str) -> bool {
    let name = name.strip_suffix('.').unwrap_or(name);
    let last = name.rsplit('.').next().unwrap_or(name);
    match last.strip_prefix("0x") {
        Some(hex) => hex.bytes().all(|b| b.is_ascii_hexdigit()),
        None => !last.is_empty() && last.bytes().all(|b| b.is_ascii_digit()),
    }
}

/// An IPv4 address at the end may use octal and hexadecimal numbers.
fn canonical_ipv6(host: &str) -> Option<String> {
    let mut host = host.to_owned();
    if host.contains('.') {
        let start = host.rfind(':')? + 1;
        let inner = host.strip_suffix(']')?;
        let embedded = &inner[start..];
        if embedded.split('.').count() != 4 || embedded.split('.').any(str::is_empty) {
            return None;
        }
        let Ok(Host::Ipv4(address)) = Host::parse(embedded) else {
            return None;
        };
        host = format!("{}{address}]", &host[..start]);
    }
    match Host::parse(&host) {
        Ok(Host::Ipv6(address)) => Some(Host::<&str>::Ipv6(address).to_string()),
        _ => None,
    }
}

fn request_target(tail: &str) -> Option<(String, Option<String>)> {
    let path_end = tail.find(['?', '#']).unwrap_or(tail.len());
    let path = tail[..path_end].replace('|', "%7C").replace('^', "%5E");
    let url = Url::parse(&format!("http://host{path}{}", &tail[path_end..])).ok()?;
    let target = match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_owned(),
    };
    Some((target, url.fragment().map(str::to_owned)))
}
