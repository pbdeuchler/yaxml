//! XML Schema 1.0 validation after libxml2 2.15.4's `xmlschemas.c`.
//!
//! [`Schema::parse`] reads a schema document (`xmlSchemaParse`) and the
//! documents it includes, imports or redefines through an optional
//! [`Resolver`]; [`Schema::validate`] checks a [`Document`](crate::Document)
//! or [`OwnedDocument`] against it (`xmlSchemaValidateDoc`). A schema is
//! immutable, `Send` and `Sync`, and validates any number of documents.
//!
//! ```
//! use yaxml::{Document, xsd::{Code, Schema}};
//!
//! let schema = Schema::parse(br#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
//!   <xs:element name="r"><xs:complexType><xs:sequence>
//!     <xs:element name="n" type="xs:int" maxOccurs="3"/>
//!   </xs:sequence></xs:complexType></xs:element>
//! </xs:schema>"#, "r.xsd")?;
//! let document = Document::parse("<r><n>1</n><n>x</n></r>")?;
//! let diagnostics = schema.validate(&document);
//! assert_eq!(diagnostics[0].code, Code::CvcDatatypeValid1_2_1);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! Validity errors are [`Diagnostic`]s; a schema that fails to build is an
//! [`Error`] listing [`SchemaDiagnostic`]s. Codes carry libxml2's
//! `XML_SCHEMAP_*` and `XML_SCHEMAV_*` numbers. See `docs/xsd.md`.

mod components;
mod fixup;
mod idc;
mod model;
mod parse;
mod simple;
mod validate;

use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::Resolver;
use crate::xpath::Tree;

use components::{
    AttrId, AttributeDecl, ElemId, ElementDecl, Idc, Names, QName, TypeDef, TypeId, Wildcard,
};

/// A schema or validity diagnostic code; the discriminant is libxml2's
/// `xmlParserErrors` value (`XML_SCHEMAP_*` 1700..=3091, `XML_SCHEMAV_*`
/// 1801..=1879).
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(u16)]
#[non_exhaustive]
#[allow(missing_docs)]
pub enum Code {
    AttrFormDefaultValue = 1701,
    ElemFormDefaultValue = 1705,
    FacetNoValue = 1708,
    InvalidBoolean = 1714,
    InvalidFacetValue = 1717,
    NotationNoName = 1723,
    UnknownFacetChild = 1740,
    RegexpInvalid = 1756,
    FailedLoad = 1757,
    NoRoot = 1759,
    /// A global component of the same kind and name already exists.
    RedefinedType = 1761,
    NotSchema = 1772,
    InvalidAttrUse = 1774,
    DerivationOkRestriction1 = 1787,
    DerivationOkRestriction2_1_1 = 1788,
    DerivationOkRestriction2_1_2 = 1789,
    DerivationOkRestriction2_2 = 1790,
    DerivationOkRestriction3 = 1791,
    WildcardInvalidNsMember = 1792,
    IntersectionNotExpressible = 1793,
    UnionNotExpressible = 1794,
    DerivationOkRestriction4_1 = 1797,
    DerivationOkRestriction4_2 = 1798,
    DerivationOkRestriction4_3 = 1799,
    CosCtExtends1_3 = 1800,
    /// A validator internal error, such as a content model too complex to match.
    VInternal = 1818,
    CvcDatatypeValid1_2_1 = 1824,
    CvcDatatypeValid1_2_2 = 1825,
    CvcDatatypeValid1_2_3 = 1826,
    CvcType3_1_1 = 1827,
    CvcType3_1_2 = 1828,
    CvcLengthValid = 1830,
    CvcMinLengthValid = 1831,
    CvcMaxLengthValid = 1832,
    CvcMinInclusiveValid = 1833,
    CvcMaxInclusiveValid = 1834,
    CvcMinExclusiveValid = 1835,
    CvcMaxExclusiveValid = 1836,
    CvcTotalDigitsValid = 1837,
    CvcFractionDigitsValid = 1838,
    CvcPatternValid = 1839,
    CvcEnumerationValid = 1840,
    CvcComplexType2_1 = 1841,
    CvcComplexType2_2 = 1842,
    CvcComplexType2_3 = 1843,
    CvcElt1 = 1845,
    CvcElt2 = 1846,
    CvcElt3_1 = 1847,
    CvcElt3_2_1 = 1848,
    CvcElt3_2_2 = 1849,
    CvcElt4_2 = 1851,
    CvcElt4_3 = 1852,
    CvcElt5_2_2_1 = 1856,
    CvcElt5_2_2_2_1 = 1857,
    CvcElt5_2_2_2_2 = 1858,
    CvcAttribute2 = 1862,
    CvcComplexType3_2_1 = 1866,
    CvcComplexType3_2_2 = 1867,
    CvcComplexType4 = 1868,
    /// A child element is not expected, or required children are missing.
    ElementContent = 1871,
    DocumentElementMissing = 1872,
    CvcAu = 1874,
    CvcType1 = 1875,
    CvcType2 = 1876,
    CvcIdc = 1877,
    CvcWildcard = 1878,
    Misc = 1879,
    SrcSimpleType1 = 3000,
    SrcSimpleType4 = 3003,
    /// A QName does not resolve to a component of the expected kind.
    SrcResolve = 3004,
    SrcRestrictionBaseOrSimpleType = 3005,
    SrcUnionMemberTypesOrSimpleTypes = 3007,
    StPropsCorrect1 = 3008,
    StPropsCorrect2 = 3009,
    StPropsCorrect3 = 3010,
    CosStRestricts1_1 = 3011,
    CosStRestricts1_2 = 3012,
    CosStRestricts1_3_1 = 3013,
    CosStRestricts2_1 = 3015,
    CosStRestricts2_3_1_1 = 3016,
    CosStRestricts2_3_1_2 = 3017,
    CosStRestricts2_3_2_1 = 3018,
    CosStRestricts2_3_2_2 = 3019,
    CosStRestricts2_3_2_3 = 3020,
    CosStRestricts2_3_2_4 = 3021,
    CosStRestricts3_1 = 3023,
    CosStRestricts3_3_1 = 3024,
    CosStRestricts3_3_1_2 = 3025,
    CosStRestricts3_3_2_2 = 3026,
    CosStRestricts3_3_2_1 = 3027,
    CosStRestricts3_3_2_3 = 3028,
    CosStRestricts3_3_2_4 = 3029,
    CosStDerivedOk2_1 = 3031,
    CosStDerivedOk2_2 = 3032,
    /// An element where the schema for schemas allows none.
    S4sElemNotAllowed = 3033,
    S4sElemMissing = 3034,
    S4sAttrNotAllowed = 3035,
    S4sAttrMissing = 3036,
    S4sAttrInvalidValue = 3037,
    SrcElement1 = 3038,
    SrcElement2_1 = 3039,
    SrcElement2_2 = 3040,
    SrcElement3 = 3041,
    PPropsCorrect2_1 = 3043,
    PPropsCorrect2_2 = 3044,
    EPropsCorrect3 = 3046,
    EPropsCorrect4 = 3047,
    EPropsCorrect5 = 3048,
    EPropsCorrect6 = 3049,
    SrcInclude = 3050,
    SrcAttribute1 = 3051,
    SrcAttribute2 = 3052,
    SrcAttribute3_2 = 3054,
    SrcAttribute4 = 3055,
    NoXmlns = 3056,
    NoXsi = 3057,
    CosValidDefault2_1 = 3059,
    CosCtExtends1_1 = 3063,
    SrcImport1_1 = 3064,
    SrcImport1_2 = 3065,
    SrcImport2_1 = 3067,
    /// A schema construction failure, such as an exhausted budget.
    Internal = 3069,
    /// The content model violates Unique Particle Attribution.
    NotDeterministic = 3070,
    SrcAttributeGroup3 = 3073,
    MgPropsCorrect2 = 3075,
    SrcCt1 = 3076,
    DerivationOkRestriction2_1_3 = 3077,
    AuPropsCorrect2 = 3078,
    APropsCorrect2 = 3079,
    CPropsCorrect = 3080,
    SrcRedefine = 3081,
    SrcImport = 3082,
    WarnSkipSchema = 3083,
    WarnUnlocatedSchema = 3084,
    WarnAttrPointlessProh = 3086,
    AgPropsCorrect = 3087,
    CosCtExtends1_2 = 3088,
    AuPropsCorrect = 3089,
    APropsCorrect3 = 3090,
    CosAllLimited = 3091,
}

impl Code {
    /// libxml2 reports these as warnings; they fail neither schema
    /// construction nor validation.
    pub fn is_warning(self) -> bool {
        matches!(
            self,
            Self::WarnSkipSchema
                | Self::WarnUnlocatedSchema
                | Self::WarnAttrPointlessProh
                | Self::Misc
        )
    }
}

/// A schema construction diagnostic in schema document `uri`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaDiagnostic {
    pub code: Code,
    pub uri: String,
    /// The start of the offending schema element, when known.
    pub offset: Option<usize>,
    /// A libxml2-style description.
    pub message: String,
}

impl fmt::Display for SchemaDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.uri)?;
        if let Some(offset) = self.offset {
            write!(f, ":{offset}")?;
        }
        write!(f, ": {}", self.message)
    }
}

/// A schema could not be built; `diagnostics` holds the errors and warnings
/// in report order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Error {
    pub diagnostics: Vec<SchemaDiagnostic>,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.diagnostics.iter().find(|d| !d.code.is_warning()) {
            Some(first) => write!(f, "invalid schema: {first}"),
            None => f.write_str("invalid schema"),
        }
    }
}

impl std::error::Error for Error {}

/// One validity error or warning.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic<Id> {
    pub code: Code,
    /// The element concerned (an attribute's owner element).
    pub node: Option<Id>,
    /// The start of `node`'s source range, when it has one.
    pub offset: Option<usize>,
    /// A libxml2-style description.
    pub message: String,
}

/// True when no diagnostic is an error.
pub fn is_valid<Id>(diagnostics: &[Diagnostic<Id>]) -> bool {
    diagnostics.iter().all(|d| d.code.is_warning())
}

/// Ceilings on schema construction and validation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    /// Limits for parsing each schema document.
    pub xml: crate::Limits,
    /// Schema documents requested from the resolver.
    pub max_documents: usize,
    /// Cumulative bytes the resolver may return.
    pub max_bytes: usize,
    /// Nesting of particles through model groups and group references.
    pub max_depth: usize,
    /// Work units for compiling all content models (positions, follow-set
    /// entries and determinism comparisons).
    pub max_model_work: usize,
    /// Counter vectors one element's content may track; only nested
    /// repetitions like `(a{2,3}){2}` need more than one.
    pub max_counter_vectors: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            xml: crate::Limits::default(),
            max_documents: 256,
            max_bytes: 64 << 20,
            max_depth: 256,
            max_model_work: 1 << 24,
            max_counter_vectors: 1024,
        }
    }
}

/// A compiled XML Schema: immutable, reusable and thread-safe.
#[derive(Clone, Debug)]
pub struct Schema {
    pub(crate) names: Names,
    pub(crate) types: Vec<TypeDef>,
    pub(crate) elements: Vec<ElementDecl>,
    pub(crate) attributes: Vec<AttributeDecl>,
    pub(crate) wildcards: Vec<Wildcard>,
    pub(crate) idcs: Vec<Idc>,
    pub(crate) global_elements: HashMap<QName, ElemId>,
    pub(crate) global_types: HashMap<QName, TypeId>,
    pub(crate) global_attributes: HashMap<QName, AttrId>,
    pub(crate) notations: HashSet<QName>,
    pub(crate) warnings: Vec<SchemaDiagnostic>,
    pub(crate) max_counter_vectors: usize,
}

impl Schema {
    /// Parses a schema document whose URI is `uri`; nothing else is loaded,
    /// so `<include>` fails and `<import>` is skipped.
    pub fn parse(input: &[u8], uri: &str) -> Result<Schema, Error> {
        Self::with_resolver(input, uri, None, Limits::default())
    }

    /// Parses a schema document, loading the documents it includes,
    /// imports or redefines through `resolver` within `limits`
    /// (`ResourceKind::Schema` requests).
    pub fn with_resolver(
        input: &[u8],
        uri: &str,
        resolver: Option<Box<dyn Resolver>>,
        limits: Limits,
    ) -> Result<Schema, Error> {
        let mut parser = parse::Parser::new(resolver, limits);
        parser.run(input, uri);
        fixup::build(parser.drafts, &limits)
    }

    /// Builds a schema from an instance's `xsi:schemaLocation` and
    /// `xsi:noNamespaceSchemaLocation` hints, in document order, loading
    /// each through `resolver` (libxml2's validation without a schema).
    pub fn from_instance<T: Tree>(
        tree: &T,
        uri: &str,
        resolver: Box<dyn Resolver>,
        limits: Limits,
    ) -> Result<Schema, Error> {
        let mut parser = parse::Parser::new(Some(resolver), limits);
        for (namespace, location) in validate::schema_locations(tree) {
            let location = crate::uri::resolve(&location, uri).unwrap_or(location);
            parser.add_import(namespace.as_deref(), &location);
        }
        fixup::build(parser.drafts, &limits)
    }

    /// Warnings reported while building the schema.
    pub fn warnings(&self) -> &[SchemaDiagnostic] {
        &self.warnings
    }

    /// Validates a document (`xmlSchemaValidateDoc`).
    pub fn validate<T: Tree>(&self, tree: &T) -> Vec<Diagnostic<T::Id>> {
        validate::run(self, tree)
    }
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Schema>();
};
