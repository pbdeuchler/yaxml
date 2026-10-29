//! XML Schema 1.0 validation after libxml2 2.15.4's `xmlschemas.c`.
//!
//! [`Schema::parse`] reads a schema document (`xmlSchemaParse`) and the
//! documents it includes, imports or redefines through an optional
//! [`Resolver`]; [`Schema::validate`] checks a [`Document`](crate::Document)
//! or [`OwnedDocument`](crate::OwnedDocument) against it
//! (`xmlSchemaValidateDoc`); [`Schema::validate_stream`] and
//! [`StreamValidator`] check parser events without a tree
//! (`xmlSchemaValidateStream`, `xmlSchemaSAXPlug`). A schema is immutable,
//! `Send` and `Sync`, and validates any number of documents.
//!
//! ```
//! use yaxml::{Document, xsd::{self, Code, Schema}};
//!
//! let schema = Schema::parse(br#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
//!   <xs:element name="r"><xs:complexType><xs:sequence>
//!     <xs:element name="n" type="xs:int" maxOccurs="3"/>
//!   </xs:sequence></xs:complexType></xs:element>
//! </xs:schema>"#, "r.xsd")?;
//! let document = Document::parse("<r><n>1</n><n>x</n></r>")?;
//! let diagnostics = schema.validate(&document);
//! assert!(!xsd::is_valid(&diagnostics));
//! assert_eq!(diagnostics[0].code, Code::CvcDatatypeValid1_2_1);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! Validity errors are [`Diagnostic`]s; a schema that fails to build is an
//! [`Error`] listing [`SchemaDiagnostic`]s. Codes carry libxml2's
//! `XML_SCHEMAP_*` and `XML_SCHEMAV_*` numbers. Differences from libxml2:
//! <https://github.com/pbdeuchler/yaxml/blob/main/docs/xsd.md>.

mod automaton;
mod components;
mod fixup;
mod idc;
mod model;
mod parse;
mod simple;
mod validate;
mod view;

pub use components::IdcKind;
pub use parse::Compositor;
pub use view::{
    Annotations, AttributeDeclaration, AttributeGroup, AttributeUse, Constraint, ContentType,
    Derivations, ElementDeclaration, IdentityConstraint, ModelGroup, ModelGroupDefinition,
    Namespaces, Particle, ProcessContents, SimpleVariety, Term, TypeDefinition, TypeKind, Wildcard,
};

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io::BufRead;
use std::sync::Arc;

use crate::valid::Dtd;
use crate::xpath::Tree;
use crate::xpath::tree::QName as NodeName;
use crate::{
    BufferedReader, EditError, Editor, Event, NameSpec, OwnedNodeId, ParseOptions, PrefixPolicy,
    ReadError, Resolver, SourceOrigin,
};

use components::{
    AttrGroup, AttrId, AttributeDecl, ElemId, ElementDecl, Idc, Names, QName, Sym, TypeDef, TypeId,
};

/// A schema or validity diagnostic code; the discriminant is libxml2's
/// `xmlParserErrors` value (`XML_SCHEMAP_*` 1700..=3091, `XML_SCHEMAV_*`
/// 1801..=1879, `XML_ERR_NO_MEMORY` 2).
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(u16)]
#[non_exhaustive]
pub enum Code {
    /// An allocation the schema or document sized failed; building or
    /// validation stopped.
    Allocation = 2,
    /// `XML_SCHEMAP_ATTRFORMDEFAULT_VALUE`.
    AttrFormDefaultValue = 1701,
    /// `XML_SCHEMAP_ELEMFORMDEFAULT_VALUE`.
    ElemFormDefaultValue = 1705,
    /// `XML_SCHEMAP_FACET_NO_VALUE`.
    FacetNoValue = 1708,
    /// `XML_SCHEMAP_INVALID_BOOLEAN`.
    InvalidBoolean = 1714,
    /// `XML_SCHEMAP_INVALID_FACET_VALUE`.
    InvalidFacetValue = 1717,
    /// `XML_SCHEMAP_NOTATION_NO_NAME`.
    NotationNoName = 1723,
    /// `XML_SCHEMAP_UNKNOWN_FACET_CHILD`.
    UnknownFacetChild = 1740,
    /// `XML_SCHEMAP_REGEXP_INVALID`.
    RegexpInvalid = 1756,
    /// `XML_SCHEMAP_FAILED_LOAD`.
    FailedLoad = 1757,
    /// `XML_SCHEMAP_NOROOT`.
    NoRoot = 1759,
    /// A global component of the same kind and name already exists (`XML_SCHEMAP_REDEFINED_TYPE`).
    RedefinedType = 1761,
    /// `XML_SCHEMAP_NOT_SCHEMA`.
    NotSchema = 1772,
    /// `XML_SCHEMAP_INVALID_ATTR_USE`.
    InvalidAttrUse = 1774,
    /// `XML_SCHEMAP_DERIVATION_OK_RESTRICTION_1`.
    DerivationOkRestriction1 = 1787,
    /// `XML_SCHEMAP_DERIVATION_OK_RESTRICTION_2_1_1`.
    DerivationOkRestriction2_1_1 = 1788,
    /// `XML_SCHEMAP_DERIVATION_OK_RESTRICTION_2_1_2`.
    DerivationOkRestriction2_1_2 = 1789,
    /// `XML_SCHEMAP_DERIVATION_OK_RESTRICTION_2_2`.
    DerivationOkRestriction2_2 = 1790,
    /// `XML_SCHEMAP_DERIVATION_OK_RESTRICTION_3`.
    DerivationOkRestriction3 = 1791,
    /// `XML_SCHEMAP_WILDCARD_INVALID_NS_MEMBER`.
    WildcardInvalidNsMember = 1792,
    /// `XML_SCHEMAP_INTERSECTION_NOT_EXPRESSIBLE`.
    IntersectionNotExpressible = 1793,
    /// `XML_SCHEMAP_UNION_NOT_EXPRESSIBLE`.
    UnionNotExpressible = 1794,
    /// `XML_SCHEMAP_DERIVATION_OK_RESTRICTION_4_1`.
    DerivationOkRestriction4_1 = 1797,
    /// `XML_SCHEMAP_DERIVATION_OK_RESTRICTION_4_2`.
    DerivationOkRestriction4_2 = 1798,
    /// `XML_SCHEMAP_DERIVATION_OK_RESTRICTION_4_3`.
    DerivationOkRestriction4_3 = 1799,
    /// `XML_SCHEMAP_COS_CT_EXTENDS_1_3`.
    CosCtExtends1_3 = 1800,
    /// A validator internal error, such as a content model too complex to match (`XML_SCHEMAV_INTERNAL`).
    VInternal = 1818,
    /// `XML_SCHEMAV_CVC_DATATYPE_VALID_1_2_1`.
    CvcDatatypeValid1_2_1 = 1824,
    /// `XML_SCHEMAV_CVC_DATATYPE_VALID_1_2_2`.
    CvcDatatypeValid1_2_2 = 1825,
    /// `XML_SCHEMAV_CVC_DATATYPE_VALID_1_2_3`.
    CvcDatatypeValid1_2_3 = 1826,
    /// `XML_SCHEMAV_CVC_TYPE_3_1_1`.
    CvcType3_1_1 = 1827,
    /// `XML_SCHEMAV_CVC_TYPE_3_1_2`.
    CvcType3_1_2 = 1828,
    /// `XML_SCHEMAV_CVC_LENGTH_VALID`.
    CvcLengthValid = 1830,
    /// `XML_SCHEMAV_CVC_MINLENGTH_VALID`.
    CvcMinLengthValid = 1831,
    /// `XML_SCHEMAV_CVC_MAXLENGTH_VALID`.
    CvcMaxLengthValid = 1832,
    /// `XML_SCHEMAV_CVC_MININCLUSIVE_VALID`.
    CvcMinInclusiveValid = 1833,
    /// `XML_SCHEMAV_CVC_MAXINCLUSIVE_VALID`.
    CvcMaxInclusiveValid = 1834,
    /// `XML_SCHEMAV_CVC_MINEXCLUSIVE_VALID`.
    CvcMinExclusiveValid = 1835,
    /// `XML_SCHEMAV_CVC_MAXEXCLUSIVE_VALID`.
    CvcMaxExclusiveValid = 1836,
    /// `XML_SCHEMAV_CVC_TOTALDIGITS_VALID`.
    CvcTotalDigitsValid = 1837,
    /// `XML_SCHEMAV_CVC_FRACTIONDIGITS_VALID`.
    CvcFractionDigitsValid = 1838,
    /// `XML_SCHEMAV_CVC_PATTERN_VALID`.
    CvcPatternValid = 1839,
    /// `XML_SCHEMAV_CVC_ENUMERATION_VALID`.
    CvcEnumerationValid = 1840,
    /// `XML_SCHEMAV_CVC_COMPLEX_TYPE_2_1`.
    CvcComplexType2_1 = 1841,
    /// `XML_SCHEMAV_CVC_COMPLEX_TYPE_2_2`.
    CvcComplexType2_2 = 1842,
    /// `XML_SCHEMAV_CVC_COMPLEX_TYPE_2_3`.
    CvcComplexType2_3 = 1843,
    /// `XML_SCHEMAV_CVC_ELT_1`.
    CvcElt1 = 1845,
    /// `XML_SCHEMAV_CVC_ELT_2`.
    CvcElt2 = 1846,
    /// `XML_SCHEMAV_CVC_ELT_3_1`.
    CvcElt3_1 = 1847,
    /// `XML_SCHEMAV_CVC_ELT_3_2_1`.
    CvcElt3_2_1 = 1848,
    /// `XML_SCHEMAV_CVC_ELT_3_2_2`.
    CvcElt3_2_2 = 1849,
    /// `XML_SCHEMAV_CVC_ELT_4_2`.
    CvcElt4_2 = 1851,
    /// `XML_SCHEMAV_CVC_ELT_4_3`.
    CvcElt4_3 = 1852,
    /// `XML_SCHEMAV_CVC_ELT_5_2_2_1`.
    CvcElt5_2_2_1 = 1856,
    /// `XML_SCHEMAV_CVC_ELT_5_2_2_2_1`.
    CvcElt5_2_2_2_1 = 1857,
    /// `XML_SCHEMAV_CVC_ELT_5_2_2_2_2`.
    CvcElt5_2_2_2_2 = 1858,
    /// `XML_SCHEMAV_CVC_ATTRIBUTE_2`.
    CvcAttribute2 = 1862,
    /// `XML_SCHEMAV_CVC_COMPLEX_TYPE_3_2_1`.
    CvcComplexType3_2_1 = 1866,
    /// `XML_SCHEMAV_CVC_COMPLEX_TYPE_3_2_2`.
    CvcComplexType3_2_2 = 1867,
    /// `XML_SCHEMAV_CVC_COMPLEX_TYPE_4`.
    CvcComplexType4 = 1868,
    /// A child element is not expected, or required children are missing (`XML_SCHEMAV_ELEMENT_CONTENT`).
    ElementContent = 1871,
    /// `XML_SCHEMAV_DOCUMENT_ELEMENT_MISSING`.
    DocumentElementMissing = 1872,
    /// `XML_SCHEMAV_CVC_AU`.
    CvcAu = 1874,
    /// `XML_SCHEMAV_CVC_TYPE_1`.
    CvcType1 = 1875,
    /// `XML_SCHEMAV_CVC_TYPE_2`.
    CvcType2 = 1876,
    /// `XML_SCHEMAV_CVC_IDC`.
    CvcIdc = 1877,
    /// `XML_SCHEMAV_CVC_WILDCARD`.
    CvcWildcard = 1878,
    /// `XML_SCHEMAV_MISC`.
    Misc = 1879,
    /// `XML_SCHEMAP_SRC_SIMPLE_TYPE_1`.
    SrcSimpleType1 = 3000,
    /// `XML_SCHEMAP_SRC_SIMPLE_TYPE_4`.
    SrcSimpleType4 = 3003,
    /// A QName does not resolve to a component of the expected kind (`XML_SCHEMAP_SRC_RESOLVE`).
    SrcResolve = 3004,
    /// `XML_SCHEMAP_SRC_RESTRICTION_BASE_OR_SIMPLETYPE`.
    SrcRestrictionBaseOrSimpleType = 3005,
    /// `XML_SCHEMAP_SRC_UNION_MEMBERTYPES_OR_SIMPLETYPES`.
    SrcUnionMemberTypesOrSimpleTypes = 3007,
    /// `XML_SCHEMAP_ST_PROPS_CORRECT_1`.
    StPropsCorrect1 = 3008,
    /// `XML_SCHEMAP_ST_PROPS_CORRECT_2`.
    StPropsCorrect2 = 3009,
    /// `XML_SCHEMAP_ST_PROPS_CORRECT_3`.
    StPropsCorrect3 = 3010,
    /// `XML_SCHEMAP_COS_ST_RESTRICTS_1_1`.
    CosStRestricts1_1 = 3011,
    /// `XML_SCHEMAP_COS_ST_RESTRICTS_1_2`.
    CosStRestricts1_2 = 3012,
    /// `XML_SCHEMAP_COS_ST_RESTRICTS_1_3_1`.
    CosStRestricts1_3_1 = 3013,
    /// `XML_SCHEMAP_COS_ST_RESTRICTS_2_1`.
    CosStRestricts2_1 = 3015,
    /// `XML_SCHEMAP_COS_ST_RESTRICTS_2_3_1_1`.
    CosStRestricts2_3_1_1 = 3016,
    /// `XML_SCHEMAP_COS_ST_RESTRICTS_2_3_1_2`.
    CosStRestricts2_3_1_2 = 3017,
    /// `XML_SCHEMAP_COS_ST_RESTRICTS_2_3_2_1`.
    CosStRestricts2_3_2_1 = 3018,
    /// `XML_SCHEMAP_COS_ST_RESTRICTS_2_3_2_2`.
    CosStRestricts2_3_2_2 = 3019,
    /// `XML_SCHEMAP_COS_ST_RESTRICTS_2_3_2_3`.
    CosStRestricts2_3_2_3 = 3020,
    /// `XML_SCHEMAP_COS_ST_RESTRICTS_2_3_2_4`.
    CosStRestricts2_3_2_4 = 3021,
    /// `XML_SCHEMAP_COS_ST_RESTRICTS_3_1`.
    CosStRestricts3_1 = 3023,
    /// `XML_SCHEMAP_COS_ST_RESTRICTS_3_3_1`.
    CosStRestricts3_3_1 = 3024,
    /// `XML_SCHEMAP_COS_ST_RESTRICTS_3_3_1_2`.
    CosStRestricts3_3_1_2 = 3025,
    /// `XML_SCHEMAP_COS_ST_RESTRICTS_3_3_2_2`.
    CosStRestricts3_3_2_2 = 3026,
    /// `XML_SCHEMAP_COS_ST_RESTRICTS_3_3_2_1`.
    CosStRestricts3_3_2_1 = 3027,
    /// `XML_SCHEMAP_COS_ST_RESTRICTS_3_3_2_3`.
    CosStRestricts3_3_2_3 = 3028,
    /// `XML_SCHEMAP_COS_ST_RESTRICTS_3_3_2_4`.
    CosStRestricts3_3_2_4 = 3029,
    /// `XML_SCHEMAP_COS_ST_DERIVED_OK_2_1`.
    CosStDerivedOk2_1 = 3031,
    /// `XML_SCHEMAP_COS_ST_DERIVED_OK_2_2`.
    CosStDerivedOk2_2 = 3032,
    /// An element where the schema for schemas allows none (`XML_SCHEMAP_S4S_ELEM_NOT_ALLOWED`).
    S4sElemNotAllowed = 3033,
    /// `XML_SCHEMAP_S4S_ELEM_MISSING`.
    S4sElemMissing = 3034,
    /// `XML_SCHEMAP_S4S_ATTR_NOT_ALLOWED`.
    S4sAttrNotAllowed = 3035,
    /// `XML_SCHEMAP_S4S_ATTR_MISSING`.
    S4sAttrMissing = 3036,
    /// `XML_SCHEMAP_S4S_ATTR_INVALID_VALUE`.
    S4sAttrInvalidValue = 3037,
    /// `XML_SCHEMAP_SRC_ELEMENT_1`.
    SrcElement1 = 3038,
    /// `XML_SCHEMAP_SRC_ELEMENT_2_1`.
    SrcElement2_1 = 3039,
    /// `XML_SCHEMAP_SRC_ELEMENT_2_2`.
    SrcElement2_2 = 3040,
    /// `XML_SCHEMAP_SRC_ELEMENT_3`.
    SrcElement3 = 3041,
    /// `XML_SCHEMAP_P_PROPS_CORRECT_2_1`.
    PPropsCorrect2_1 = 3043,
    /// `XML_SCHEMAP_P_PROPS_CORRECT_2_2`.
    PPropsCorrect2_2 = 3044,
    /// `XML_SCHEMAP_E_PROPS_CORRECT_3`.
    EPropsCorrect3 = 3046,
    /// `XML_SCHEMAP_E_PROPS_CORRECT_4`.
    EPropsCorrect4 = 3047,
    /// `XML_SCHEMAP_E_PROPS_CORRECT_5`.
    EPropsCorrect5 = 3048,
    /// `XML_SCHEMAP_E_PROPS_CORRECT_6`.
    EPropsCorrect6 = 3049,
    /// `XML_SCHEMAP_SRC_INCLUDE`.
    SrcInclude = 3050,
    /// `XML_SCHEMAP_SRC_ATTRIBUTE_1`.
    SrcAttribute1 = 3051,
    /// `XML_SCHEMAP_SRC_ATTRIBUTE_2`.
    SrcAttribute2 = 3052,
    /// `XML_SCHEMAP_SRC_ATTRIBUTE_3_2`.
    SrcAttribute3_2 = 3054,
    /// `XML_SCHEMAP_SRC_ATTRIBUTE_4`.
    SrcAttribute4 = 3055,
    /// `XML_SCHEMAP_NO_XMLNS`.
    NoXmlns = 3056,
    /// `XML_SCHEMAP_NO_XSI`.
    NoXsi = 3057,
    /// `XML_SCHEMAP_COS_VALID_DEFAULT_2_1`.
    CosValidDefault2_1 = 3059,
    /// `XML_SCHEMAP_COS_CT_EXTENDS_1_1`.
    CosCtExtends1_1 = 3063,
    /// `XML_SCHEMAP_SRC_IMPORT_1_1`.
    SrcImport1_1 = 3064,
    /// `XML_SCHEMAP_SRC_IMPORT_1_2`.
    SrcImport1_2 = 3065,
    /// `XML_SCHEMAP_SRC_IMPORT_2_1`.
    SrcImport2_1 = 3067,
    /// A schema construction failure, such as an exhausted budget (`XML_SCHEMAP_INTERNAL`).
    Internal = 3069,
    /// The content model violates Unique Particle Attribution (`XML_SCHEMAP_NOT_DETERMINISTIC`).
    NotDeterministic = 3070,
    /// `XML_SCHEMAP_SRC_ATTRIBUTE_GROUP_3`.
    SrcAttributeGroup3 = 3073,
    /// `XML_SCHEMAP_MG_PROPS_CORRECT_2`.
    MgPropsCorrect2 = 3075,
    /// `XML_SCHEMAP_SRC_CT_1`.
    SrcCt1 = 3076,
    /// `XML_SCHEMAP_DERIVATION_OK_RESTRICTION_2_1_3`.
    DerivationOkRestriction2_1_3 = 3077,
    /// `XML_SCHEMAP_AU_PROPS_CORRECT_2`.
    AuPropsCorrect2 = 3078,
    /// `XML_SCHEMAP_A_PROPS_CORRECT_2`.
    APropsCorrect2 = 3079,
    /// `XML_SCHEMAP_C_PROPS_CORRECT`.
    CPropsCorrect = 3080,
    /// `XML_SCHEMAP_SRC_REDEFINE`.
    SrcRedefine = 3081,
    /// `XML_SCHEMAP_SRC_IMPORT`.
    SrcImport = 3082,
    /// `XML_SCHEMAP_WARN_SKIP_SCHEMA`.
    WarnSkipSchema = 3083,
    /// `XML_SCHEMAP_WARN_UNLOCATED_SCHEMA`.
    WarnUnlocatedSchema = 3084,
    /// `XML_SCHEMAP_WARN_ATTR_POINTLESS_PROH`.
    WarnAttrPointlessProh = 3086,
    /// `XML_SCHEMAP_AG_PROPS_CORRECT`.
    AgPropsCorrect = 3087,
    /// `XML_SCHEMAP_COS_CT_EXTENDS_1_2`.
    CosCtExtends1_2 = 3088,
    /// `XML_SCHEMAP_AU_PROPS_CORRECT`.
    AuPropsCorrect = 3089,
    /// `XML_SCHEMAP_A_PROPS_CORRECT_3`.
    APropsCorrect3 = 3090,
    /// `XML_SCHEMAP_COS_ALL_LIMITED`.
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
    /// What went wrong.
    pub code: Code,
    /// The schema document holding the problem.
    pub uri: String,
    /// The start of the offending schema element, when known.
    pub offset: Option<usize>,
    /// A libxml2-style description.
    pub message: String,
}

impl fmt::Display for SchemaDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let at = (self.offset).map_or_else(String::new, |offset| format!(":{offset}"));
        write!(f, "{}{at}: {}", self.uri, self.message)
    }
}

/// A schema that could not be built.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Error {
    /// Errors and warnings, in report order.
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
    /// What went wrong.
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
#[non_exhaustive]
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
    /// Bytes the compiled content models and the automaton compiling the
    /// current one may hold, counted by entry size (allocator slack comes
    /// on top). Transitions can be quadratic in the particles: compiling
    /// a sequence of `n` optional elements holds about 22·n² bytes, so the
    /// default stops at n = 1,741. Exceeding it is reported like
    /// `max_model_work`.
    pub max_model_bytes: usize,
    /// Counter vectors one element's content may track; only nested
    /// repetitions like `(a{2,3}){2}` need more than one.
    pub max_counter_vectors: usize,
    /// Key sequences one validation may keep for identity constraints;
    /// beyond it validation reports `XML_SCHEMAV_INTERNAL` and stops
    /// checking them.
    pub max_idc_keys: usize,
}

setters!(const Limits { xml: crate::Limits, max_documents: usize, max_bytes: usize, max_depth: usize, max_model_work: usize, max_model_bytes: usize, max_counter_vectors: usize, max_idc_keys: usize });

impl Limits {
    /// The defaults: 256 documents, 64 MiB, depth 256, 16 Mi model work
    /// units, 64 MiB of model memory, 1024 counter vectors and 1 Mi
    /// identity-constraint keys.
    pub const DEFAULT: Self = Self {
        xml: crate::Limits::DEFAULT,
        max_documents: 256,
        max_bytes: 64 << 20,
        max_depth: 256,
        max_model_work: 1 << 24,
        max_model_bytes: 64 << 20,
        max_counter_vectors: 1024,
        max_idc_keys: 1 << 20,
    };
}

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// A compiled XML Schema: immutable, reusable and thread-safe. Cloning shares
/// the compiled data, which is how validators and readers keep a schema alive.
#[derive(Clone, Debug)]
pub struct Schema {
    data: Arc<Compiled>,
}

/// The compiled components behind a [`Schema`].
#[derive(Debug)]
pub(crate) struct Compiled {
    pub(crate) names: Names,
    /// The main document's target namespace.
    pub(crate) target: Sym,
    pub(crate) types: Vec<TypeDef>,
    pub(crate) elements: Vec<ElementDecl>,
    pub(crate) attributes: Vec<AttributeDecl>,
    pub(crate) wildcards: Vec<components::Wildcard>,
    pub(crate) idcs: Vec<Idc>,
    pub(crate) global_idcs: HashMap<QName, usize>,
    /// Effective content particles by type.
    pub(crate) particles: HashMap<TypeId, parse::Particle>,
    /// Model group definitions, references resolved.
    pub(crate) groups: Vec<parse::GroupDraft>,
    pub(crate) global_groups: HashMap<QName, usize>,
    pub(crate) global_elements: HashMap<QName, ElemId>,
    pub(crate) global_types: HashMap<QName, TypeId>,
    pub(crate) global_attributes: HashMap<QName, AttrId>,
    pub(crate) attribute_groups: Vec<AttrGroup>,
    pub(crate) global_attribute_groups: HashMap<QName, usize>,
    pub(crate) notations: HashSet<QName>,
    pub(crate) warnings: Vec<SchemaDiagnostic>,
    pub(crate) max_counter_vectors: usize,
    pub(crate) max_idc_keys: usize,
    /// `None` without annotations.
    pub(crate) notes: Option<Notes>,
}

/// The annotations, copied out of the schema documents.
#[derive(Debug)]
pub(crate) struct Notes {
    pub(crate) doc: crate::OwnedDocument,
    pub(crate) owners: HashMap<(parse::Loc, parse::Annotated), Vec<OwnedNodeId>>,
    /// `_xmlSchema.annot`.
    pub(crate) schema: Option<OwnedNodeId>,
}

impl Schema {
    /// Parses a schema document whose URI is `uri`; nothing else is loaded,
    /// so `<include>` and `<redefine>` fail and `<import>` is skipped.
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
        &self.data.warnings
    }

    /// Validates a document (`xmlSchemaValidateDoc`).
    pub fn validate<T: Tree>(&self, tree: &T) -> Vec<Diagnostic<T::Id>> {
        validate::run(self, tree, None, false).0
    }

    /// Validates the editor's document like [`validate`](Self::validate) and
    /// then adds the defaults the schema supplies, as libxml2 does with
    /// `XML_SCHEMA_VAL_VC_I_CREATE`: each defaulted attribute (value
    /// normalized by its type) and the default value of each empty,
    /// non-nilled element as a text child. A namespaced attribute takes the
    /// innermost prefix bound to its URI, else `p0`, `p1`, ... declared on
    /// the document element. An edit error stops with the earlier defaults
    /// already added.
    pub fn validate_creating(
        &self,
        editor: &mut Editor<'_>,
    ) -> Result<Vec<Diagnostic<OwnedNodeId>>, EditError> {
        let (diagnostics, created) = validate::run(self, editor.document(), None, true);
        for validate::Created {
            element,
            attribute,
            value,
        } in created
        {
            let Some((namespace, local)) = attribute else {
                editor.append_text(element, &value)?;
                continue;
            };
            let node = editor.document().node(element).expect("validated element");
            let prefix = match namespace.as_deref() {
                None => None,
                Some(uri) => match node
                    .namespaces()
                    .into_iter()
                    .find(|b| b.uri == uri && b.prefix.is_some())
                {
                    Some(binding) => binding.prefix.map(str::to_owned),
                    None => {
                        let fresh = (0usize..)
                            .map(|n| format!("p{n}"))
                            .find(|p| node.search_ns(Some(p)).is_none())
                            .expect("unbound prefix");
                        let root = editor
                            .document()
                            .document_element()
                            .expect("validated")
                            .id();
                        editor.declare_namespace(root, Some(&fresh), uri)?;
                        Some(fresh)
                    }
                },
            };
            let name = NameSpec {
                local: &local,
                namespace: namespace.as_deref(),
                prefix: PrefixPolicy::Exact(prefix.as_deref()),
            };
            editor.set_attribute(element, name, &value)?;
        }
        Ok(diagnostics)
    }

    /// Validates the subtree of `element` as if it were the document element
    /// (`xmlSchemaValidateOneElement`); IDs the parser recorded anywhere in
    /// the document count. `None` when `element` is not an element.
    pub fn validate_element<T: Tree>(
        &self,
        tree: &T,
        element: T::Id,
    ) -> Option<Vec<Diagnostic<T::Id>>> {
        matches!(tree.item(element), crate::xpath::tree::Item::Element(_))
            .then(|| validate::run(self, tree, Some(element), false).0)
    }

    /// Parses and validates a document in one pass without building a tree
    /// (`xmlSchemaValidateStream`). Diagnostics are those of
    /// [`StreamValidator`]; a parse failure is the error.
    pub fn validate_stream<R: BufRead>(
        &self,
        input: R,
        options: ParseOptions,
    ) -> Result<Vec<Diagnostic<usize>>, ReadError> {
        let mut reader = BufferedReader::with_options(input, options).map_err(ReadError::Xml)?;
        let mut validator = StreamValidator::new(self);
        while let Some((event, origin)) = reader.read_event()? {
            validator.event(&event, origin);
        }
        Ok(validator.finish())
    }
}

/// A validator for one document fed as events, libxml2's `xmlSchemaSAXPlug`.
///
/// Feed it every event of a [`Reader`](crate::Reader),
/// [`PushParser`](crate::PushParser) or [`BufferedReader`] with its origin,
/// then [`finish`](Self::finish). Memory follows the open elements, the
/// identity-constraint tables ([`Limits::max_idc_keys`]) and the IDs seen.
///
/// Diagnostics equal [`Schema::validate`]'s, with `node` the element's index
/// in document order. Without [`set_dtd`](Self::set_dtd) the DTD is the
/// internal subset, so an external subset's `ID` attributes and unparsed
/// entities are unknown.
///
/// ```
/// use std::ops::ControlFlow;
/// use yaxml::{PushParser, xsd::{Code, Schema, StreamValidator}};
///
/// let schema = Schema::parse(br#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
///   <xs:element name="r" type="xs:int"/></xs:schema>"#, "r.xsd")?;
/// let mut validator = StreamValidator::new(&schema);
/// let mut parser = PushParser::new();
/// for chunk in [&b"<r>4"[..], b"x</r>"] {
///     parser.feed(chunk, |event, origin| {
///         validator.event(event, origin);
///         ControlFlow::Continue(())
///     })?;
/// }
/// parser.finish(|event, origin| {
///     validator.event(event, origin);
///     ControlFlow::Continue(())
/// })?;
/// let diagnostics = validator.finish();
/// assert_eq!(diagnostics[0].code, Code::CvcDatatypeValid1_2_1);
/// assert_eq!(diagnostics[0].node, Some(0));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct StreamValidator {
    validator: validate::Validator<usize>,
    dtd: Option<Dtd>,
    elements: usize,
}

impl StreamValidator {
    /// Supplies the document's DTD, replacing the internal subset the validator
    /// parses from the DOCTYPE event: a reader that loads the external subset
    /// passes its [`dtd`](crate::BufferedReader::dtd), which `ID` attributes and
    /// `ENTITY` values consult.
    pub fn set_dtd(&mut self, dtd: &Dtd) {
        self.dtd = Some(dtd.clone());
    }

    /// A validator for one document against `schema`.
    pub fn new(schema: &Schema) -> Self {
        Self {
            validator: validate::Validator::new(schema, HashMap::new()),
            dtd: None,
            elements: 0,
        }
    }

    /// Validates the next event of the document.
    pub fn event(&mut self, event: &Event<'_>, origin: SourceOrigin) {
        match event {
            Event::Start(element) | Event::Empty(element) => {
                let node = validate::Node {
                    id: self.elements,
                    offset: origin.input_range().map(|range| range.start),
                };
                self.elements += 1;
                let dtd = self.dtd.as_ref();
                let qualified = element.name.qualified();
                let attributes = element.attributes.iter().map(|attribute| {
                    let id = attribute.name.qualified() == "xml:id"
                        || dtd
                            .is_some_and(|d| d.declares_id(qualified, attribute.name.qualified()));
                    (NodeName::of(&attribute.name), &*attribute.value, id)
                });
                self.validator
                    .start(node, NodeName::of(&element.name), attributes, dtd);
                if matches!(event, Event::Empty(_)) {
                    self.validator.end();
                }
            }
            Event::End(_) => self.validator.end(),
            Event::Text(text) => self.validator.text(text, false),
            Event::Cdata(text) => self.validator.text(text, true),
            Event::Doctype(doctype) => {
                let subset = doctype.internal_subset.unwrap_or_default();
                self.dtd = Dtd::parse(subset.as_bytes()).ok();
                self.validator.other();
            }
            _ => self.validator.other(),
        }
    }

    /// The diagnostics so far; `finish` adds the missing-document-element one.
    pub fn diagnostics(&self) -> &[Diagnostic<usize>] {
        self.validator.diagnostics()
    }

    /// The diagnostics, once the document has ended.
    pub fn finish(self) -> Vec<Diagnostic<usize>> {
        self.validator.finish()
    }
}
