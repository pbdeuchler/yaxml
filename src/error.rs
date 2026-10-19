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
    /// Bytes in the document type declaration, including its internal subset.
    DtdBytes,
    /// Entity replacement bytes produced by expansion, or the equivalent work.
    EntityExpansion,
    /// Entity references expanded across the document.
    EntityReferences,
    /// Nesting depth of entity expansion.
    EntityDepth,
}

/// Structured failures; offsets are input bytes, or prospective writer output bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ErrorKind {
    InvalidUtf8,
    InvalidUtf16,
    /// A byte has no mapping in the declared single-byte encoding.
    UnmappedByte,
    InvalidCharacter,
    UnexpectedEof,
    Syntax,
    InvalidName,
    MismatchedEndTag,
    DuplicateAttribute,
    Namespace,
    InvalidReference,
    /// An undeclared entity whose declaration is required (WFC: Entity Declared).
    UnknownEntity,
    /// An entity references itself directly or indirectly.
    EntityLoop,
    /// A content reference names an unparsed (NDATA) entity.
    UnparsedEntity,
    /// An attribute value references an external entity.
    ExternalEntityInAttribute,
    /// Entity replacement text does not nest correctly with its context.
    UnbalancedEntity,
    /// Malformed document type declaration or internal subset.
    InvalidDtd,
    MissingRoot,
    MultipleRoots,
    TextOutsideRoot,
    InvalidDeclaration,
    UnsupportedEncoding,
    EncodingMismatch,
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

/// A ceiling for bounded owned-tree edits and construction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum EditLimit {
    /// Bytes in one node or attribute value.
    ValueBytes,
    /// Bytes in one local name, prefix or namespace URI.
    NameBytes,
    /// Aggregate supplied construction-string bytes in one call.
    SuppliedBytes,
    /// Explicit and generated items on one constructed element.
    Attributes,
    /// Prospective element depth.
    Depth,
    /// Full new string-pool capacity requests across editor calls.
    OwnedBytes,
    /// Successful fresh node slots across editor calls.
    CreatedNodes,
    /// Successful initial attributes and namespace declarations across calls.
    CreatedAttributes,
    /// Per-call namespace and expanded-name lookup work.
    NamespaceWork,
}

/// A string field in a construction specification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EditComponent {
    /// An element or attribute local name.
    LocalName,
    /// A namespace URI.
    NamespaceUri,
    /// A requested or selected prefix.
    Prefix,
    /// An ordinary attribute value.
    Value,
}

/// A component/item location for construction errors. Semantic errors have no
/// byte offset; global limits and allocation failures have no location.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EditLocation {
    /// The element name supplied directly in `ElementSpec`.
    ElementName {
        component: EditComponent,
        byte_offset: Option<usize>,
    },
    /// A namespace declaration in `ElementSpec.items`.
    NamespaceItem {
        index: usize,
        component: Option<EditComponent>,
        byte_offset: Option<usize>,
    },
    /// An ordinary attribute in `ElementSpec.items`.
    AttributeItem {
        index: usize,
        component: Option<EditComponent>,
        byte_offset: Option<usize>,
    },
}

/// A bounded edit/construction failure. Existing scalar edit methods report
/// `value_offset`; construction methods report the typed `location` instead.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum EditErrorKind {
    InvalidHandle,
    WrongNodeKind,
    ProtectedNode,
    MissingAttribute,
    NamespaceDeclaration,
    InvalidCharacter,
    InvalidValue,
    InvalidName,
    Namespace,
    DuplicateAttribute,
    IdentifierExhausted,
    LimitExceeded(EditLimit),
    Allocation,
}

/// An edit failure. Existing scalar methods retain their optional UTF-8 byte
/// index in `value_offset`. Construction failures use `location`, whose byte
/// offset is relative to the named UTF-8 component. Global failures have no
/// location.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EditError {
    pub kind: EditErrorKind,
    pub value_offset: Option<usize>,
    pub location: Option<EditLocation>,
}

impl EditError {
    pub(crate) const fn new(kind: EditErrorKind, value_offset: Option<usize>) -> Self {
        Self {
            kind,
            value_offset,
            location: None,
        }
    }

    pub(crate) const fn located(kind: EditErrorKind, location: EditLocation) -> Self {
        Self {
            kind,
            value_offset: None,
            location: Some(location),
        }
    }
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "XML edit error {:?}", self.kind)?;
        if let Some(offset) = self.value_offset {
            write!(f, " at value byte {offset}")?;
        }
        if let Some(location) = self.location {
            write!(f, " at {location:?}")?;
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
