use crate::{Error, ErrorKind};
use memchr::{memchr, memchr2, memchr3};
use std::borrow::Cow;

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

/// XML end-of-line and CDATA attribute normalization. References are expanded
/// after literal whitespace normalization: &#xA; must remain a newline.
pub(crate) fn normalize_required(
    raw: &str,
    offset: usize,
    attribute: bool,
    references: bool,
) -> Result<String, Error> {
    normalize(raw, offset, attribute, references, true)
}

pub(crate) fn normalize_replacement_required(raw: &str, offset: usize) -> Result<String, Error> {
    normalize(raw, offset, false, true, false)
}

fn normalize(
    raw: &str,
    offset: usize,
    attribute: bool,
    references: bool,
    physical_eol: bool,
) -> Result<String, Error> {
    let mut out = String::new();
    out.try_reserve(raw.len())
        .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
    let mut chars = raw.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            '&' if references => {
                let start = i + 1;
                let end = raw[start..]
                    .find(';')
                    .map(|n| start + n)
                    .ok_or(Error::new(ErrorKind::InvalidReference, offset + i))?;
                let entity = &raw[start..end];
                let decoded = match entity {
                    "lt" => '<',
                    "gt" => '>',
                    "amp" => '&',
                    "apos" => '\'',
                    "quot" => '"',
                    _ if entity.starts_with('#') => {
                        let (digits, radix) = match entity.strip_prefix("#x") {
                            Some(s) => (s, 16),
                            None => (&entity[1..], 10),
                        };
                        if digits.is_empty()
                            || !digits.bytes().all(|b| {
                                if radix == 16 {
                                    b.is_ascii_hexdigit()
                                } else {
                                    b.is_ascii_digit()
                                }
                            })
                        {
                            return Err(Error::new(ErrorKind::InvalidReference, offset + i));
                        }
                        u32::from_str_radix(digits, radix)
                            .ok()
                            .and_then(char::from_u32)
                            .filter(|c| xml_char(*c))
                            .ok_or(Error::new(ErrorKind::InvalidReference, offset + i))?
                    }
                    _ => return Err(Error::new(ErrorKind::UnknownEntity, offset + i)),
                };
                out.push(decoded);
                while chars.peek().is_some_and(|(pos, _)| *pos <= end) {
                    chars.next();
                }
            }
            '\r' if physical_eol => {
                if chars.peek().is_some_and(|(_, c)| *c == '\n') {
                    chars.next();
                }
                out.push(if attribute { ' ' } else { '\n' });
            }
            '\n' | '\t' if attribute => out.push(' '),
            _ => out.push(c),
        }
    }
    Ok(out)
}

pub(crate) fn copy_cow<'a>(s: &Cow<'a, str>, offset: usize) -> Result<Cow<'a, str>, Error> {
    match s {
        Cow::Borrowed(s) => Ok(Cow::Borrowed(s)),
        Cow::Owned(s) => {
            let mut out = String::new();
            out.try_reserve(s.len())
                .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
            out.push_str(s);
            Ok(Cow::Owned(out))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
