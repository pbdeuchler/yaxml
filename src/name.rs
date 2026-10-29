use std::borrow::Cow;
use std::num::NonZeroUsize;

use crate::{Error, ErrorKind};

/// A validated QName and its namespace URI, borrowing or owning its strings.
///
/// The namespace is kept as supplied, `Some("")` included; the parser or
/// [`Writer`](crate::Writer) checks it against the namespace scope.
///
/// ```
/// use std::borrow::Cow;
/// use yaxml::Name;
///
/// let name = Name::new(String::from("p:item"), Some(Cow::Borrowed("urn:example")))?;
/// assert_eq!((name.prefix(), name.local()), (Some("p"), "item"));
/// assert_eq!(name.namespace(), Some("urn:example"));
/// # Ok::<(), yaxml::Error>(())
/// ```
///
/// Getter results borrow the name, not its source:
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
    /// Validates `qualified` as a QName, without copying it.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::InvalidName`] with the byte offset into `qualified`.
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

    /// The full `prefix:local` spelling.
    pub fn qualified(&self) -> &str {
        &self.qualified
    }

    /// The prefix, if any.
    pub fn prefix(&self) -> Option<&str> {
        self.prefix_length
            .map(|length| &self.qualified[..length.get()])
    }

    /// The local part.
    pub fn local(&self) -> &str {
        match self.prefix_length {
            Some(length) => &self.qualified[length.get() + 1..],
            None => &self.qualified,
        }
    }

    /// The namespace URI, as supplied or resolved.
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

    /// This name with its borrowed strings `copy`ed.
    pub(crate) fn into_static(
        self,
        copy: &mut impl FnMut(&str) -> Result<Cow<'static, str>, Error>,
    ) -> Result<Name<'static>, Error> {
        let mut owned = |text: Cow<'a, str>| match text {
            Cow::Owned(text) => Ok(Cow::Owned(text)),
            Cow::Borrowed(text) => copy(text),
        };
        Ok(Name {
            qualified: owned(self.qualified)?,
            prefix_length: self.prefix_length,
            namespace: self.namespace.map(&mut owned).transpose()?,
        })
    }

    /// A copy that borrows what this name borrows.
    pub(crate) fn try_clone(&self) -> Result<Self, std::collections::TryReserveError> {
        Ok(Self {
            qualified: crate::fallible::cow(&self.qualified)?,
            prefix_length: self.prefix_length,
            namespace: self
                .namespace
                .as_ref()
                .map(crate::fallible::cow)
                .transpose()?,
        })
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

/// The length of the longest prefix of `text` made of name characters,
/// colons included when `colon`; ASCII is classified by table.
pub(crate) fn name_chars_length(text: &str, colon: bool) -> usize {
    let bytes = text.as_bytes();
    let ascii = bytes
        .iter()
        .position(|&byte| {
            byte >= 0x80 || ASCII_NAME[usize::from(byte)] & 2 == 0 && !(colon && byte == b':')
        })
        .unwrap_or(bytes.len());
    if bytes.get(ascii).is_none_or(|byte| *byte < 0x80) {
        return ascii;
    }
    let rest = &text[ascii..];
    ascii
        + rest
            .char_indices()
            .find(|(_, c)| !name_char(*c, colon))
            .map_or(rest.len(), |(i, _)| i)
}

/// An `NCName`, as `xmlValidateNCName` without blanks reads one.
pub(crate) fn is_ncname(name: &str) -> bool {
    name.chars().next().is_some_and(|c| name_start(c, false))
        && name_chars_length(name, false) == name.len()
}

/// [`validate_name`], then under `old10` the Fourth Edition rule.
pub(crate) fn validate_edition(
    name: &str,
    offset: usize,
    colon: bool,
    old10: bool,
) -> Result<(), Error> {
    validate_name(name, offset, colon)?;
    match old10 && !old10_name(name, false) {
        true => Err(Error::new(ErrorKind::InvalidName, offset)),
        false => Ok(()),
    }
}

/// Whether `name` is a Fourth Edition `Name` (or `Nmtoken`), libxml2's
/// `XML_PARSE_OLD10` rule. Fourth Edition names are a subset of Fifth
/// Edition names with the same ASCII, so callers check only names that
/// passed [`validate_name`] and hold non-ASCII.
pub(crate) fn old10_name(name: &str, nmtoken: bool) -> bool {
    name.char_indices()
        .all(|(i, c)| old10_char(c, i == 0 && !nmtoken))
}

/// A Fourth Edition name character, or name-start character for `start`.
pub(crate) fn old10_char(c: char, start: bool) -> bool {
    use crate::chars::{is_base_char, is_combining, is_digit, is_extender, is_ideographic};
    is_base_char(c)
        || is_ideographic(c)
        || matches!(c, '_' | ':')
        || !start && (is_digit(c) || is_combining(c) || is_extender(c) || matches!(c, '.' | '-'))
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
