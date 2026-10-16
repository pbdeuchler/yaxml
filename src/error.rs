use std::fmt;

/// A resource ceiling, checked before retaining additional processor state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Limit {
    InputBytes,
    OutputBytes,
    TokenBytes,
    DecodedBytes,
    DecodedTokenBytes,
    Depth,
    Attributes,
    Events,
    OwnedBytes,
}

/// Structured failures; offsets are input bytes, or prospective writer output bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ErrorKind {
    InvalidUtf8,
    InvalidUtf16,
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
    EncodingMismatch,
    UnsupportedDtd,
    LimitExceeded(Limit),
    Allocation,
    /// The non-recycled owned-document identity space is exhausted.
    IdentifierExhausted,
    /// The event callback requested terminal cancellation.
    Aborted,
    /// The operation is disallowed after completion, failure, or a recovered panic.
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

/// A ceiling for bounded owned-value edits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum EditLimit {
    ValueBytes,
    OwnedBytes,
}

/// Failures leave logical values and source provenance unchanged.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum EditErrorKind {
    InvalidHandle,
    WrongNodeKind,
    MissingAttribute,
    NamespaceDeclaration,
    InvalidCharacter,
    InvalidValue,
    LimitExceeded(EditLimit),
    Allocation,
}

/// An edit failure, with an optional UTF-8 byte index in the replacement value.
/// Handle, kind, lookup, namespace, limit and allocation errors have no index.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EditError {
    pub kind: EditErrorKind,
    pub value_offset: Option<usize>,
}

impl EditError {
    pub(crate) const fn new(kind: EditErrorKind, value_offset: Option<usize>) -> Self {
        Self { kind, value_offset }
    }
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "XML edit error {:?}", self.kind)?;
        if let Some(offset) = self.value_offset {
            write!(f, " at value byte {offset}")?;
        }
        Ok(())
    }
}

impl std::error::Error for EditError {}

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

/// Failure while validating XML events or writing to a caller-provided sink.
/// The first failure preserves its cause; later writer calls return InvalidState.
#[derive(Debug)]
#[non_exhaustive]
pub enum WriteError {
    Xml(Error),
    Io(std::io::Error),
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Xml(error) => error.fmt(f),
            Self::Io(error) => write!(f, "failed to write XML output: {error}"),
        }
    }
}

impl std::error::Error for WriteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Xml(error) => Some(error),
            Self::Io(error) => Some(error),
        }
    }
}
