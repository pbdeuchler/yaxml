//! URI parsing, serialization, escaping and reference resolution following
//! libxml2 2.15.4 `uri.c` on Unix, including its deviations from RFC 3986
//! (see `docs/uri.md`).
//!
//! Inputs end at the first NUL, as C strings do. Unescaped components must be
//! UTF-8 (`InvalidUtf8` otherwise) and are cut at a decoded NUL.

use std::fmt::{self, Write};

use memchr::memchr;

/// A parsed URI reference. Components hold unescaped text (raw text for
/// [`Uri::parse_raw`]) except `query`, which is always as written.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Uri {
    pub scheme: Option<String>,
    pub user: Option<String>,
    pub host: Option<String>,
    /// Never zero: libxml2 cannot tell port 0 from no port.
    pub port: Option<u32>,
    /// `scheme://` followed by neither host nor port (libxml2's port -1).
    pub empty_authority: bool,
    pub path: Option<String>,
    pub query: Option<String>,
    pub fragment: Option<String>,
}

/// URI failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum UriError {
    /// Invalid syntax; the offset is the furthest byte either grammar reached.
    Syntax { offset: usize },
    /// Percent-decoding produced bytes that are not UTF-8.
    InvalidUtf8,
}

impl fmt::Display for UriError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax { offset } => write!(f, "invalid URI syntax at byte {offset}"),
            Self::InvalidUtf8 => f.write_str("percent-decoded URI text is not UTF-8"),
        }
    }
}

impl std::error::Error for UriError {}

/// Components as slices of the input, before unescaping.
#[derive(Default)]
struct Parts<'a> {
    scheme: Option<&'a str>,
    user: Option<&'a str>,
    host: Option<&'a str>,
    port: Option<u32>,
    empty: bool,
    path: Option<&'a str>,
    query: Option<&'a str>,
    fragment: Option<&'a str>,
}

fn cstr(s: &str) -> &str {
    memchr(0, s.as_bytes()).map_or(s, |end| &s[..end])
}

fn at(s: &[u8], i: usize) -> u8 {
    s.get(i).copied().unwrap_or(0)
}

fn unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~')
}

fn sub_delim(b: u8) -> bool {
    matches!(
        b,
        b'!' | b'$' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+' | b',' | b';' | b'='
    )
}

/// libxml2's RFC 2396 unreserved set, used by escaping and serialization.
fn unreserved_2396(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b)
}

fn pct(s: &[u8], i: usize) -> bool {
    at(s, i) == b'%' && at(s, i + 1).is_ascii_hexdigit() && at(s, i + 2).is_ascii_hexdigit()
}

fn pchar(b: u8) -> bool {
    unreserved(b) || sub_delim(b) || matches!(b, b':' | b'@')
}

/// Skips bytes accepted by `ok` and well-formed `%XX` escapes.
fn skip(s: &[u8], mut i: usize, ok: impl Fn(u8) -> bool) -> usize {
    while pct(s, i) || ok(at(s, i)) {
        i += if at(s, i) == b'%' { 3 } else { 1 };
    }
    i
}

/// `segment`, or `segment-nz` when `empty` is false, optionally forbidding a byte.
fn segment(s: &[u8], i: usize, forbid: u8, empty: bool) -> Result<usize, usize> {
    match skip(s, i, |b| pchar(b) && b != forbid) {
        end if end == i && !empty => Err(i),
        end => Ok(end),
    }
}

/// `*( "/" segment )`
fn slashes(s: &[u8], mut i: usize) -> usize {
    while at(s, i) == b'/' {
        i = skip(s, i + 1, pchar);
    }
    i
}

/// `[ userinfo "@" ] host [ ":" port ]` starting at `i`.
fn authority(src: &str, i: usize, decode: bool) -> Result<(Parts<'_>, usize), usize> {
    let s = src.as_bytes();
    let info = skip(s, i, |b| unreserved(b) || sub_delim(b) || b == b':');
    let (user, i) = match at(s, info) {
        // libxml2 unescapes an empty userinfo with length 0, which means "the
        // whole remaining string".
        b'@' if info == i && decode => (Some(&src[i..]), info + 1),
        b'@' => (Some(&src[i..info]), info + 1),
        _ => (None, i),
    };
    let end = if at(s, i) == b'[' {
        memchr(b']', &s[i..]).map(|k| i + k + 1).ok_or(s.len())?
    } else {
        // libxml2's IPv4 branch can never match (a missing `cur++`), so digits are a reg-name.
        skip(s, i, |b| unreserved(b) || sub_delim(b))
    };
    let host = (end > i).then(|| &src[i..end]);
    if at(s, end) != b':' {
        return Ok((
            Parts {
                user,
                host,
                ..Parts::default()
            },
            end,
        ));
    }
    let digits = s[end + 1..]
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .count();
    let port = s[end + 1..end + 1 + digits]
        .iter()
        .try_fold(0u32, |n, &b| {
            n.checked_mul(10)?.checked_add(u32::from(b - b'0'))
        })
        .filter(|&n| digits > 0 && n <= i32::MAX.cast_unsigned())
        .ok_or(end + 1)?;
    let port = (port > 0).then_some(port);
    Ok((
        Parts {
            user,
            host,
            port,
            ..Parts::default()
        },
        end + 1 + digits,
    ))
}

/// Splits `src` as `URI` (`absolute`) or `relative-ref`.
fn split(src: &str, absolute: bool, decode: bool) -> Result<Parts<'_>, usize> {
    let s = src.as_bytes();
    let (scheme, i) = if absolute {
        let end = match at(s, 0).is_ascii_alphabetic() {
            true => {
                let rest = |b: &&u8| b.is_ascii_alphanumeric() || b"+-.".contains(b);
                1 + s[1..].iter().take_while(rest).count()
            }
            false => return Err(0),
        };
        if at(s, end) != b':' {
            return Err(end);
        }
        (Some(&src[..end]), end + 1)
    } else {
        (None, 0)
    };
    let (head, start, path_end) = if at(s, i) == b'/' && at(s, i + 1) == b'/' {
        let (head, i) = authority(src, i + 2, decode)?;
        let empty = absolute && head.host.is_none() && head.port.is_none();
        (Parts { empty, ..head }, i, slashes(s, i))
    } else if at(s, i) == b'/' {
        let end = segment(s, i + 1, 0, false).map_or(i + 1, |e| slashes(s, e));
        (Parts::default(), i, end)
    } else if pct(s, i) || pchar(at(s, i)) {
        let forbid = if absolute { 0 } else { b':' };
        (
            Parts::default(),
            i,
            slashes(s, segment(s, i, forbid, false)?),
        )
    } else {
        (Parts::default(), i, i)
    };
    let (query, q_end) = match at(s, path_end) {
        b'?' => {
            let stop = skip(s, path_end + 1, |b| pchar(b) || b == b'/' || b == b'?');
            (Some(&src[path_end + 1..stop]), stop)
        }
        _ => (None, path_end),
    };
    let (fragment, stop) = match at(s, q_end) {
        b'#' => {
            let stop = skip(s, q_end + 1, |b| pchar(b) || b"/?[]".contains(&b));
            (Some(&src[q_end + 1..stop]), stop)
        }
        _ => (None, q_end),
    };
    let path = (path_end > start).then(|| &src[start..path_end]);
    match stop == s.len() {
        true => Ok(Parts {
            scheme,
            path,
            query,
            fragment,
            ..head
        }),
        false => Err(stop),
    }
}

fn parse_with(src: &str, decode: bool) -> Result<Uri, UriError> {
    let src = cstr(src);
    let parts = split(src, true, decode)
        .or_else(|first| split(src, false, decode).map_err(|second| first.max(second)))
        .map_err(|offset| UriError::Syntax { offset })?;
    let text = |s: &str| {
        if decode {
            unescape(s)
        } else {
            Ok(s.to_owned())
        }
    };
    let owned = |s: Option<&str>| s.map(text).transpose();
    Ok(Uri {
        scheme: parts.scheme.map(str::to_owned),
        user: owned(parts.user)?,
        host: owned(parts.host)?,
        port: parts.port,
        empty_authority: parts.empty,
        path: owned(parts.path)?,
        query: parts.query.map(str::to_owned),
        fragment: owned(parts.fragment)?,
    })
}

impl Uri {
    /// `xmlParseURI`: an RFC 3986 URI or relative reference, percent-decoded.
    pub fn parse(src: &str) -> Result<Self, UriError> {
        parse_with(src, true)
    }

    /// `xmlParseURIRaw(src, 1)`: like [`Uri::parse`] but keeps escapes as written.
    pub fn parse_raw(src: &str) -> Result<Self, UriError> {
        parse_with(src, false)
    }

    /// Whether serialization emits `//`.
    pub fn has_authority(&self) -> bool {
        self.host.is_some() || self.port.is_some() || self.empty_authority
    }
}

/// Writes `s`, percent-encoding (uppercase) every byte `keep` rejects.
fn put(w: &mut impl Write, s: &str, keep: impl Fn(u8) -> bool) -> fmt::Result {
    let mut run = 0;
    for (i, b) in s.bytes().enumerate() {
        if !keep(b) {
            if run < i {
                w.write_str(&s[run..i])?;
            }
            write!(w, "%{b:02X}")?;
            run = i + 1;
        }
    }
    w.write_str(&s[run..])
}

/// `xmlSaveUri`.
impl fmt::Display for Uri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let keep = |extra: &'static [u8]| move |b: u8| unreserved_2396(b) || extra.contains(&b);
        if let Some(scheme) = &self.scheme {
            write!(f, "{scheme}:")?;
        }
        if self.has_authority() {
            f.write_str("//")?;
            if let Some(user) = &self.user {
                put(f, user, keep(b";:&=+$,"))?;
                f.write_char('@')?;
            }
            f.write_str(self.host.as_deref().unwrap_or(""))?;
            if let Some(port) = self.port {
                write!(f, ":{port}")?;
            }
        }
        if let Some(path) = &self.path {
            // Keep the colon of `file:///d:` unescaped.
            let drive = self.scheme.as_deref() == Some("file")
                && matches!(path.as_bytes(), [b'/', d, b':', ..] if d.is_ascii_alphabetic());
            let (head, rest) = path.split_at(if drive { 3 } else { 0 });
            f.write_str(head)?;
            put(f, rest, keep(b"/;@&=+$,"))?;
        }
        if let Some(query) = &self.query {
            write!(f, "?{query}")?;
        }
        if let Some(fragment) = &self.fragment {
            f.write_char('#')?;
            put(f, fragment, keep(b";/?:@&=+$,[]"))?;
        }
        Ok(())
    }
}

/// `xmlURIEscapeStr`: percent-encodes every byte except ASCII alphanumerics,
/// `-_.!~*'()`, `@` and the ASCII bytes of `allowed_extra`.
pub fn escape(s: &str, allowed_extra: &str) -> String {
    let s = cstr(s);
    let mut out = String::with_capacity(s.len());
    let keep = |b: u8| {
        b == b'@' || unreserved_2396(b) || (b.is_ascii() && allowed_extra.as_bytes().contains(&b))
    };
    // Writing to a String cannot fail.
    let _ = put(&mut out, s, keep);
    out
}

/// `xmlURIUnescapeString`: decodes well-formed `%XX` escapes, cutting the
/// result at a decoded NUL.
pub fn unescape(s: &str) -> Result<String, UriError> {
    let b = cstr(s).as_bytes();
    let hex = |i| b.get(i).and_then(|&d| char::from(d).to_digit(16));
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match (b[i], hex(i + 1), hex(i + 2)) {
            (b'%', Some(hi), Some(lo)) => {
                out.push((hi << 4 | lo) as u8);
                i += 3;
            }
            (c, ..) => {
                out.push(c);
                i += 1;
            }
        }
    }
    out.truncate(memchr(0, &out).unwrap_or(out.len()));
    String::from_utf8(out).map_err(|_| UriError::InvalidUtf8)
}

/// `xmlNormalizePath`, replaying libxml2's in-place rewrite exactly (including
/// its reads of stale output bytes, which keep `/..` at the root).
fn normalize(path: &str, is_file: bool) -> String {
    let mut buf = cstr(path).as_bytes().to_vec();
    if buf.is_empty() {
        return String::new();
    }
    buf.push(0);
    let (mut cur, mut out, mut segments) = (0, 0, 0i32);
    if buf[0] == b'/' {
        (cur, out) = (1, 1);
    }
    while buf[cur] != 0 {
        while buf[cur] == b'/' {
            cur += 1;
        }
        if buf[cur] == b'.' {
            let (next, last) = (buf[cur + 1], buf.get(cur + 2).copied().unwrap_or(0));
            if next == 0 {
                break;
            } else if next == b'/' {
                cur += 2;
                continue;
            } else if next == b'.' && (last == 0 || last == b'/') {
                if segments > 0 || buf[out] == b'/' {
                    if segments > 0 {
                        out = out.saturating_sub(1);
                        while out > 0 && buf[out - 1] != b'/' {
                            out -= 1;
                        }
                        segments -= 1;
                    }
                    if last == 0 {
                        break;
                    }
                    cur += 3;
                    continue;
                }
                // Keep "../" at the start of a relative path.
                segments -= 1;
            }
        }
        while buf[cur] != 0 && buf[cur] != b'/' {
            buf[out] = buf[cur];
            (out, cur) = (out + 1, cur + 1);
        }
        if buf[cur] != 0 {
            buf[out] = b'/';
            (out, cur) = (out + 1, cur + 1);
        }
        segments += 1;
    }
    if is_file && out == 0 {
        buf[0] = b'.';
        out = 1;
    }
    buf.truncate(out);
    // Only whole segments and separators are copied, so this is valid UTF-8.
    String::from_utf8(buf).unwrap_or_default()
}

/// `xmlNormalizeURIPath`: removes `.` and resolvable `..` segments and
/// repeated slashes. Unlike RFC 3986, `/..` at the root is kept.
pub fn normalize_path(path: &str) -> String {
    normalize(path, false)
}

/// `xmlPathToURI` / `xmlCanonicPath`: escapes a `scheme://` string's characters
/// outside the URI set; plain filesystem paths are returned unchanged.
pub fn canonic_path(path: &str) -> String {
    match path.contains("://") {
        true => escape(path, ":/?#[]@!$&()*+,;='%"),
        false => path.to_owned(),
    }
}

/// `xmlResolvePath`: `reference` as a filesystem path relative to `base`.
fn resolve_path(reference: &str, base: &str) -> Result<String, UriError> {
    if reference.is_empty() {
        return match base.is_empty() {
            true => Err(UriError::Syntax { offset: 0 }),
            false => Ok(base.to_owned()),
        };
    }
    let (path, fragment) =
        reference.split_at(memchr(b'#', reference.as_bytes()).unwrap_or(reference.len()));
    let path = unescape(path)?;
    let dir = base.rfind('/').map_or(0, |i| i + 1);
    let mut result = if base.is_empty() || path.starts_with('/') || dir == 0 {
        path
    } else {
        normalize(&[&base[..dir], &path].concat(), true)
    };
    result.push_str(fragment);
    Ok(result)
}

/// RFC 3986 section 5.2 steps 2 to 7 on parsed components, as libxml2 does it.
fn merge(r: Uri, b: Uri) -> Uri {
    let (r_auth, b_auth) = (r.has_authority(), b.has_authority());
    if r.path.is_none() && !r_auth {
        return Uri {
            query: r.query.or(b.query),
            fragment: r.fragment,
            ..b
        };
    }
    let head = Uri {
        scheme: b.scheme,
        query: r.query,
        fragment: r.fragment,
        ..Uri::default()
    };
    if r_auth {
        return Uri {
            user: r.user,
            host: r.host,
            port: r.port,
            path: r.path,
            ..head
        };
    }
    let head = Uri {
        user: b.user,
        host: b.host,
        port: b.port,
        empty_authority: b.empty_authority,
        ..head
    };
    let rpath = r.path.as_deref().unwrap_or("");
    if rpath.starts_with('/') {
        return Uri {
            path: r.path,
            ..head
        };
    }
    let dir = b
        .path
        .as_deref()
        .map_or("", |p| &p[..p.rfind('/').map_or(0, |i| i + 1)]);
    let slash = if dir.is_empty() && !rpath.is_empty() && b_auth {
        "/"
    } else {
        ""
    };
    Uri {
        path: Some(normalize_path(&[dir, slash, rpath].concat())),
        ..head
    }
}

/// `xmlBuildURI`: resolves `reference` against `base`. A base without `://` is
/// a filesystem path; an empty reference yields the base.
pub fn resolve(reference: &str, base: &str) -> Result<String, UriError> {
    let (reference, base) = (cstr(reference), cstr(base));
    let parsed = match reference.is_empty() {
        true => None,
        false => Some(Uri::parse(reference)?),
    };
    if parsed.as_ref().is_some_and(|r| r.scheme.is_some()) {
        return Ok(reference.to_owned());
    }
    if !base.contains("://") {
        return resolve_path(reference, base);
    }
    let Ok(base) = Uri::parse(base) else {
        return parsed
            .map(|r| r.to_string())
            .ok_or(UriError::Syntax { offset: 0 });
    };
    Ok(match parsed {
        Some(r) => merge(r, base),
        None => Uri {
            fragment: None,
            ..base
        },
    }
    .to_string())
}

/// `xmlParseUriOrPath`: strings without `://` are filesystem paths.
fn parse_or_path(s: &str) -> Result<Uri, UriError> {
    if !s.contains("://") {
        return Ok(Uri {
            path: Some(normalize(s, true)),
            ..Uri::default()
        });
    }
    let uri = Uri::parse(s)?;
    Ok(Uri {
        path: uri.path.as_deref().map(normalize_path),
        ..uri
    })
}

/// `xmlBuildRelativeURI`: expresses `uri` relative to `base`; returns the
/// (normalized) `uri` when no relative form exists, and `uri` verbatim if it
/// does not parse.
pub fn relative(uri: &str, base: &str) -> Result<String, UriError> {
    let (uri, base) = (cstr(uri), cstr(base));
    if uri.is_empty() {
        return Err(UriError::Syntax { offset: 0 });
    }
    let Ok(target) = parse_or_path(uri) else {
        return Ok(uri.to_owned());
    };
    let Some(base) = (!base.is_empty())
        .then(|| parse_or_path(base).ok())
        .flatten()
    else {
        return Ok(target.to_string());
    };
    if (&base.scheme, &base.host, base.port, base.empty_authority)
        != (
            &target.scheme,
            &target.host,
            target.port,
            target.empty_authority,
        )
    {
        return Ok(target.to_string());
    }
    if base.path == target.path {
        return Ok(String::new());
    }
    let Some(bpath) = base.path.as_deref() else {
        return Ok(escape(target.path.as_deref().unwrap_or(""), "/;&=+$,"));
    };
    let tpath = target.path.as_deref().unwrap_or("/");
    let (b, t) = (bpath.as_bytes(), tpath.as_bytes());
    if (at(b, 0) == b'/') != (at(t, 0) == b'/') {
        return Ok(Uri {
            path: Some(tpath.to_owned()),
            ..target
        }
        .to_string());
    }
    let pos = b.iter().zip(t).take_while(|(x, y)| x == y).count();
    if at(b, pos) == at(t, pos) {
        return Ok(String::new());
    }
    let start = t[..pos]
        .iter()
        .rposition(|&c| c == b'/')
        .map_or(0, |i| i + 1);
    let tail = &tpath[start..];
    let ups = b[start..].iter().filter(|&&c| c == b'/').count();
    Ok(match (ups, tail) {
        (0, "") => "./".to_owned(),
        (0, _) => escape(tail, "/;&=+$,"),
        _ => escape(
            &format!(
                "{}{}",
                "../".repeat(ups),
                tail.strip_prefix('/').unwrap_or(tail)
            ),
            "/;&=+$,",
        ),
    })
}
