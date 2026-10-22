use std::fmt;

/// The resource ceiling an [`ErrorKind::LimitExceeded`] error hit.
///
/// Most variants name a [`Limits`](crate::Limits) or
/// [`DecodeLimits`](crate::DecodeLimits) field; writers and other modules
/// reuse them for their own ceilings of the same kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Limit {
    /// Total input bytes.
    InputBytes,
    /// Total bytes a writer produces.
    OutputBytes,
    /// Raw bytes in one token, or in one written name or value.
    TokenBytes,
    /// Total UTF-8 bytes produced by decoding.
    DecodedBytes,
    /// Decoded UTF-8 bytes in one token.
    DecodedTokenBytes,
    /// Element nesting depth.
    Depth,
    /// Attributes on one element, namespace declarations included.
    Attributes,
    /// Attributes a DOM keeps across the document, namespace declarations and defaults included.
    TotalAttributes,
    /// Emitted events or DOM nodes.
    Events,
    /// Cumulative string capacity requested for copies.
    OwnedBytes,
    /// Bytes in the document type declaration, including its internal subset.
    DtdBytes,
    /// Entity replacement bytes produced by expansion, or the equivalent work.
    EntityExpansion,
    /// Entity references expanded across the document.
    EntityReferences,
    /// Nesting depth of entity expansion.
    EntityDepth,
    /// External resources requested from the resolver.
    ExternalLoads,
    /// Cumulative bytes returned by the resolver.
    ExternalBytes,
}

/// A condition libxml2 reports without stopping the parse, at warning level
/// unless noted. Every surface records it in `errors()` as
/// [`ErrorKind::Warning`], strict or not.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Warning {
    /// An XML declaration's version is `1.x` other than `1.0`
    /// (`XML_WAR_UNKNOWN_VERSION`).
    UnknownVersion,
    /// A namespace declaration's value is not a URI reference
    /// (`XML_WAR_NS_URI`, which libxml2 reports at error level).
    NamespaceUri,
    /// A default namespace declaration's URI is relative
    /// (`XML_WAR_NS_URI_RELATIVE`).
    RelativeNamespaceUri,
    /// An `xml:space` value other than `default` or `preserve`
    /// (`XML_WAR_SPACE_VALUE`).
    SpaceValue,
    /// A processing-instruction target starting with `xml`, other than the
    /// W3C's `xml-stylesheet` and `xml-model` (`XML_ERR_RESERVED_XML_NAME`).
    ReservedTarget,
    /// A declared encoding disagrees with the one detected from a byte order
    /// mark or the UTF-16 `<?` signature, which wins
    /// (`XML_WAR_ENCODING_MISMATCH`).
    EncodingMismatch,
    /// A reference to an undeclared entity that unread markup could
    /// declare; it expands to nothing (`XML_WAR_UNDECLARED_ENTITY`). `error`
    /// is libxml2's error level, used when `DTDLOAD`, `DTDATTR` or `NOENT`
    /// without `NO_XXE` asked for every declaration. Under `DTDVALID` it is
    /// a validity error instead.
    UndeclaredEntity {
        /// Reported at error level.
        error: bool,
    },
    /// An internal-subset declaration of a predefined entity (`lt`, `gt`,
    /// `amp`, `apos`, `quot`) with other replacement text than XML 1.0
    /// section 4.6 allows; the declaration is dropped
    /// (`XML_ERR_REDECL_PREDEF_ENTITY`).
    PredefinedEntity,
}

/// What went wrong in an [`Error`].
///
/// [`ErrorKind::libxml2_code`] maps a kind to libxml2's domain and code.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// Input is not well-formed UTF-8.
    InvalidUtf8,
    /// Input is not well-formed UTF-16 (an unpaired surrogate or odd length).
    InvalidUtf16,
    /// A byte has no mapping in the declared single-byte encoding.
    UnmappedByte,
    /// A character outside the XML `Char` production.
    InvalidCharacter,
    /// Input ended inside markup or with elements still open.
    UnexpectedEof,
    /// Malformed markup.
    Syntax,
    /// A name that is not a valid XML `Name` or `NCName`.
    InvalidName,
    /// An end tag does not match the open element.
    MismatchedEndTag,
    /// An attribute appears twice on one element.
    DuplicateAttribute,
    /// A namespace constraint is violated, such as an unbound prefix.
    Namespace,
    /// A malformed character or entity reference.
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
    /// The resolver failed to load an external DTD subset or entity.
    ExternalResource,
    /// `XML_PARSE_XINCLUDE` processing failed; [`crate::xinclude::process`]
    /// reports the detail.
    XInclude,
    /// The document has no root element.
    MissingRoot,
    /// Content after the root element closed, such as a second root.
    MultipleRoots,
    /// Non-whitespace text or CDATA outside the root element.
    TextOutsideRoot,
    /// A malformed XML declaration, or a PI named `xml` elsewhere.
    InvalidDeclaration,
    /// An encoding that is not built in, such as Shift_JIS.
    UnsupportedEncoding,
    /// A requested parser option, flag or property is unknown or not implemented.
    UnsupportedOption,
    /// The encoding declaration disagrees with the detected encoding, or is
    /// missing where detection requires one.
    EncodingMismatch,
    /// A resource ceiling was reached.
    LimitExceeded(Limit),
    /// A non-fatal diagnostic; the parse went on unchanged.
    Warning(Warning),
    /// Memory allocation failed.
    Allocation,
    /// No more owned-document node identifiers are available.
    IdentifierExhausted,
    /// The event callback requested terminal cancellation.
    Aborted,
    /// The operation is disallowed after completion, failure, or a recovered panic.
    InvalidState,
}

/// A parse, decode or write failure.
///
/// Small and `Copy`; [`Error::describe`] expands it into libxml2's full
/// [`Diagnostic`](crate::Diagnostic).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Error {
    /// What went wrong.
    pub kind: ErrorKind,
    /// Input byte offset of the failure, or the output offset for writers.
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

/// A failure in DTD-processed text. `offset` is relative to the text the
/// caller supplied, and absent when the failure lies in declared text
/// (replacement text or a default value) the caller locates itself.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DtdError {
    pub(crate) kind: ErrorKind,
    pub(crate) offset: Option<usize>,
}

impl DtdError {
    pub(crate) const fn new(kind: ErrorKind, offset: Option<usize>) -> Self {
        Self { kind, offset }
    }
}

impl From<Error> for DtdError {
    fn from(error: Error) -> Self {
        Self::new(error.kind, Some(error.offset))
    }
}

impl std::error::Error for Error {}

/// The [`EditLimits`](crate::EditLimits) ceiling an
/// [`EditErrorKind::LimitExceeded`] error hit.
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

/// Where in an [`ElementSpec`](crate::ElementSpec) a construction error lies.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EditLocation {
    /// The element name supplied directly in `ElementSpec`.
    ElementName {
        /// The faulty string.
        component: EditComponent,
        /// UTF-8 byte offset within that string, when the error has one.
        byte_offset: Option<usize>,
    },
    /// A namespace declaration in `ElementSpec.items`.
    NamespaceItem {
        /// Position in `ElementSpec.items`.
        index: usize,
        /// The faulty string, if one is at fault.
        component: Option<EditComponent>,
        /// UTF-8 byte offset within that string, when the error has one.
        byte_offset: Option<usize>,
    },
    /// An ordinary attribute in `ElementSpec.items`.
    AttributeItem {
        /// Position in `ElementSpec.items`.
        index: usize,
        /// The faulty string, if one is at fault.
        component: Option<EditComponent>,
        /// UTF-8 byte offset within that string, when the error has one.
        byte_offset: Option<usize>,
    },
}

/// What went wrong in an [`EditError`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum EditErrorKind {
    /// The node id is stale, removed or from another document.
    InvalidHandle,
    /// The operation does not apply to this kind of node.
    WrongNodeKind,
    /// The node cannot be changed this way: the document node, the root
    /// element, the DOCTYPE, or entity-reference content.
    ProtectedNode,
    /// The named attribute does not exist.
    MissingAttribute,
    /// The attribute is a namespace declaration, which has its own methods.
    NamespaceDeclaration,
    /// A character outside the XML `Char` production.
    InvalidCharacter,
    /// A value the node cannot hold, such as `--` in a comment or `?>` in a PI.
    InvalidValue,
    /// A name that is not a valid XML name.
    InvalidName,
    /// A prefix or namespace URI that cannot be declared or resolved.
    Namespace,
    /// The element already has an attribute with this name.
    DuplicateAttribute,
    /// The node cannot go there: it would contain its own parent, be a
    /// second document element, or put Text, CDATA or an entity reference
    /// directly under the document; or a sibling anchor has no parent.
    Hierarchy,
    /// No more node identifiers are available.
    IdentifierExhausted,
    /// An edit ceiling was reached.
    LimitExceeded(EditLimit),
    /// Memory allocation failed.
    Allocation,
    /// Content given to [`Editor::parse_content`](crate::Editor::parse_content)
    /// does not parse; `value_offset` is the [`Error`] offset in it.
    Parse(ErrorKind),
}

/// An [`Editor`](crate::Editor) or construction failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EditError {
    /// What went wrong.
    pub kind: EditErrorKind,
    /// UTF-8 byte offset of the fault in the supplied value or name, for
    /// single-value edits.
    pub value_offset: Option<usize>,
    /// Where in the specification the fault lies, for construction.
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

/// A failure while parsing from a [`std::io::Read`] or [`std::io::BufRead`].
#[derive(Debug)]
#[non_exhaustive]
pub enum ReadError {
    /// The input is not acceptable XML.
    Xml(Error),
    /// The reader failed.
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

/// A failure while writing XML to a [`std::io::Write`].
///
/// After the first failure, further writer calls return
/// [`ErrorKind::InvalidState`].
#[derive(Debug)]
#[non_exhaustive]
pub enum WriteError {
    /// The output would not be well-formed, or a limit was reached.
    Xml(Error),
    /// The sink failed.
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
