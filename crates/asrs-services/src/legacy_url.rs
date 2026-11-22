//! Node's legacy `url` module (`lib/url.js`): `url.parse` without query parsing,
//! `url.format` and `url.resolve`, as got 8 uses them.

use crate::Invalid;
use asrs_js::string::{JsString, decode_uri_component};

#[derive(Clone, Debug, Default)]
pub struct LegacyUrl {
    pub protocol: Option<String>,
    pub slashes: bool,
    pub auth: Option<String>,
    pub host: Option<String>,
    pub port: Option<String>,
    pub hostname: Option<String>,
    pub hash: Option<String>,
    pub search: Option<String>,
    pub query: Option<String>,
    pub pathname: Option<String>,
    pub path: Option<String>,
    pub href: String,
}

fn text(units: &[u16]) -> String {
    String::from_utf16_lossy(units)
}

fn is_slashed(protocol: &str) -> bool {
    matches!(
        protocol,
        "http"
            | "http:"
            | "https"
            | "https:"
            | "ftp"
            | "ftp:"
            | "gopher"
            | "gopher:"
            | "file"
            | "file:"
            | "ws"
            | "ws:"
            | "wss"
            | "wss:"
    )
}

fn is_hostless(protocol: &str) -> bool {
    matches!(protocol, "javascript" | "javascript:")
}

/// `^\/\/[^@/]+@[^@/]+`
fn host_pattern(rest: &[u16]) -> bool {
    if rest.first() != Some(&0x2f) || rest.get(1) != Some(&0x2f) {
        return false;
    }
    let mut i = 2;
    let start = i;
    while i < rest.len() && rest[i] != 0x40 && rest[i] != 0x2f {
        i += 1;
    }
    if i == start || rest.get(i) != Some(&0x40) {
        return false;
    }
    i + 1 < rest.len() && rest[i + 1..].iter().all(|&u| u != 0x40 && u != 0x2f)
}

const AUTO_ESCAPE: &[(u16, &str)] = &[
    (0x09, "%09"),
    (0x0a, "%0A"),
    (0x0d, "%0D"),
    (0x20, "%20"),
    (0x22, "%22"),
    (0x27, "%27"),
    (0x3c, "%3C"),
    (0x3e, "%3E"),
    (0x5c, "%5C"),
    (0x5e, "%5E"),
    (0x60, "%60"),
    (0x7b, "%7B"),
    (0x7c, "%7C"),
    (0x7d, "%7D"),
];

fn auto_escape(rest: &mut Vec<u16>) {
    if !rest.iter().any(|u| AUTO_ESCAPE.iter().any(|(c, _)| c == u)) {
        return;
    }
    let mut out = Vec::with_capacity(rest.len());
    for &u in rest.iter() {
        match AUTO_ESCAPE.iter().find(|(c, _)| *c == u) {
            Some((_, esc)) => out.extend(esc.encode_utf16()),
            None => out.push(u),
        }
    }
    *rest = out;
}

fn forbidden_host(hostname: &str, ipv6: bool) -> bool {
    hostname.chars().any(|c| {
        if ipv6 {
            matches!(
                c,
                '\0' | '\t'
                    | '\n'
                    | '\r'
                    | ' '
                    | '#'
                    | '%'
                    | '/'
                    | '<'
                    | '>'
                    | '?'
                    | '@'
                    | '\\'
                    | '^'
                    | '|'
            )
        } else {
            matches!(
                c,
                '\0' | '\t'
                    | '\n'
                    | '\r'
                    | ' '
                    | '#'
                    | '%'
                    | '/'
                    | ':'
                    | '<'
                    | '>'
                    | '?'
                    | '@'
                    | '['
                    | '\\'
                    | ']'
                    | '^'
                    | '|'
            )
        }
    })
}

fn parse_host(host: &str) -> (String, Option<String>) {
    for (i, c) in host.char_indices() {
        if c == ':' && host[i + 1..].chars().all(|d| d.is_ascii_digit()) {
            return (
                host[..i].to_owned(),
                (i + 1 != host.len()).then(|| host[i + 1..].to_owned()),
            );
        }
    }
    (host.to_owned(), None)
}

/// JavaScript's `\s`.
fn is_js_space(u: u16) -> bool {
    matches!(
        u,
        0x09..=0x0d | 0x20 | 0xa0 | 0x1680 | 0x2000..=0x200a | 0x2028 | 0x2029 | 0x202f | 0x205f
            | 0x3000 | 0xfeff
    )
}

/// `simplePathPattern` (`/^(\/\/?(?!\/)[^?\s]*)(\?[^\s]*)?$/`): the index of the `?` that
/// starts the search, or the length when there is none.
fn simple_path(rest: &[u16]) -> Option<usize> {
    if rest.first() != Some(&0x2f) || rest.iter().any(|&u| is_js_space(u)) {
        return None;
    }
    let slashes = if rest.get(1) == Some(&0x2f) { 2 } else { 1 };
    if rest.get(slashes) == Some(&0x2f) {
        return None;
    }
    Some(rest.iter().position(|&u| u == 0x3f).unwrap_or(rest.len()))
}

/// `url.parse(url)` / `url.parse(url, false, slashesDenoteHost)`.
pub fn parse(input: &str, slashes_denote_host: bool) -> Result<LegacyUrl, Invalid> {
    let url: Vec<u16> = input.encode_utf16().collect();
    let mut rest: Vec<u16> = Vec::new();
    let mut start: Option<usize> = None;
    let mut end: Option<usize> = None;
    let mut in_ws = false;
    let mut split = false;
    let mut has_hash = false;
    let mut has_at = false;
    let mut last_pos = 0usize;
    for (i, &code) in url.iter().enumerate() {
        let is_ws = code < 33 || code == 0xa0 || code == 0xfeff;
        match start {
            None => {
                if is_ws {
                    continue;
                }
                start = Some(i);
                last_pos = i;
            }
            Some(_) if in_ws => {
                if !is_ws {
                    end = None;
                    in_ws = false;
                }
            }
            Some(_) if is_ws => {
                end = Some(i);
                in_ws = true;
            }
            _ => {}
        }
        if !split {
            match code {
                0x40 => has_at = true,
                0x23 => {
                    has_hash = true;
                    split = true;
                }
                0x3f => split = true,
                0x5c => {
                    if i > last_pos {
                        rest.extend_from_slice(&url[last_pos..i]);
                    }
                    rest.push(0x2f);
                    last_pos = i + 1;
                }
                _ => {}
            }
        } else if code == 0x23 {
            has_hash = true;
        }
    }
    if let Some(start) = start {
        if last_pos == start {
            match end {
                None => rest = url[start..].to_vec(),
                Some(end) => rest = url[start..end].to_vec(),
            }
        } else if end.is_none() && last_pos < url.len() {
            rest.extend_from_slice(&url[last_pos..]);
        } else if let Some(end) = end.filter(|&e| last_pos < e) {
            rest.extend_from_slice(&url[last_pos..end]);
        }
    }

    let mut out = LegacyUrl::default();
    if !slashes_denote_host
        && !has_hash
        && !has_at
        && let Some(question) = simple_path(&rest)
    {
        out.path = Some(text(&rest));
        out.href = text(&rest);
        out.pathname = Some(text(&rest[..question]));
        if question < rest.len() {
            out.search = Some(text(&rest[question..]));
            out.query = Some(text(&rest[question + 1..]));
        }
        return Ok(out);
    }
    let proto_len = rest
        .iter()
        .take_while(|&&u| {
            u < 0x80 && (u as u8 as char).is_ascii_alphanumeric()
                || u == u16::from(b'.')
                || u == u16::from(b'+')
                || u == u16::from(b'-')
        })
        .count();
    let mut proto: Option<String> = None;
    let mut lower_proto = String::new();
    if proto_len > 0 && rest.get(proto_len) == Some(&u16::from(b':')) {
        proto = Some(text(&rest[..proto_len + 1]));
        lower_proto = JsString::from(proto.as_deref().unwrap_or_default())
            .to_lower_case()
            .to_string_lossy();
        out.protocol = Some(lower_proto.clone());
        rest.drain(..proto_len + 1);
    }

    let mut slashes = false;
    if slashes_denote_host || proto.is_some() || host_pattern(&rest) {
        slashes = rest.first() == Some(&0x2f) && rest.get(1) == Some(&0x2f);
        if slashes && !(proto.is_some() && is_hostless(&lower_proto)) {
            rest.drain(..2);
            out.slashes = true;
        }
    }

    if !is_hostless(&lower_proto)
        && (slashes || (proto.is_some() && !is_slashed(proto.as_deref().unwrap_or_default())))
    {
        let mut host_end: Option<usize> = None;
        let mut at_sign: Option<usize> = None;
        let mut non_host: Option<usize> = None;
        let mut i = 0;
        while i < rest.len() {
            match rest[i] {
                0x09 | 0x0a | 0x0d => {
                    rest.remove(i);
                    continue;
                }
                0x20 | 0x22 | 0x25 | 0x27 | 0x3b | 0x3c | 0x3e | 0x5c | 0x5e | 0x60 | 0x7b
                | 0x7c | 0x7d => {
                    if non_host.is_none() {
                        non_host = Some(i);
                    }
                }
                0x23 | 0x2f | 0x3f => {
                    if non_host.is_none() {
                        non_host = Some(i);
                    }
                    host_end = Some(i);
                }
                0x40 => {
                    at_sign = Some(i);
                    non_host = None;
                }
                _ => {}
            }
            if host_end.is_some() {
                break;
            }
            i += 1;
        }
        let mut start2 = 0;
        if let Some(at) = at_sign {
            let auth = decode_uri_component(&JsString::from_units(rest[..at].to_vec()))
                .map_err(|_| Invalid)?
                .to_string_lossy();
            out.auth = Some(auth);
            start2 = at + 1;
        }
        let host = match non_host {
            None => {
                let h = text(&rest[start2..]);
                rest.clear();
                h
            }
            Some(nh) => {
                let h = text(&rest[start2..nh]);
                rest.drain(..nh);
                h
            }
        };
        let (hostname, port) = parse_host(&host);
        out.port = port;
        if !hostname.is_empty() {
            out.hostname = Some(hostname);
        }
        if out.hostname.is_none() {
            out.hostname = Some(String::new());
        }
        let ipv6 = out
            .hostname
            .as_deref()
            .is_some_and(|h| h.starts_with('[') && h.ends_with(']'));
        if !ipv6 {
            let hostname = out.hostname.take().unwrap_or_default();
            match hostname
                .char_indices()
                .find(|(_, c)| matches!(c, '/' | '\\' | '#' | '?' | ':'))
            {
                Some((j, _)) => {
                    let mut fixed: Vec<u16> = vec![0x2f];
                    fixed.extend(hostname[j..].encode_utf16());
                    fixed.extend_from_slice(&rest);
                    rest = fixed;
                    out.hostname = Some(hostname[..j].to_owned());
                }
                None => out.hostname = Some(hostname),
            }
        }
        let mut hostname = out.hostname.take().unwrap_or_default();
        if hostname.encode_utf16().count() > 255 {
            hostname = String::new();
        } else {
            hostname = JsString::from(hostname.as_str())
                .to_lower_case()
                .to_string_lossy();
        }
        if !hostname.is_empty() {
            if ipv6 {
                if forbidden_host(&hostname, true) {
                    return Err(Invalid);
                }
            } else {
                let ascii = to_ascii(&hostname);
                if ascii.is_empty() || forbidden_host(&ascii, false) {
                    return Err(Invalid);
                }
                hostname = ascii;
            }
        }
        out.host = Some(match &out.port {
            Some(port) => format!("{hostname}:{port}"),
            None => hostname.clone(),
        });
        if ipv6 {
            out.hostname = Some(hostname[1..hostname.len() - 1].to_owned());
            if rest.first() != Some(&0x2f) {
                rest.insert(0, 0x2f);
            }
        } else {
            out.hostname = Some(hostname);
        }
    }

    if !is_hostless(&lower_proto) {
        auto_escape(&mut rest);
    }

    let mut question: Option<usize> = None;
    let mut hash: Option<usize> = None;
    for (i, &code) in rest.iter().enumerate() {
        if code == 0x23 {
            out.hash = Some(text(&rest[i..]));
            hash = Some(i);
            break;
        } else if code == 0x3f && question.is_none() {
            question = Some(i);
        }
    }
    if let Some(q) = question {
        match hash {
            None => {
                out.search = Some(text(&rest[q..]));
                out.query = Some(text(&rest[q + 1..]));
            }
            Some(h) => {
                out.search = Some(text(&rest[q..h]));
                out.query = Some(text(&rest[q + 1..h]));
            }
        }
    }
    let use_question = question.is_some() && (hash.is_none() || question < hash);
    let first = if use_question { question } else { hash };
    match first {
        None => {
            if !rest.is_empty() {
                out.pathname = Some(text(&rest));
            }
        }
        Some(0) => {}
        Some(first) => out.pathname = Some(text(&rest[..first])),
    }
    if is_slashed(&lower_proto)
        && out.hostname.as_deref().is_some_and(|h| !h.is_empty())
        && out.pathname.is_none()
    {
        out.pathname = Some("/".to_owned());
    }
    if out.pathname.is_some() || out.search.is_some() {
        let p = out.pathname.as_deref().unwrap_or_default();
        let s = out.search.as_deref().unwrap_or_default();
        out.path = Some(format!("{p}{s}"));
    }
    out.href = format(&out);
    Ok(out)
}

/// `internal/idna`'s `toASCII` (ada's UTS 46 processing, nontransitional, no STD3 rules or
/// DNS length check); `""` where ada fails.
fn to_ascii(hostname: &str) -> String {
    idna::domain_to_ascii(hostname).unwrap_or_default()
}

fn encode_auth(auth: &str) -> String {
    fn allowed(u: u16) -> bool {
        matches!(u,
            0x21 | 0x27..=0x2a | 0x2d | 0x2e | 0x30..=0x3a | 0x41..=0x5a | 0x5f | 0x61..=0x7a
                | 0x7e)
    }
    let mut out = String::new();
    for c in auth.chars() {
        let u = c as u32;
        if u < 0x80 && allowed(u as u16) {
            out.push(c);
        } else {
            let mut buf = [0u8; 4];
            for b in c.encode_utf8(&mut buf).as_bytes() {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

/// `Url.prototype.format`.
pub fn format(url: &LegacyUrl) -> String {
    let mut auth = url.auth.clone().unwrap_or_default();
    if !auth.is_empty() {
        auth = format!("{}@", encode_auth(&auth));
    }
    let mut protocol = url.protocol.clone().unwrap_or_default();
    if !protocol.is_empty() && !protocol.ends_with(':') {
        protocol.push(':');
    }
    let mut pathname = url.pathname.clone().unwrap_or_default();
    let mut hash = url.hash.clone().unwrap_or_default();
    let mut host = String::new();
    if let Some(h) = url.host.as_ref().filter(|h| !h.is_empty()) {
        host = format!("{auth}{h}");
    } else if let Some(h) = url.hostname.as_ref().filter(|h| !h.is_empty()) {
        let h = if h.contains(':') && !(h.starts_with('[') && h.ends_with(']')) {
            format!("[{h}]")
        } else {
            h.clone()
        };
        host = format!("{auth}{h}");
        if let Some(port) = url.port.as_ref().filter(|p| !p.is_empty()) {
            host.push(':');
            host.push_str(port);
        }
    }
    let mut search = url.search.clone().unwrap_or_default();
    if pathname.contains('#') || pathname.contains('?') {
        pathname = pathname
            .chars()
            .map(|c| match c {
                '#' => "%23".to_owned(),
                '?' => "%3F".to_owned(),
                c => String::from(c),
            })
            .collect();
    }
    if url.slashes || is_slashed(&protocol) {
        if url.slashes || !host.is_empty() {
            if !pathname.is_empty() && !pathname.starts_with('/') {
                pathname = format!("/{pathname}");
            }
            host = format!("//{host}");
        } else if protocol.starts_with("file") {
            host = "//".to_owned();
        }
    }
    if search.contains('#') {
        search = search.replace('#', "%23");
    }
    if !hash.is_empty() && !hash.starts_with('#') {
        hash = format!("#{hash}");
    }
    if !search.is_empty() && !search.starts_with('?') {
        search = format!("?{search}");
    }
    format!("{protocol}{host}{pathname}{search}{hash}")
}

/// `url.resolve(source, relative)`.
pub fn resolve(source: &str, relative: &str) -> Result<String, Invalid> {
    let source = parse(source, true)?;
    let relative = parse(relative, true)?;
    Ok(format(&resolve_object(&source, relative)))
}

fn truthy(value: &Option<String>) -> bool {
    value.as_deref().is_some_and(|v| !v.is_empty())
}

fn join_path(path: &[Option<String>]) -> String {
    path.iter()
        .map(|p| p.as_deref().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("/")
}

fn split_path(pathname: &Option<String>) -> Vec<Option<String>> {
    match pathname.as_deref() {
        Some(p) if !p.is_empty() => p.split('/').map(|s| Some(s.to_owned())).collect(),
        _ => Vec::new(),
    }
}

fn set_path(result: &mut LegacyUrl) {
    if result.pathname.is_some() || result.search.is_some() {
        result.path = Some(format!(
            "{}{}",
            result.pathname.as_deref().unwrap_or_default(),
            result.search.as_deref().unwrap_or_default()
        ));
    }
}

fn split_auth_in_host(result: &mut LegacyUrl) {
    let Some(host) = result
        .host
        .clone()
        .filter(|h| h.find('@').is_some_and(|i| i > 0))
    else {
        return;
    };
    let mut parts = host.split('@');
    result.auth = parts.next().map(str::to_owned);
    result.host = parts.next().map(str::to_owned);
    result.hostname = result.host.clone();
}

/// `Url.prototype.resolveObject` (node 22).
fn resolve_object(source: &LegacyUrl, mut relative: LegacyUrl) -> LegacyUrl {
    let mut result = source.clone();
    result.hash = relative.hash.clone();
    if relative.href.is_empty() {
        result.href = format(&result);
        return result;
    }
    if relative.slashes && relative.protocol.is_none() {
        let protocol = result.protocol.take();
        result = relative;
        result.protocol = protocol;
        if is_slashed(result.protocol.as_deref().unwrap_or_default())
            && truthy(&result.hostname)
            && !truthy(&result.pathname)
        {
            result.pathname = Some("/".to_owned());
            result.path = Some("/".to_owned());
        }
        result.href = format(&result);
        return result;
    }
    if truthy(&relative.protocol) && relative.protocol != result.protocol {
        let protocol = relative.protocol.clone().unwrap_or_default();
        if !is_slashed(&protocol) {
            result = relative;
            result.href = format(&result);
            return result;
        }
        result.protocol = relative.protocol.clone();
        if !truthy(&relative.host)
            && protocol != "file"
            && protocol != "file:"
            && !is_hostless(&protocol)
        {
            let mut rel_path: Vec<String> = relative
                .pathname
                .as_deref()
                .unwrap_or_default()
                .split('/')
                .map(str::to_owned)
                .collect();
            rel_path.reverse();
            let mut host = String::new();
            while let Some(segment) = rel_path.pop() {
                if !segment.is_empty() {
                    host = segment;
                    break;
                }
            }
            rel_path.reverse();
            relative.host = Some(host);
            if !truthy(&relative.hostname) {
                relative.hostname = Some(String::new());
            }
            if rel_path.first().is_none_or(|p| !p.is_empty()) {
                rel_path.insert(0, String::new());
            }
            if rel_path.len() < 2 {
                rel_path.insert(0, String::new());
            }
            result.pathname = Some(rel_path.join("/"));
        } else {
            result.pathname = relative.pathname.clone();
        }
        result.search = relative.search.clone();
        result.query = relative.query.clone();
        result.host = Some(relative.host.clone().unwrap_or_default());
        result.auth = relative.auth.clone();
        result.hostname = if truthy(&relative.hostname) {
            relative.hostname.clone()
        } else {
            relative.host.clone()
        };
        result.port = relative.port.clone();
        if truthy(&result.pathname) || truthy(&result.search) {
            result.path = Some(format!(
                "{}{}",
                result.pathname.as_deref().unwrap_or_default(),
                result.search.as_deref().unwrap_or_default()
            ));
        }
        result.slashes = result.slashes || relative.slashes;
        result.href = format(&result);
        return result;
    }

    let is_source_abs = result
        .pathname
        .as_deref()
        .is_some_and(|p| p.starts_with('/'));
    let is_rel_abs = truthy(&relative.host)
        || relative
            .pathname
            .as_deref()
            .is_some_and(|p| p.starts_with('/'));
    let mut must_end_abs =
        is_rel_abs || is_source_abs || (truthy(&result.host) && truthy(&relative.pathname));
    let remove_all_dots = must_end_abs;
    let mut src_path = split_path(&result.pathname);
    let mut rel_path = split_path(&relative.pathname);
    let no_leading_slashes =
        truthy(&result.protocol) && !is_slashed(result.protocol.as_deref().unwrap_or_default());
    if no_leading_slashes {
        result.hostname = Some(String::new());
        result.port = None;
        if truthy(&result.host) {
            if src_path.first() == Some(&Some(String::new())) {
                src_path[0] = result.host.clone();
            } else {
                src_path.insert(0, result.host.clone());
            }
        }
        result.host = Some(String::new());
        if truthy(&relative.protocol) {
            relative.hostname = None;
            relative.port = None;
            result.auth = None;
            if truthy(&relative.host) {
                if rel_path.first() == Some(&Some(String::new())) {
                    rel_path[0] = relative.host.clone();
                } else {
                    rel_path.insert(0, relative.host.clone());
                }
            }
            relative.host = None;
        }
        let empty = Some(Some(String::new()));
        must_end_abs = must_end_abs
            && (rel_path.first().cloned() == empty || src_path.first().cloned() == empty);
    }

    if is_rel_abs {
        if relative.host.is_some() {
            if result.host != relative.host {
                result.auth = None;
            }
            result.host = relative.host.clone();
            result.port = relative.port.clone();
        }
        if relative.hostname.is_some() {
            if result.hostname != relative.hostname {
                result.auth = None;
            }
            result.hostname = relative.hostname.clone();
        }
        result.search = relative.search.clone();
        result.query = relative.query.clone();
        src_path = rel_path;
    } else if !rel_path.is_empty() {
        src_path.pop();
        src_path.extend(rel_path);
        result.search = relative.search.clone();
        result.query = relative.query.clone();
    } else if relative.search.is_some() {
        if no_leading_slashes {
            let first = if src_path.is_empty() {
                None
            } else {
                src_path.remove(0)
            };
            result.host = first.clone();
            result.hostname = first;
            split_auth_in_host(&mut result);
        }
        result.search = relative.search.clone();
        result.query = relative.query.clone();
        set_path(&mut result);
        result.href = format(&result);
        return result;
    }

    if src_path.is_empty() {
        result.pathname = None;
        result.path = if truthy(&result.search) {
            Some(format!("/{}", result.search.as_deref().unwrap_or_default()))
        } else {
            None
        };
        result.href = format(&result);
        return result;
    }

    let segment = |p: &Option<String>| p.as_deref().unwrap_or_default().to_owned();
    let last = src_path.last().map(segment).unwrap_or_default();
    let has_trailing_slash =
        ((truthy(&result.host) || truthy(&relative.host) || src_path.len() > 1)
            && (last == "." || last == ".."))
            || last.is_empty();
    let mut up = 0usize;
    let mut i = src_path.len();
    while i > 0 {
        i -= 1;
        let last = segment(&src_path[i]);
        if last == "." {
            src_path.remove(i);
        } else if last == ".." {
            src_path.remove(i);
            up += 1;
        } else if up > 0 {
            src_path.remove(i);
            up -= 1;
        }
    }
    if !must_end_abs && !remove_all_dots {
        for _ in 0..up {
            src_path.insert(0, Some("..".to_owned()));
        }
    }
    let first_is = |path: &[Option<String>], f: &dyn Fn(&str) -> bool| {
        path.first().and_then(|p| p.as_deref()).is_some_and(f)
    };
    if must_end_abs
        && !first_is(&src_path, &|p| p.is_empty())
        && !first_is(&src_path, &|p| p.starts_with('/'))
    {
        src_path.insert(0, Some(String::new()));
    }
    if has_trailing_slash && !join_path(&src_path).ends_with('/') {
        src_path.push(Some(String::new()));
    }
    let is_absolute =
        first_is(&src_path, &|p| p.is_empty()) || first_is(&src_path, &|p| p.starts_with('/'));
    if no_leading_slashes {
        let host = if is_absolute || src_path.is_empty() {
            Some(String::new())
        } else {
            src_path.remove(0)
        };
        result.host = host.clone();
        result.hostname = host;
        split_auth_in_host(&mut result);
    }
    must_end_abs = must_end_abs || (truthy(&result.host) && !src_path.is_empty());
    if must_end_abs && !is_absolute {
        src_path.insert(0, Some(String::new()));
    }
    if src_path.is_empty() {
        result.pathname = None;
        result.path = None;
    } else {
        result.pathname = Some(join_path(&src_path));
    }
    set_path(&mut result);
    if truthy(&relative.auth) {
        result.auth = relative.auth.clone();
    }
    result.slashes = result.slashes || relative.slashes;
    result.href = format(&result);
    result
}
