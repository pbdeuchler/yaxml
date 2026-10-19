use std::borrow::Cow;
use std::num::NonZeroUsize;

use crate::{Error, ErrorKind};

/// A lexical QName and its namespace metadata, borrowing or owning its strings.
///
/// The QName is validated at construction. Namespace metadata is preserved as
/// supplied; its consistency with an element's namespace scope is checked by
/// the parser or writer. Getter results borrow this view, even when its strings
/// originated in a longer-lived input.
///
/// Keep the name alive while using a getter's result:
///
/// ```compile_fail
/// let local = {
///     let name = yaxml::Name::new(String::from("p:item"), None).unwrap();
///     name.local()
/// };
/// println!("{local}");
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Name<'a> {
    qualified: Cow<'a, str>,
    prefix_length: Option<NonZeroUsize>,
    namespace: Option<Cow<'a, str>>,
}

impl<'a> Name<'a> {
    /// Validates a QName without copying borrowed input or a supplied owned buffer.
    /// Errors identify a UTF-8 byte offset relative to the start of the QName.
    /// Namespace metadata is not resolved or validated against a scope here.
    pub fn new(
        qualified: impl Into<Cow<'a, str>>,
        namespace: Option<Cow<'a, str>>,
    ) -> Result<Self, Error> {
        let qualified = qualified.into();
        let (prefix, _) = qname(&qualified, 0)?;
        let prefix_length = prefix.and_then(|prefix| NonZeroUsize::new(prefix.len()));
        Ok(Self::from_validated_parts(
            qualified,
            prefix_length,
            namespace,
        ))
    }

    /// Returns the complete lexical spelling.
    pub fn qualified(&self) -> &str {
        &self.qualified
    }

    /// Returns the lexical prefix when present.
    pub fn prefix(&self) -> Option<&str> {
        self.prefix_length
            .map(|length| &self.qualified[..length.get()])
    }

    /// Returns the local part of the QName.
    pub fn local(&self) -> &str {
        match self.prefix_length {
            Some(length) => &self.qualified[length.get() + 1..],
            None => &self.qualified,
        }
    }

    /// Returns namespace metadata as supplied or resolved, including an empty URI.
    pub fn namespace(&self) -> Option<&str> {
        self.namespace.as_deref()
    }

    /// The caller has validated this QName and derived the split from these bytes.
    /// Retained pool views preserve that metadata rather than rescanning the name.
    pub(crate) fn from_validated_parts(
        qualified: Cow<'a, str>,
        prefix_length: Option<NonZeroUsize>,
        namespace: Option<Cow<'a, str>>,
    ) -> Self {
        debug_assert!(!qualified.is_empty());
        if let Some(length) = prefix_length {
            debug_assert_eq!(qualified.as_bytes().get(length.get()), Some(&b':'));
            debug_assert!(length.get() < qualified.len() - 1);
        }
        Self {
            qualified,
            prefix_length,
            namespace,
        }
    }

    pub(crate) fn set_namespace(&mut self, namespace: Option<Cow<'a, str>>) {
        self.namespace = namespace;
    }

    pub(crate) fn prefix_length(&self) -> Option<NonZeroUsize> {
        self.prefix_length
    }

    pub(crate) fn qualified_source(&self) -> &Cow<'a, str> {
        &self.qualified
    }

    pub(crate) fn try_into_static(self, offset: usize) -> Result<Name<'static>, Error> {
        let Cow::Owned(qualified) = self.qualified else {
            return Err(Error::new(ErrorKind::InvalidState, offset));
        };
        let namespace = match self.namespace {
            Some(Cow::Owned(namespace)) => Some(Cow::Owned(namespace)),
            Some(Cow::Borrowed(_)) => {
                return Err(Error::new(ErrorKind::InvalidState, offset));
            }
            None => None,
        };
        Ok(Name::from_validated_parts(
            Cow::Owned(qualified),
            self.prefix_length,
            namespace,
        ))
    }

    pub(crate) fn as_borrowed(&self) -> Name<'_> {
        Name::from_validated_parts(
            Cow::Borrowed(&self.qualified),
            self.prefix_length,
            self.namespace.as_deref().map(Cow::Borrowed),
        )
    }
}

pub(crate) fn qname(name: &str, offset: usize) -> Result<(Option<&str>, &str), Error> {
    if let Some(colon) = memchr::memchr(b':', name.as_bytes()) {
        let (prefix, local) = (&name[..colon], &name[colon + 1..]);
        validate_name(prefix, offset, false)?;
        validate_name(local, offset + prefix.len() + 1, false)?;
        Ok((Some(prefix), local))
    } else {
        validate_name(name, offset, false)?;
        Ok((None, name))
    }
}

/// ASCII name classes: bit 0 starts a name, bit 1 continues one (colon excluded).
const ASCII_NAME: [u8; 128] = {
    let mut table = [0; 128];
    let mut byte = 0;
    while byte < 128 {
        let c = byte as u8;
        table[byte] = if c.is_ascii_alphabetic() || c == b'_' {
            3
        } else if c.is_ascii_digit() || c == b'-' || c == b'.' {
            2
        } else {
            0
        };
        byte += 1;
    }
    table
};

pub(crate) fn validate_name(name: &str, offset: usize, colon: bool) -> Result<(), Error> {
    // ASCII prefix by table; the first non-ASCII byte starts the char path.
    let bytes = name.as_bytes();
    let mut ascii = 0;
    while let Some(&byte) = bytes.get(ascii) {
        if byte >= 0x80 {
            break;
        }
        let class = if ascii == 0 { 1 } else { 2 };
        if ASCII_NAME[usize::from(byte)] & class == 0 && !(colon && byte == b':') {
            return Err(Error::new(ErrorKind::InvalidName, offset + ascii));
        }
        ascii += 1;
    }
    if ascii == bytes.len() && ascii > 0 {
        return Ok(());
    }
    if ascii > 0 {
        return match name[ascii..]
            .char_indices()
            .find(|(_, c)| !name_char(*c, colon))
        {
            Some((i, _)) => Err(Error::new(ErrorKind::InvalidName, offset + ascii + i)),
            None => Ok(()),
        };
    }
    let mut chars = name.char_indices();
    let Some((_, first)) = chars.next() else {
        return Err(Error::new(ErrorKind::InvalidName, offset));
    };
    if !name_start(first, colon) {
        return Err(Error::new(ErrorKind::InvalidName, offset));
    }
    for (i, c) in chars {
        if !name_char(c, colon) {
            return Err(Error::new(ErrorKind::InvalidName, offset + i));
        }
    }
    Ok(())
}

pub(crate) fn name_char(c: char, colon: bool) -> bool {
    name_start(c, colon)
        || matches!(c, '-' | '.' | '0'..='9' | '\u{b7}' | '\u{300}'..='\u{36f}' | '\u{203f}'..='\u{2040}')
}

pub(crate) fn name_start(c: char, colon: bool) -> bool {
    (colon && c == ':')
        || matches!(c, 'A'..='Z' | 'a'..='z' | '_'
        | '\u{c0}'..='\u{d6}' | '\u{d8}'..='\u{f6}' | '\u{f8}'..='\u{2ff}'
        | '\u{370}'..='\u{37d}' | '\u{37f}'..='\u{1fff}' | '\u{200c}'..='\u{200d}'
        | '\u{2070}'..='\u{218f}' | '\u{2c00}'..='\u{2fef}' | '\u{3001}'..='\u{d7ff}'
        | '\u{f900}'..='\u{fdcf}' | '\u{fdf0}'..='\u{fffd}' | '\u{10000}'..='\u{effff}')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference(name: &str, colon: bool) -> Result<(), usize> {
        let mut chars = name.char_indices();
        match chars.next() {
            Some((_, first)) if name_start(first, colon) => {}
            _ => return Err(0),
        }
        chars
            .find(|(_, c)| !name_char(*c, colon))
            .map_or(Ok(()), |(i, _)| Err(i))
    }

    #[test]
    fn validate_name_matches_the_name_productions() {
        let alphabet: Vec<char> = (0u8..128)
            .map(char::from)
            .chain([
                '\u{b7}',
                '\u{c0}',
                '\u{d7}',
                '\u{300}',
                '\u{2070}',
                '\u{10000}',
            ])
            .collect();
        let mut names = vec![String::new()];
        for &a in &alphabet {
            names.push(a.to_string());
            for &b in &alphabet {
                names.push(format!("{a}{b}"));
            }
        }
        names.extend([
            "ab:cd".into(),
            "abc\u{d7}".into(),
            "é1-x".into(),
            "x.y\u{300}z".into(),
        ]);
        for name in &names {
            for colon in [false, true] {
                let expected = reference(name, colon).map_err(|i| 7 + i);
                let actual = validate_name(name, 7, colon).map_err(|error| {
                    assert_eq!(error.kind, ErrorKind::InvalidName);
                    error.offset
                });
                assert_eq!(actual, expected, "{name:?} colon={colon}");
            }
        }
    }
}
