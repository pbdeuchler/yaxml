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
#[derive(Debug, Eq, PartialEq)]
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

    #[cfg(test)]
    pub(crate) fn qualified_source(&self) -> &Cow<'a, str> {
        &self.qualified
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
    if let Some((prefix, local)) = name.split_once(':') {
        validate_name(prefix, offset, false)?;
        validate_name(local, offset + prefix.len() + 1, false)?;
        Ok((Some(prefix), local))
    } else {
        validate_name(name, offset, false)?;
        Ok((None, name))
    }
}

pub(crate) fn validate_name(name: &str, offset: usize, colon: bool) -> Result<(), Error> {
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
