//! Read-only views of a compiled schema's components (libxml2's
//! `schemasInternals.h` structures), borrowed from their [`Schema`].

use super::Schema;
use super::components::{
    AttrId, AttrUse, AttributeDecl, Content, EXTENSION, ElemId, Kind, LIST, NO_NS, NsConstraint,
    Process, QName, RESTRICTION, SUBSTITUTION, Set, TypeId, UNION, ValueConstraint, Variety,
    WildId,
};
use crate::regexp::Regex;
use crate::schema_types::{Facet, WhiteSpace};

/// Derivation methods, as a type's `final` and `block` or an element's
/// `block` (libxml2's `XML_SCHEMAS_*_FINAL_*` and `_BLOCK_*` flags).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct Derivations {
    /// `extension`.
    pub extension: bool,
    /// `restriction`.
    pub restriction: bool,
    /// `substitution` (element `block` only).
    pub substitution: bool,
    /// `list` (simple type `final` only).
    pub list: bool,
    /// `union` (simple type `final` only).
    pub union: bool,
}

impl From<Set> for Derivations {
    fn from(set: Set) -> Self {
        Self {
            extension: set & EXTENSION != 0,
            restriction: set & RESTRICTION != 0,
            substitution: set & SUBSTITUTION != 0,
            list: set & LIST != 0,
            union: set & UNION != 0,
        }
    }
}

/// A default or fixed value as written.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Constraint<'a> {
    /// `fixed` rather than `default`.
    pub fixed: bool,
    /// The lexical value.
    pub value: &'a str,
}

fn constraint(value: &Option<ValueConstraint>) -> Option<Constraint<'_>> {
    (value.as_ref()).map(|value| Constraint {
        fixed: value.fixed,
        value: &value.lexical,
    })
}

/// A simple type's variety (`XML_SCHEMAS_TYPE_VARIETY_*`).
#[derive(Clone, Debug)]
pub enum SimpleVariety<'a> {
    /// An atomic type.
    Atomic,
    /// A list of the item type.
    List(TypeDefinition<'a>),
    /// The member types, in order; a union derived by restriction lists its
    /// base's (libxml2 leaves `memberTypes` empty there).
    Union(Vec<TypeDefinition<'a>>),
}

/// A complex type's content type (`xmlSchemaContentType`).
#[derive(Clone, Copy, Debug)]
pub enum ContentType<'a> {
    /// No content.
    Empty,
    /// Simple content of this type.
    Simple(TypeDefinition<'a>),
    /// Element-only content.
    Elements,
    /// Mixed content.
    Mixed,
}

/// The kind of a type definition.
#[derive(Clone, Debug)]
pub enum TypeKind<'a> {
    /// A simple type.
    Simple(SimpleVariety<'a>),
    /// A complex type with this content type.
    Complex(ContentType<'a>),
}

/// The `processContents` of a wildcard.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessContents {
    /// `skip`.
    Skip,
    /// `lax`.
    Lax,
    /// `strict`.
    Strict,
}

/// The namespaces a wildcard allows; `None` is the absent namespace.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Namespaces<'a> {
    /// `##any`.
    Any,
    /// `##other`: any namespace but this one and the absent namespace.
    Not(Option<&'a str>),
    /// A list, sorted with the absent namespace first.
    Set(Vec<Option<&'a str>>),
}

/// A complex type's attribute wildcard (`xmlSchemaWildcard`).
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct AttributeWildcard<'a> {
    /// The namespace constraint.
    pub namespaces: Namespaces<'a>,
    /// `processContents`.
    pub process: ProcessContents,
}

/// A type definition (`xmlSchemaType`), built in or declared.
#[derive(Clone, Copy)]
pub struct TypeDefinition<'a> {
    schema: &'a Schema,
    id: TypeId,
}

/// A global element declaration (`xmlSchemaElement`).
#[derive(Clone, Copy)]
pub struct ElementDeclaration<'a> {
    schema: &'a Schema,
    name: QName,
    id: ElemId,
}

/// An attribute declaration (`xmlSchemaAttribute`).
#[derive(Clone, Copy)]
pub struct AttributeDeclaration<'a> {
    schema: &'a Schema,
    id: AttrId,
}

/// An attribute use of a complex type, inherited ones included.
#[derive(Clone, Copy)]
pub struct AttributeUse<'a> {
    schema: &'a Schema,
    used: &'a AttrUse,
}

impl Schema {
    fn ns(&self, sym: u32) -> Option<&str> {
        (sym != NO_NS).then(|| self.data.names.str(sym))
    }

    fn qname(&self, namespace: Option<&str>, name: &str) -> QName {
        let names = &self.data.names;
        QName {
            ns: namespace.map_or(NO_NS, |ns| names.get(ns)),
            local: names.get(name),
        }
    }

    /// A global element declaration (`xmlSchemaGetElem`); `None` is the
    /// absent namespace.
    ///
    /// ```
    /// use yaxml::xsd::{ContentType, Schema, TypeKind};
    ///
    /// let schema = Schema::parse(br#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
    ///   <xs:element name="r" nillable="true"><xs:complexType mixed="true">
    ///     <xs:attribute name="id" type="xs:ID" use="required"/>
    ///   </xs:complexType></xs:element>
    /// </xs:schema>"#, "r.xsd")?;
    /// let r = schema.element(None, "r").unwrap();
    /// assert!(r.is_nillable());
    /// let ty = r.type_definition();
    /// assert!(matches!(ty.kind(), TypeKind::Complex(ContentType::Mixed)));
    /// let id = ty.attribute_uses().next().unwrap();
    /// assert!(id.is_required());
    /// assert_eq!(id.declaration().type_definition().name(), Some("ID"));
    /// assert!(schema.element(None, "missing").is_none());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn element(&self, namespace: Option<&str>, name: &str) -> Option<ElementDeclaration<'_>> {
        let name = self.qname(namespace, name);
        let &id = self.data.global_elements.get(&name)?;
        Some(ElementDeclaration {
            schema: self,
            name,
            id,
        })
    }

    /// The global element declarations, by namespace then name.
    pub fn elements(&self) -> impl Iterator<Item = ElementDeclaration<'_>> {
        let mut all: Vec<_> = (self.data.global_elements.iter())
            .map(|(&name, &id)| ElementDeclaration {
                schema: self,
                name,
                id,
            })
            .collect();
        all.sort_by_key(|element| (element.namespace(), element.name()));
        all.into_iter()
    }

    /// A global or built-in type definition (`xmlSchemaGetType`).
    pub fn type_definition(
        &self,
        namespace: Option<&str>,
        name: &str,
    ) -> Option<TypeDefinition<'_>> {
        let name = self.qname(namespace, name);
        let builtins = crate::schema_types::Type::all().count();
        let id = (self.data.global_types.get(&name).copied())
            .or_else(|| (0..=builtins).find(|&id| self.data.types[id].name == Some(name)))?;
        Some(TypeDefinition { schema: self, id })
    }

    /// The schema's global type definitions (built-ins excluded), by
    /// namespace then name.
    pub fn type_definitions(&self) -> impl Iterator<Item = TypeDefinition<'_>> {
        let mut all: Vec<_> = (self.data.global_types.values())
            .map(|&id| TypeDefinition { schema: self, id })
            .filter(|ty| !ty.is_builtin())
            .collect();
        all.sort_by_key(|ty| (ty.namespace(), ty.name()));
        all.into_iter()
    }

    /// A global attribute declaration (`xmlSchemaGetAttributeDecl`).
    pub fn attribute(
        &self,
        namespace: Option<&str>,
        name: &str,
    ) -> Option<AttributeDeclaration<'_>> {
        let &id = self
            .data
            .global_attributes
            .get(&self.qname(namespace, name))?;
        Some(AttributeDeclaration { schema: self, id })
    }

    /// The global attribute declarations, by namespace then name.
    pub fn attributes(&self) -> impl Iterator<Item = AttributeDeclaration<'_>> {
        let mut all: Vec<_> = (self.data.global_attributes.values())
            .map(|&id| AttributeDeclaration { schema: self, id })
            .collect();
        all.sort_by_key(|attribute| (attribute.namespace(), attribute.name()));
        all.into_iter()
    }
}

impl<'a> TypeDefinition<'a> {
    fn def(self) -> &'a super::components::TypeDef {
        &self.schema.data.types[self.id]
    }

    fn of(self, id: TypeId) -> TypeDefinition<'a> {
        TypeDefinition {
            schema: self.schema,
            id,
        }
    }

    /// The name; `None` for an anonymous type.
    pub fn name(self) -> Option<&'a str> {
        Some(self.schema.data.names.str(self.def().name?.local))
    }

    /// The target namespace; `None` if absent or anonymous.
    pub fn namespace(self) -> Option<&'a str> {
        self.schema.ns(self.def().name?.ns)
    }

    /// A built-in type: `xs:anyType` or a [`schema_types::Type`](crate::schema_types::Type).
    pub fn is_builtin(self) -> bool {
        self.id <= crate::schema_types::Type::all().count()
    }

    /// The base type definition (`baseType`); `xs:anyType` is its own.
    pub fn base(self) -> TypeDefinition<'a> {
        self.of(self.def().base)
    }

    /// A simple type's whitespace handling (`XML_SCHEMAS_TYPE_WHITESPACE_*`);
    /// `None` for a union or complex type.
    pub fn whitespace(self) -> Option<WhiteSpace> {
        let simple = self.def().simple()?;
        (!matches!(simple.variety, Variety::Union(_))).then_some(simple.whitespace)
    }

    /// A simple type's effective length, bound, digit and `whiteSpace`
    /// facets, inherited ones included (libxml2's `facetSet`, which also
    /// holds this step's patterns and enumerations); values are typed, not
    /// lexical.
    pub fn facets(self) -> &'a [Facet] {
        self.def().simple().map_or(&[], |simple| &simple.facets)
    }

    /// The patterns of this derivation step, any one of which must match
    /// (each ancestor's step must match too).
    pub fn patterns(self) -> &'a [Regex] {
        self.def().simple().map_or(&[], |simple| &simple.patterns)
    }

    /// Derived by extension rather than restriction.
    pub fn is_extension(self) -> bool {
        self.def().derivation == EXTENSION
    }

    /// `final`.
    pub fn final_(self) -> Derivations {
        self.def().final_.into()
    }

    /// `block`.
    pub fn block(self) -> Derivations {
        self.def().block.into()
    }

    /// `abstract`.
    pub fn is_abstract(self) -> bool {
        self.def().abstract_
    }

    /// Simple with its variety, or complex with its content type.
    pub fn kind(self) -> TypeKind<'a> {
        match &self.def().kind {
            Kind::Simple(simple) => TypeKind::Simple(match &simple.variety {
                Variety::Atomic => SimpleVariety::Atomic,
                Variety::List(item) => SimpleVariety::List(self.of(*item)),
                Variety::Union(members) => {
                    SimpleVariety::Union(members.iter().map(|&id| self.of(id)).collect())
                }
            }),
            Kind::Complex(complex) => TypeKind::Complex(match &complex.content {
                Content::Empty => ContentType::Empty,
                Content::Simple(id) => ContentType::Simple(self.of(*id)),
                Content::Elements { mixed: false, .. } => ContentType::Elements,
                Content::Elements { mixed: true, .. } => ContentType::Mixed,
            }),
        }
    }

    /// A complex type's attribute uses, inherited ones included; none for a
    /// simple type.
    pub fn attribute_uses(self) -> impl Iterator<Item = AttributeUse<'a>> {
        let schema = self.schema;
        (self.def().complex().into_iter())
            .flat_map(|complex| &complex.attributes)
            .map(move |used| AttributeUse { schema, used })
    }

    /// A complex type's attribute wildcard.
    pub fn attribute_wildcard(self) -> Option<AttributeWildcard<'a>> {
        let id: WildId = self.def().complex()?.wildcard?;
        let wildcard = &self.schema.data.wildcards[id];
        let ns = |sym| self.schema.ns(sym);
        Some(AttributeWildcard {
            namespaces: match &wildcard.ns {
                NsConstraint::Any => Namespaces::Any,
                NsConstraint::Not(sym) => Namespaces::Not(ns(*sym)),
                NsConstraint::Set(set) => {
                    let mut set: Vec<_> = set.iter().map(|&sym| ns(sym)).collect();
                    set.sort();
                    Namespaces::Set(set)
                }
            },
            process: match wildcard.process {
                Process::Skip => ProcessContents::Skip,
                Process::Lax => ProcessContents::Lax,
                Process::Strict => ProcessContents::Strict,
            },
        })
    }
}

impl<'a> ElementDeclaration<'a> {
    fn decl(self) -> &'a super::components::ElementDecl {
        &self.schema.data.elements[self.id]
    }

    /// The local name.
    pub fn name(self) -> &'a str {
        self.schema.data.names.str(self.name.local)
    }

    /// The target namespace; `None` if absent.
    pub fn namespace(self) -> Option<&'a str> {
        self.schema.ns(self.name.ns)
    }

    /// The element's type.
    pub fn type_definition(self) -> TypeDefinition<'a> {
        TypeDefinition {
            schema: self.schema,
            id: self.decl().ty,
        }
    }

    /// `nillable`.
    pub fn is_nillable(self) -> bool {
        self.decl().nillable
    }

    /// `abstract`.
    pub fn is_abstract(self) -> bool {
        self.decl().abstract_
    }

    /// The `default` or `fixed` value.
    pub fn value(self) -> Option<Constraint<'a>> {
        constraint(&self.decl().value)
    }

    /// `block`.
    pub fn block(self) -> Derivations {
        self.decl().block.into()
    }
}

impl<'a> AttributeDeclaration<'a> {
    fn decl(self) -> &'a AttributeDecl {
        &self.schema.data.attributes[self.id]
    }

    /// The local name.
    pub fn name(self) -> &'a str {
        self.schema.data.names.str(self.decl().name.local)
    }

    /// The target namespace; `None` if absent.
    pub fn namespace(self) -> Option<&'a str> {
        self.schema.ns(self.decl().name.ns)
    }

    /// The attribute's simple type.
    pub fn type_definition(self) -> TypeDefinition<'a> {
        TypeDefinition {
            schema: self.schema,
            id: self.decl().ty,
        }
    }

    /// The declaration's `default` or `fixed` value.
    pub fn value(self) -> Option<Constraint<'a>> {
        constraint(&self.decl().value)
    }
}

impl<'a> AttributeUse<'a> {
    /// The attribute declaration used.
    pub fn declaration(self) -> AttributeDeclaration<'a> {
        AttributeDeclaration {
            schema: self.schema,
            id: self.used.decl,
        }
    }

    /// `use="required"`.
    pub fn is_required(self) -> bool {
        self.used.required
    }

    /// The use's own value constraint (`defValue`); the declaration's is
    /// [`AttributeDeclaration::value`].
    pub fn value(self) -> Option<Constraint<'a>> {
        constraint(&self.used.value)
    }
}

macro_rules! debug_by_name {
    ($($view:ident),*) => {$(
        impl std::fmt::Debug for $view<'_> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct(stringify!($view))
                    .field("namespace", &self.namespace())
                    .field("name", &self.name())
                    .finish()
            }
        }
    )*};
}

debug_by_name!(TypeDefinition, ElementDeclaration, AttributeDeclaration);

impl std::fmt::Debug for AttributeUse<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AttributeUse")
            .field("declaration", &self.declaration())
            .field("required", &self.is_required())
            .finish()
    }
}
