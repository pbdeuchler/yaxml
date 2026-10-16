use std::fmt;

/// A resource ceiling, checked before retaining additional parser state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Limit {
    InputBytes,
    TokenBytes,
    Depth,
    Attributes,
    Events,
    OwnedBytes,
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
    /// The event callback requested terminal cancellation.
    Aborted,
    /// Input was supplied after successful final EOF.
    InvalidState,
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

/// Failure while consuming an explicit caller-provided byte reader.
#[derive(Debug)]
#[non_exhaustive]
pub enum ReadError {
    Xml(Error),
    Io(std::io::Error),
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Xml(error) => error.fmt(f),
            Self::Io(error) => write!(f, "failed to read XML input: {error}"),
        }
    }
}

impl std::error::Error for ReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Xml(error) => Some(error),
            Self::Io(error) => Some(error),
        }
    }
}
