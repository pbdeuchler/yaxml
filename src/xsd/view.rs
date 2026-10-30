//! Read-only views of a compiled schema's components (libxml2's
//! `schemasInternals.h` structures), borrowed from their [`Schema`].

use super::Schema;
use super::components::{
    AttrGroup, AttrId, AttrUse, AttributeDecl, Content, EXTENSION, ElemId, IdcKind, Kind, LIST,
    NO_NS, NsConstraint, Process, QName, RESTRICTION, SUBSTITUTION, Set, TypeId, UNION,
    ValueConstraint, Variety, WildId,
};
use super::parse::{self, Annotated, Compositor, Loc};
use crate::regexp::Regex;
use crate::schema_types::{Facet, WhiteSpace};
use crate::{OwnedDocument, OwnedNode, OwnedNodeId};

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

/// An attribute wildcard, or a wildcard particle's term
/// (`xmlSchemaWildcard`).
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Wildcard<'a> {
    /// The namespace constraint.
    pub namespaces: Namespaces<'a>,
    /// `processContents`.
    pub process: ProcessContents,
    annotations: Annotations<'a>,
}

impl<'a> Wildcard<'a> {
    /// An `xs:anyAttribute`'s annotations (`annot`); a wildcard particle's
    /// are the particle's, a complete wildcard has none.
    pub fn annotations(&self) -> Annotations<'a> {
        self.annotations.clone()
    }
}

/// A component's annotations (libxml2's `annot` chain), in document order:
/// copies of its `xs:annotation` elements, whose parent is a synthetic
/// `annotations` root (libxml2's point into the schema documents).
#[derive(Clone)]
pub struct Annotations<'a> {
    doc: Option<&'a OwnedDocument>,
    ids: std::slice::Iter<'a, OwnedNodeId>,
}

impl<'a> Iterator for Annotations<'a> {
    type Item = OwnedNode<'a>;

    fn next(&mut self) -> Option<OwnedNode<'a>> {
        let id = self.ids.next()?;
        self.doc?.node(*id)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.ids.size_hint()
    }
}

impl ExactSizeIterator for Annotations<'_> {}

impl std::fmt::Debug for Annotations<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.ids.as_slice()).finish()
    }
}

/// A type definition (`xmlSchemaType`), built in or declared.
#[derive(Clone, Copy)]
pub struct TypeDefinition<'a> {
    schema: &'a Schema,
    id: TypeId,
}

/// An element declaration, global or local (`xmlSchemaElement`).
#[derive(Clone, Copy)]
pub struct ElementDeclaration<'a> {
    schema: &'a Schema,
    id: ElemId,
}

/// An attribute declaration (`xmlSchemaAttribute`).
#[derive(Clone, Copy)]
pub struct AttributeDeclaration<'a> {
    schema: &'a Schema,
    id: AttrId,
}

/// An attribute use of a complex type (inherited ones included) or of an
/// attribute group.
#[derive(Clone, Copy)]
pub struct AttributeUse<'a> {
    schema: &'a Schema,
    used: &'a AttrUse,
}

/// A global attribute group definition (`xmlSchemaAttributeGroup`).
#[derive(Clone, Copy)]
pub struct AttributeGroup<'a> {
    schema: &'a Schema,
    id: usize,
}

/// A content model particle (libxml2's private `xmlSchemaParticle`).
#[derive(Clone, Copy)]
pub struct Particle<'a> {
    schema: &'a Schema,
    particle: &'a parse::Particle,
}

/// A particle's term.
#[derive(Clone, Debug)]
pub enum Term<'a> {
    /// A local or referenced element declaration.
    Element(ElementDeclaration<'a>),
    /// An element wildcard (`xs:any`).
    Wildcard(Wildcard<'a>),
    /// A model group; a group reference's is the referenced definition's.
    Group(ModelGroup<'a>),
}

/// A model group (libxml2's private `xmlSchemaModelGroup`).
#[derive(Clone, Copy)]
pub struct ModelGroup<'a> {
    schema: &'a Schema,
    compositor: Compositor,
    particles: &'a [parse::Particle],
    loc: Loc,
}

/// A global model group definition (`xs:group`, libxml2's private
/// `xmlSchemaModelGroupDef`).
#[derive(Clone, Copy)]
pub struct ModelGroupDefinition<'a> {
    schema: &'a Schema,
    id: usize,
}

/// An identity-constraint definition (libxml2's private `xmlSchemaIDC`).
#[derive(Clone, Copy)]
pub struct IdentityConstraint<'a> {
    schema: &'a Schema,
    id: usize,
}

impl Schema {
    fn ns(&self, sym: u32) -> Option<&str> {
        (sym != NO_NS).then(|| self.data.names.str(sym))
    }

    fn annotations(&self, loc: Option<Loc>, kind: Annotated) -> Annotations<'_> {
        let notes = self.data.notes.as_ref();
        let ids = (loc.zip(notes)).and_then(|(loc, notes)| notes.owners.get(&(loc, kind)));
        Annotations {
            doc: notes.map(|notes| &notes.doc),
            ids: ids.map_or(&[][..], Vec::as_slice).iter(),
        }
    }

    /// The first top-level `xs:annotation` (`_xmlSchema.annot`), in the
    /// documents' parse order, as a copy ([`Annotations`]).
    ///
    /// ```
    /// use yaxml::xsd::Schema;
    /// let schema = Schema::parse(br#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
    ///   <xs:annotation><xs:documentation>Orders</xs:documentation></xs:annotation>
    ///   <xs:element name="order"><xs:annotation><xs:appinfo/></xs:annotation></xs:element>
    /// </xs:schema>"#, "o.xsd")?;
    /// assert_eq!(schema.annotation().unwrap().content().as_deref(), Some("Orders"));
    /// let order = schema.element(None, "order").unwrap();
    /// assert_eq!(order.annotations().count(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn annotation(&self) -> Option<OwnedNode<'_>> {
        let notes = self.data.notes.as_ref()?;
        notes.doc.node(notes.schema?)
    }

    fn wildcard(&self, id: Option<WildId>) -> Option<Wildcard<'_>> {
        let wildcard = &self.data.wildcards[id?];
        let ns = |sym| self.ns(sym);
        Some(Wildcard {
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
            annotations: self.annotations(wildcard.loc, Annotated::Wildcard),
        })
    }

    fn qname(&self, namespace: Option<&str>, name: &str) -> QName {
        let names = &self.data.names;
        QName {
            ns: namespace.map_or(NO_NS, |ns| names.get(ns)),
            local: names.get(name),
        }
    }

    /// The main document's `targetNamespace` (`_xmlSchema.targetNamespace`).
    pub fn target_namespace(&self) -> Option<&str> {
        self.ns(self.data.target)
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
        let &id = (self.data.global_elements).get(&self.qname(namespace, name))?;
        Some(ElementDeclaration { schema: self, id })
    }

    /// An identity-constraint definition by name (`_xmlSchema.idcDef`);
    /// `None` is the absent namespace.
    pub fn identity_constraint(
        &self,
        namespace: Option<&str>,
        name: &str,
    ) -> Option<IdentityConstraint<'_>> {
        let &id = (self.data.global_idcs).get(&self.qname(namespace, name))?;
        Some(IdentityConstraint { schema: self, id })
    }

    /// A global model group definition (`_xmlSchema.groupDecl`); `None` is
    /// the absent namespace.
    ///
    /// ```
    /// use yaxml::xsd::{Compositor, Schema, Term};
    ///
    /// let schema = Schema::parse(br#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
    ///   <xs:group name="g"><xs:choice><xs:element name="a"/><xs:any/></xs:choice></xs:group>
    /// </xs:schema>"#, "g.xsd")?;
    /// let group = schema.model_group(None, "g").unwrap().model_group().unwrap();
    /// assert_eq!(group.compositor(), Compositor::Choice);
    /// let terms: Vec<_> = group.particles().map(|p| p.term()).collect();
    /// assert!(matches!(&terms[..], [Some(Term::Element(a)), Some(Term::Wildcard(_))] if a.name() == "a"));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn model_group(
        &self,
        namespace: Option<&str>,
        name: &str,
    ) -> Option<ModelGroupDefinition<'_>> {
        let &id = (self.data.global_groups).get(&self.qname(namespace, name))?;
        Some(ModelGroupDefinition { schema: self, id })
    }

    /// The global model group definitions, by namespace then name.
    pub fn model_groups(&self) -> impl Iterator<Item = ModelGroupDefinition<'_>> {
        let mut all: Vec<_> = (self.data.global_groups.values())
            .map(|&id| ModelGroupDefinition { schema: self, id })
            .collect();
        all.sort_by_key(|g| (g.namespace(), g.name()));
        all.into_iter()
    }

    /// The global element declarations, by namespace then name.
    pub fn elements(&self) -> impl Iterator<Item = ElementDeclaration<'_>> {
        let mut all: Vec<_> = (self.data.global_elements.values())
            .map(|&id| ElementDeclaration { schema: self, id })
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

    /// A global attribute group definition.
    pub fn attribute_group(
        &self,
        namespace: Option<&str>,
        name: &str,
    ) -> Option<AttributeGroup<'_>> {
        let name = self.qname(namespace, name);
        let &id = self.data.global_attribute_groups.get(&name)?;
        Some(AttributeGroup { schema: self, id })
    }

    /// The global attribute group definitions, by namespace then name.
    pub fn attribute_groups(&self) -> impl Iterator<Item = AttributeGroup<'_>> {
        let mut all: Vec<_> = (self.data.global_attribute_groups.values())
            .map(|&id| AttributeGroup { schema: self, id })
            .collect();
        all.sort_by_key(|group| (group.namespace(), group.name()));
        all.into_iter()
    }

    /// The global notation declarations as (target namespace, name), sorted
    /// (`_xmlSchema.notaDecl`; like libxml2, public and system identifiers
    /// are not kept).
    pub fn notations(&self) -> impl Iterator<Item = (Option<&str>, &str)> {
        let mut all: Vec<_> = (self.data.notations.iter())
            .map(|name| (self.ns(name.ns), self.data.names.str(name.local)))
            .collect();
        all.sort_unstable();
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

    /// A complex type's content particle (`subtypes`) for element-only or
    /// mixed content: an extension's is a sequence of its base's and its
    /// own, as libxml2 builds it. `xs:anyType`'s is a sequence of a lax
    /// `xs:any`.
    pub fn particle(self) -> Option<Particle<'a>> {
        let particle = self.schema.data.particles.get(&self.id)?;
        Some(Particle {
            schema: self.schema,
            particle,
        })
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
    pub fn attribute_wildcard(self) -> Option<Wildcard<'a>> {
        self.schema.wildcard(self.def().complex()?.wildcard)
    }

    /// Its annotations (`annot`), with those of its content, derivation,
    /// list and union children; none for a built-in or content type.
    pub fn annotations(self) -> Annotations<'a> {
        self.schema.annotations(self.def().loc, Annotated::Type)
    }
}

impl<'a> AttributeGroup<'a> {
    fn group(self) -> &'a AttrGroup {
        &self.schema.data.attribute_groups[self.id]
    }

    /// The local name.
    pub fn name(self) -> &'a str {
        self.schema.data.names.str(self.group().name.local)
    }

    /// The target namespace; `None` if absent.
    pub fn namespace(self) -> Option<&'a str> {
        self.schema.ns(self.group().name.ns)
    }

    /// The attribute uses, those of referenced groups included.
    pub fn attribute_uses(self) -> impl Iterator<Item = AttributeUse<'a>> {
        let schema = self.schema;
        (self.group().uses.iter()).map(move |used| AttributeUse { schema, used })
    }

    /// The attribute wildcard, intersected with those of referenced groups.
    pub fn attribute_wildcard(self) -> Option<Wildcard<'a>> {
        self.schema.wildcard(self.group().wildcard)
    }

    /// Its annotations (`annot`).
    pub fn annotations(self) -> Annotations<'a> {
        self.schema
            .annotations(Some(self.group().loc), Annotated::AttrGroup)
    }
}

impl<'a> ElementDeclaration<'a> {
    fn decl(self) -> &'a super::components::ElementDecl {
        &self.schema.data.elements[self.id]
    }

    /// The local name.
    pub fn name(self) -> &'a str {
        self.schema.data.names.str(self.decl().name.local)
    }

    /// The target namespace; `None` if absent.
    pub fn namespace(self) -> Option<&'a str> {
        self.schema.ns(self.decl().name.ns)
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

    /// `final` (extension and restriction only).
    pub fn final_(self) -> Derivations {
        self.decl().final_.into()
    }

    /// The substitution group head (`substitutionGroup`).
    pub fn substitution_group(self) -> Option<ElementDeclaration<'a>> {
        let id = self.decl().head?;
        Some(ElementDeclaration { id, ..self })
    }

    /// The non-abstract declarations that may substitute for this one,
    /// directly or through other heads, as its `block` and their types
    /// allow; non-empty is libxml2's `XML_SCHEMAS_ELEM_SUBST_GROUP_HEAD`.
    pub fn substitutes(self) -> impl Iterator<Item = ElementDeclaration<'a>> {
        (self.decl().members.iter()).map(move |&id| ElementDeclaration { id, ..self })
    }

    /// Its annotations (`annot`).
    pub fn annotations(self) -> Annotations<'a> {
        self.schema
            .annotations(Some(self.decl().loc), Annotated::Element)
    }

    /// The identity constraints, in document order (`_xmlSchemaElement.idcs`).
    pub fn identity_constraints(self) -> impl Iterator<Item = IdentityConstraint<'a>> {
        let schema = self.schema;
        (self.decl().idcs.iter()).map(move |&id| IdentityConstraint { schema, id })
    }
}

impl<'a> Particle<'a> {
    /// `minOccurs`.
    pub fn min_occurs(self) -> u32 {
        self.particle.min
    }

    /// `maxOccurs`; `None` is `unbounded`.
    pub fn max_occurs(self) -> Option<u32> {
        self.particle.max
    }

    /// The term; `None` for a reference that resolved to nothing (or to an
    /// empty group definition), which matches nothing, as in libxml2.
    pub fn term(self) -> Option<Term<'a>> {
        let schema = self.schema;
        let group = |compositor, particles, loc| {
            Term::Group(ModelGroup {
                schema,
                compositor,
                particles,
                loc,
            })
        };
        Some(match &self.particle.term {
            parse::Term::Element(id) => Term::Element(ElementDeclaration { schema, id: *id }),
            parse::Term::Wildcard(id) => Term::Wildcard(schema.wildcard(Some(*id))?),
            parse::Term::Group(compositor, particles) => {
                group(*compositor, particles, self.particle.loc)
            }
            parse::Term::GroupDef(id) => {
                let (compositor, particles, loc) = schema.data.groups[*id].model.as_ref()?;
                group(*compositor, particles, *loc)
            }
            _ => return None,
        })
    }

    /// Its annotations (`annot`): an element or group reference's, an
    /// `xs:any`'s; a local element's are its declaration's, a model
    /// group's the group's.
    pub fn annotations(self) -> Annotations<'a> {
        self.schema
            .annotations(Some(self.particle.loc), Annotated::Particle)
    }
}

impl<'a> ModelGroup<'a> {
    /// `sequence`, `choice` or `all`.
    pub fn compositor(self) -> Compositor {
        self.compositor
    }

    /// The particles, in order.
    pub fn particles(self) -> impl Iterator<Item = Particle<'a>> {
        let schema = self.schema;
        (self.particles.iter()).map(move |particle| Particle { schema, particle })
    }

    /// Its annotations (`annot`).
    pub fn annotations(self) -> Annotations<'a> {
        self.schema
            .annotations(Some(self.loc), Annotated::ModelGroup)
    }
}

impl<'a> ModelGroupDefinition<'a> {
    fn group(self) -> &'a parse::GroupDraft {
        &self.schema.data.groups[self.id]
    }

    /// The local name.
    pub fn name(self) -> &'a str {
        self.schema.data.names.str(self.group().name.local)
    }

    /// The target namespace; `None` if absent.
    pub fn namespace(self) -> Option<&'a str> {
        self.schema.ns(self.group().name.ns)
    }

    /// The model group; `None` for a definition with only an annotation.
    pub fn model_group(self) -> Option<ModelGroup<'a>> {
        let (compositor, particles, loc) = self.group().model.as_ref()?;
        Some(ModelGroup {
            schema: self.schema,
            compositor: *compositor,
            particles,
            loc: *loc,
        })
    }

    /// Its annotations (`annot`).
    pub fn annotations(self) -> Annotations<'a> {
        self.schema
            .annotations(Some(self.group().loc), Annotated::Group)
    }
}

impl<'a> IdentityConstraint<'a> {
    fn idc(self) -> &'a super::components::Idc {
        &self.schema.data.idcs[self.id]
    }

    /// `unique`, `key` or `keyref`.
    pub fn kind(self) -> IdcKind {
        self.idc().kind
    }

    /// The local name.
    pub fn name(self) -> &'a str {
        self.schema.data.names.str(self.idc().name.local)
    }

    /// The target namespace; `None` if absent.
    pub fn namespace(self) -> Option<&'a str> {
        self.schema.ns(self.idc().name.ns)
    }

    /// The selector's XPath, as written.
    pub fn selector(self) -> &'a str {
        &self.idc().selector.0
    }

    /// The fields' XPaths, as written.
    pub fn fields(self) -> impl Iterator<Item = &'a str> {
        self.idc().fields.iter().map(|(xpath, _)| &**xpath)
    }

    /// The key or unique constraint a keyref refers to.
    pub fn refer(self) -> Option<IdentityConstraint<'a>> {
        let id = self.idc().refer?;
        Some(IdentityConstraint { id, ..self })
    }

    /// Its annotations (`annot`), with its selector's and fields'.
    pub fn annotations(self) -> Annotations<'a> {
        self.schema
            .annotations(Some(self.idc().loc), Annotated::Idc)
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

    /// A global declaration's annotations (`annot`); a local one's are its
    /// use's.
    pub fn annotations(self) -> Annotations<'a> {
        self.schema
            .annotations(Some(self.decl().loc), Annotated::Attribute)
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

    /// Its annotations (`annot`).
    pub fn annotations(self) -> Annotations<'a> {
        self.schema.annotations(Some(self.used.loc), Annotated::Use)
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

debug_by_name!(
    TypeDefinition,
    ElementDeclaration,
    AttributeDeclaration,
    AttributeGroup,
    ModelGroupDefinition,
    IdentityConstraint
);

impl std::fmt::Debug for Particle<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Particle")
            .field("min", &self.min_occurs())
            .field("max", &self.max_occurs())
            .field("term", &self.term())
            .finish()
    }
}

impl std::fmt::Debug for ModelGroup<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelGroup")
            .field("compositor", &self.compositor)
            .field("particles", &self.particles().collect::<Vec<_>>())
            .finish()
    }
}

impl std::fmt::Debug for AttributeUse<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AttributeUse")
            .field("declaration", &self.declaration())
            .field("required", &self.is_required())
            .finish()
    }
}
