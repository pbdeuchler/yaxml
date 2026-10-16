use std::fmt;

/// A resource ceiling, checked before retaining additional parser state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Limit {
    InputBytes,
    TokenBytes,
    Depth,
    Attributes,
    Events,
}

/// Structured failure categories; offsets are zero-based input byte offsets.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ErrorKind {
    InvalidUtf8,
    InvalidCharacter,
    UnexpectedEof,
    Syntax,
    InvalidName,
    MismatchedEndTag,
    DuplicateAttribute,
    Namespace,
    InvalidReference,
    UnknownEntity,
    MissingRoot,
    MultipleRoots,
    TextOutsideRoot,
    InvalidDeclaration,
    UnsupportedEncoding,
    UnsupportedDtd,
    LimitExceeded(Limit),
    Allocation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Error {
    pub kind: ErrorKind,
    pub offset: usize,
}

impl Error {
    pub(crate) const fn new(kind: ErrorKind, offset: usize) -> Self {
        Self { kind, offset }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "XML error {:?} at byte {}", self.kind, self.offset)
    }
}

impl std::error::Error for Error {}
