use crate::{Error, ErrorKind};
use memchr::{memchr, memchr2, memchr3};

pub(crate) fn xml_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}')
}

/// Byte offset of the first character outside `xml_char`. In valid UTF-8
/// only C0 controls and U+FFFE/U+FFFF (`EF BF BE`/`EF BF BF`) can fail, so
/// 64-byte blocks are screened with a branch-free byte test first.
pub(crate) fn invalid_char(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let control = |b: u8| (b < 0x20) & (b != b'\t') & (b != b'\n') & (b != b'\r');
    let mut start = 0;
    for block in bytes.chunks(64) {
        if block
            .iter()
            .fold(false, |any, &b| any | control(b) | (b == 0xef))
        {
            let found = block.iter().enumerate().position(|(i, &b)| {
                control(b)
                    || (b == 0xef
                        && bytes.get(start + i + 1) == Some(&0xbf)
                        && matches!(bytes.get(start + i + 2), Some(0xbe | 0xbf)))
            });
            if let Some(i) = found {
                return Some(start + i);
            }
        }
        start += block.len();
    }
    None
}

pub(crate) fn needs_normalization(raw: &str, attribute: bool, references: bool) -> bool {
    let bytes = raw.as_bytes();
    match (attribute, references) {
        (false, false) => memchr(b'\r', bytes).is_some(),
        (false, true) => memchr2(b'\r', b'&', bytes).is_some(),
        (true, false) => memchr3(b'\r', b'\n', b'\t', bytes).is_some(),
        (true, true) => {
            memchr3(b'\r', b'\n', b'&', bytes).is_some() || memchr(b'\t', bytes).is_some()
        }
    }
}

/// XML end-of-line and CDATA attribute normalization, appended to `out`. References are expanded after literal whitespace
/// normalization: &#xA; must remain a newline. A malformed reference is
/// `recover`ed from by dropping it as far as libxml2 reads it, except that
/// an attribute value fails at a character reference libxml2 reads as zero
/// (an `InvalidReference` error at the offset where libxml2 reads on).
pub(crate) fn normalize_into(
    raw: &str,
    offset: usize,
    attribute: bool,
    references: bool,
    out: &mut String,
    recover: impl FnMut(Error) -> Result<(), Error>,
) -> Result<(), Error> {
    normalize(raw, offset, attribute, references, true, out, recover)
}

pub(crate) fn normalize_replacement_required(raw: &str, offset: usize) -> Result<String, Error> {
    let mut out = String::new();
    normalize(raw, offset, false, true, false, &mut out, Err)?;
    Ok(out)
}

/// Recovery from characters outside `Char` in `raw`, as libxml2 does:
/// dropped from text, U+FFFD in an `attribute` value.
pub(crate) fn repair_characters(
    raw: &str,
    offset: usize,
    attribute: bool,
    recover: &mut impl FnMut(Error) -> Result<(), Error>,
) -> Result<String, Error> {
    let mut out = String::new();
    out.try_reserve(raw.len())
        .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
    let mut rest = raw;
    while let Some(at) = invalid_char(rest) {
        recover(Error::new(
            ErrorKind::InvalidCharacter,
            offset + raw.len() - rest.len() + at,
        ))?;
        out.push_str(&rest[..at]);
        out.extend(attribute.then_some('\u{fffd}'));
        let length = rest[at..].chars().next().map_or(1, char::len_utf8);
        rest = &rest[at + length..];
    }
    out.push_str(rest);
    Ok(out)
}

fn normalize(
    raw: &str,
    offset: usize,
    attribute: bool,
    references: bool,
    physical_eol: bool,
    out: &mut String,
    mut recover: impl FnMut(Error) -> Result<(), Error>,
) -> Result<(), Error> {
    out.try_reserve(raw.len())
        .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
    let bytes = raw.as_bytes();
    // Unchanged runs between special bytes are copied whole; every special
    // byte is ASCII, so run boundaries are character boundaries.
    let (mut run, mut i) = (0, 0);
    while let Some(&byte) = bytes.get(i) {
        let special = match byte {
            b'&' => references,
            b'\r' => physical_eol,
            b'\n' | b'\t' => attribute,
            _ => false,
        };
        if !special {
            i += 1;
            continue;
        }
        out.push_str(&raw[run..i]);
        match byte {
            b'&' => {
                let start = i + 1;
                let value = memchr(b';', &bytes[start..])
                    .map(|n| start + n)
                    .ok_or(Error::new(ErrorKind::InvalidReference, offset + i))
                    .and_then(|end| Ok((reference(&raw[start..end], offset + i)?, end + 1)));
                match value {
                    Ok((value, end)) => {
                        out.push(value);
                        i = end;
                    }
                    Err(error) => {
                        recover(error)?;
                        let (value, end) = malformed_reference(&raw[start..]);
                        if attribute && value.is_none() && bytes.get(start) == Some(&b'#') {
                            let at = offset + start + end;
                            return Err(Error::new(ErrorKind::InvalidReference, at));
                        }
                        out.extend(value);
                        i = start + end;
                    }
                }
            }
            b'\r' => {
                i += 1 + usize::from(bytes.get(i + 1) == Some(&b'\n'));
                out.push(if attribute { ' ' } else { '\n' });
            }
            _ => {
                i += 1;
                out.push(' ');
            }
        }
        run = i;
    }
    out.push_str(&raw[run..]);
    Ok(())
}

/// How libxml2 reads a malformed reference whose body (after `&`) starts
/// `rest`: past `#`, an `x` and the digits of a character reference, or the
/// name of an entity reference, and a `;` that follows them. A complete
/// character reference to a non-character becomes U+FFFD, unless it is zero.
fn malformed_reference(rest: &str) -> (Option<char>, usize) {
    let bytes = rest.as_bytes();
    let (body, radix) = match bytes {
        [b'#', b'x', ..] => (2, 16),
        [b'#', ..] => (1, 10),
        // A name starts with a letter, `_`, `:` or a non-ASCII character.
        [b'0'..=b'9' | b'-' | b'.', ..] => return (None, 0),
        _ => (0, 0),
    };
    let part = |b: &u8| match radix {
        0 => !b.is_ascii() || b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b':'),
        _ => char::from(*b).is_digit(radix),
    };
    let end = body + bytes[body..].iter().take_while(|b| part(b)).count();
    let closed = bytes.get(end) == Some(&b';');
    let zero = rest[body..end].bytes().all(|digit| digit == b'0');
    let value = (radix != 0 && closed && !zero).then_some('\u{fffd}');
    (value, end + usize::from(closed))
}

/// The character named by a predefined or numeric reference body.
fn reference(entity: &str, offset: usize) -> Result<char, Error> {
    match entity.strip_prefix('#') {
        Some(number) => numeric_reference(number).ok_or(ErrorKind::InvalidReference),
        None => predefined(entity).ok_or(ErrorKind::UnknownEntity),
    }
    .map_err(|kind| Error::new(kind, offset))
}

/// The character of a predefined entity.
pub(crate) fn predefined(name: &str) -> Option<char> {
    Some(match name {
        "lt" => '<',
        "gt" => '>',
        "amp" => '&',
        "apos" => '\'',
        "quot" => '"',
        _ => return None,
    })
}

/// The character named by the digits of a character reference after `&#`,
/// with a leading `x` for hexadecimal.
pub(crate) fn numeric_reference(number: &str) -> Option<char> {
    let (digits, radix) = match number.strip_prefix('x') {
        Some(digits) => (digits, 16),
        None => (number, 10),
    };
    if digits.is_empty() || !digits.chars().all(|digit| digit.is_digit(radix)) {
        return None;
    }
    u32::from_str_radix(digits, radix)
        .ok()
        .and_then(char::from_u32)
        .filter(|c| xml_char(*c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_references_are_dropped_as_far_as_libxml2_reads_them() {
        use ErrorKind::{InvalidReference as Invalid, UnknownEntity as Unknown};
        // (text, recovered value, errors at offsets)
        type Case = (&'static str, &'static str, &'static [(ErrorKind, usize)]);
        let cases: &[Case] = &[
            ("a&amp;b", "a&b", &[]),
            ("a & b", "a  b", &[(Invalid, 2)]),
            ("a &x b", "a  b", &[(Invalid, 2)]),
            ("&.x;", ".x;", &[(Unknown, 0)]),
            ("&#x4G;", "G;", &[(Invalid, 0)]),
            ("&#12a", "a", &[(Invalid, 0)]),
            ("&#0;x", "x", &[(Invalid, 0)]),
            ("&#;x", "x", &[(Invalid, 0)]),
            (
                "&#1;&#xFFFE;&#x110000;",
                "\u{fffd}\u{fffd}\u{fffd}",
                &[(Invalid, 0), (Invalid, 4), (Invalid, 12)],
            ),
            ("&undeclared;!", "!", &[(Unknown, 0)]),
            ("&é;", "", &[(Unknown, 0)]),
        ];
        for (text, value, errors) in cases {
            let mut found = Vec::new();
            let recover = |error: Error| {
                found.push((error.kind, error.offset));
                Ok(())
            };
            let mut recovered = String::from("kept:");
            normalize_into(text, 0, false, true, &mut recovered, recover).unwrap();
            assert_eq!(recovered, format!("kept:{value}"), "{text}");
            assert_eq!(found, *errors, "{text}");
            let strict = normalize_into(text, 0, false, true, &mut String::new(), Err);
            let first = errors
                .first()
                .map(|&(kind, offset)| Error::new(kind, offset));
            assert_eq!(strict.err(), first, "{text}");
        }
    }

    fn reference(text: &str) -> Option<usize> {
        text.char_indices()
            .find(|(_, c)| !xml_char(*c))
            .map(|(i, _)| i)
    }

    #[test]
    fn invalid_char_matches_the_char_production() {
        let filler = "a".repeat(62);
        let cases: &[(&str, Option<usize>)] = &[
            ("", None),
            ("plain\ttext\r\nok", None),
            ("\u{fffd}\u{e000}\u{10ffff}\u{ff21}", None),
            ("a\u{0}b", Some(1)),
            ("ab\u{1f}", Some(2)),
            ("\u{b}", Some(0)),
            ("x\u{fffe}", Some(1)),
            ("xy\u{ffff}z", Some(2)),
            ("\u{ffef}\u{fffe}", Some(3)),
        ];
        for (text, expected) in cases {
            assert_eq!(invalid_char(text), *expected, "{text:?}");
            assert_eq!(invalid_char(text), reference(text), "{text:?}");
        }
        // A U+FFFE sequence straddling the 64-byte screening block boundary.
        for pad in 60..66 {
            let text = format!("{}\u{fffe}", &format!("{filler}aaaa")[..pad]);
            assert_eq!(invalid_char(&text), Some(pad));
        }
        let every: String = (0..=0x10ffff).filter_map(char::from_u32).collect();
        assert_eq!(invalid_char(&every), reference(&every));
        let valid: String = every.chars().filter(|c| xml_char(*c)).collect();
        assert_eq!(invalid_char(&valid), None);
        for c in every.chars().filter(|c| !xml_char(*c)) {
            let text = format!("{valid}{c}");
            assert_eq!(invalid_char(&text), Some(valid.len()), "{c:?}");
        }
    }
}
