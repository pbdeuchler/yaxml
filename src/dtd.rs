use std::borrow::Cow;
use std::ops::Range;

use crate::escape::xml_char;
use crate::name::{name_char, name_start};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DtdLimits {
    pub(crate) max_unit_bytes: usize,
    pub(crate) max_tokens: usize,
    pub(crate) max_model_depth: usize,
    pub(crate) max_attribute_definitions: usize,
}

impl Default for DtdLimits {
    fn default() -> Self {
        Self {
            max_unit_bytes: 64 * 1024,
            max_tokens: 4 * 1024,
            max_model_depth: 256,
            max_attribute_definitions: 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DtdLimit {
    UnitBytes,
    Tokens,
    ModelDepth,
    AttributeDefinitions,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DtdErrorKind {
    Syntax,
    InvalidName,
    InvalidNmtoken,
    InvalidCharacter,
    InvalidReference,
    InvalidPublicIdentifier,
    SystemIdentifierFragment,
    ReservedPiTarget,
    LimitExceeded(DtdLimit),
    Allocation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DtdError {
    pub(crate) kind: DtdErrorKind,
    pub(crate) offset: usize,
}

impl DtdError {
    fn new(kind: DtdErrorKind, offset: usize) -> Self {
        Self { kind, offset }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReferenceKind {
    General,
    Parameter,
    DecimalCharacter,
    HexadecimalCharacter,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Connector {
    Choice,
    Sequence,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Occurrence {
    Optional,
    ZeroOrMore,
    OneOrMore,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ModelSyntax {
    Open,
    Close,
    Pcdata,
    Connector(Connector),
    Occurrence(Occurrence),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LiteralKind {
    AttributeValue,
    Comment,
    EntityValue,
    ProcessingInstructionData,
    PublicIdentifier,
    SystemIdentifier,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum AttributeTypeMeta {
    Cdata,
    Id,
    IdRef,
    IdRefs,
    Entity,
    Entities,
    NmToken,
    NmTokens,
    Notation(Range<usize>),
    Enumeration(Range<usize>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DefaultMeta {
    Required,
    Implied,
    Value { fixed: bool, literal: usize },
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AttributeDefinitionMeta {
    name: usize,
    attribute_type: AttributeTypeMeta,
    default: DefaultMeta,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum TokenKind {
    Name,
    AttributeListName { definitions: usize },
    NmToken,
    Literal(LiteralKind),
    Reference(ReferenceKind),
    Model(ModelSyntax),
    AttributeDefinition(AttributeDefinitionMeta),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DtdToken {
    kind: TokenKind,
    range: Range<usize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ExternalMeta {
    public: Option<usize>,
    system: Option<usize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DtdUnitKind {
    Header,
    HeaderSubset,
    HeaderSystem,
    HeaderSystemSubset,
    HeaderPublic,
    HeaderPublicSubset,
    ElementEmpty,
    ElementAny,
    ElementMixed,
    ElementChildren,
    AttributeList,
    GeneralInternalEntity,
    ParameterInternalEntity,
    GeneralSystemEntity,
    ParameterSystemEntity,
    GeneralPublicEntity,
    ParameterPublicEntity,
    GeneralSystemNdataEntity,
    GeneralPublicNdataEntity,
    SystemNotation,
    PublicNotation,
    PublicSystemNotation,
    Comment,
    ProcessingInstruction,
    ProcessingInstructionData,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct DtdUnit<'a> {
    raw: Cow<'a, str>,
    tokens: Vec<DtdToken>,
    kind: DtdUnitKind,
}

impl<'a> DtdUnit<'a> {
    pub(crate) fn parse(raw: impl Into<Cow<'a, str>>, limits: DtdLimits) -> Result<Self, DtdError> {
        let raw = raw.into();
        if raw.len() > limits.max_unit_bytes {
            return Err(DtdError::new(
                DtdErrorKind::LimitExceeded(DtdLimit::UnitBytes),
                limits.max_unit_bytes,
            ));
        }
        if let Some((offset, _)) = raw
            .char_indices()
            .find(|(_, character)| !xml_char(*character))
        {
            return Err(DtdError::new(DtdErrorKind::InvalidCharacter, offset));
        }
        let (kind, tokens) = Parser::new(&raw, limits).parse()?;
        Ok(Self { raw, tokens, kind })
    }

    pub(crate) fn raw(&self) -> &str {
        &self.raw
    }

    pub(crate) fn view(&self) -> DtdUnitView<'_> {
        match self.kind {
            kind @ (DtdUnitKind::Header
            | DtdUnitKind::HeaderSubset
            | DtdUnitKind::HeaderSystem
            | DtdUnitKind::HeaderSystemSubset
            | DtdUnitKind::HeaderPublic
            | DtdUnitKind::HeaderPublicSubset) => {
                let (external, has_internal_subset) = match kind {
                    DtdUnitKind::Header => (None, false),
                    DtdUnitKind::HeaderSubset => (None, true),
                    DtdUnitKind::HeaderSystem => (
                        Some(ExternalMeta {
                            public: None,
                            system: Some(1),
                        }),
                        false,
                    ),
                    DtdUnitKind::HeaderSystemSubset => (
                        Some(ExternalMeta {
                            public: None,
                            system: Some(1),
                        }),
                        true,
                    ),
                    DtdUnitKind::HeaderPublic => (
                        Some(ExternalMeta {
                            public: Some(1),
                            system: Some(2),
                        }),
                        false,
                    ),
                    DtdUnitKind::HeaderPublicSubset => (
                        Some(ExternalMeta {
                            public: Some(1),
                            system: Some(2),
                        }),
                        true,
                    ),
                    _ => unreachable!(),
                };
                DtdUnitView::Header(DoctypeHeaderView {
                    unit: self,
                    external,
                    has_internal_subset,
                })
            }
            kind @ (DtdUnitKind::ElementEmpty
            | DtdUnitKind::ElementAny
            | DtdUnitKind::ElementMixed
            | DtdUnitKind::ElementChildren) => {
                DtdUnitView::Element(ElementDeclarationView { unit: self, kind })
            }
            DtdUnitKind::AttributeList => {
                DtdUnitView::AttributeList(AttributeListView { unit: self })
            }
            kind @ (DtdUnitKind::GeneralInternalEntity
            | DtdUnitKind::ParameterInternalEntity
            | DtdUnitKind::GeneralSystemEntity
            | DtdUnitKind::ParameterSystemEntity
            | DtdUnitKind::GeneralPublicEntity
            | DtdUnitKind::ParameterPublicEntity
            | DtdUnitKind::GeneralSystemNdataEntity
            | DtdUnitKind::GeneralPublicNdataEntity) => {
                DtdUnitView::Entity(EntityDeclarationView { unit: self, kind })
            }
            kind @ (DtdUnitKind::SystemNotation
            | DtdUnitKind::PublicNotation
            | DtdUnitKind::PublicSystemNotation) => {
                DtdUnitView::Notation(NotationDeclarationView { unit: self, kind })
            }
            DtdUnitKind::Comment => DtdUnitView::Comment(self.token_text(0)),
            kind
            @ (DtdUnitKind::ProcessingInstruction | DtdUnitKind::ProcessingInstructionData) => {
                DtdUnitView::ProcessingInstruction(ProcessingInstructionView {
                    target: self.token_text(0),
                    data: (kind == DtdUnitKind::ProcessingInstructionData)
                        .then(|| self.token_text(1)),
                })
            }
        }
    }

    pub(crate) fn references(&self) -> impl Iterator<Item = ReferenceView<'_>> {
        self.tokens.iter().filter_map(|token| match token.kind {
            TokenKind::Reference(kind) => Some(ReferenceView {
                kind,
                spelling: &self.raw[token.range.clone()],
            }),
            _ => None,
        })
    }

    fn token_text(&self, index: usize) -> &str {
        &self.raw[self.tokens[index].range.clone()]
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DtdUnitView<'unit> {
    Header(DoctypeHeaderView<'unit>),
    Element(ElementDeclarationView<'unit>),
    AttributeList(AttributeListView<'unit>),
    Entity(EntityDeclarationView<'unit>),
    Notation(NotationDeclarationView<'unit>),
    Comment(&'unit str),
    ProcessingInstruction(ProcessingInstructionView<'unit>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ReferenceView<'a> {
    pub(crate) kind: ReferenceKind,
    pub(crate) spelling: &'a str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ExternalIdView<'a> {
    pub(crate) public: Option<&'a str>,
    pub(crate) system: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DoctypeHeaderView<'a> {
    unit: &'a DtdUnit<'a>,
    external: Option<ExternalMeta>,
    has_internal_subset: bool,
}

impl<'a> DoctypeHeaderView<'a> {
    pub(crate) fn root_name(self) -> &'a str {
        self.unit.token_text(0)
    }

    pub(crate) fn external_id(self) -> Option<ExternalIdView<'a>> {
        self.external.map(|external| external.view(self.unit))
    }

    pub(crate) fn has_internal_subset(self) -> bool {
        self.has_internal_subset
    }
}

impl ExternalMeta {
    fn view<'a>(self, unit: &'a DtdUnit<'a>) -> ExternalIdView<'a> {
        ExternalIdView {
            public: self.public.map(|index| unit.token_text(index)),
            system: self.system.map(|index| unit.token_text(index)),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ElementContentView<'a> {
    Empty,
    Any,
    Mixed(ModelTokens<'a>),
    Children(ModelTokens<'a>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ElementDeclarationView<'a> {
    unit: &'a DtdUnit<'a>,
    kind: DtdUnitKind,
}

impl<'a> ElementDeclarationView<'a> {
    pub(crate) fn name(self) -> &'a str {
        self.unit.token_text(0)
    }

    pub(crate) fn content(self) -> ElementContentView<'a> {
        match self.kind {
            DtdUnitKind::ElementEmpty => ElementContentView::Empty,
            DtdUnitKind::ElementAny => ElementContentView::Any,
            DtdUnitKind::ElementMixed => ElementContentView::Mixed(ModelTokens {
                unit: self.unit,
                next: 1,
                end: self.unit.tokens.len(),
            }),
            DtdUnitKind::ElementChildren => ElementContentView::Children(ModelTokens {
                unit: self.unit,
                next: 1,
                end: self.unit.tokens.len(),
            }),
            _ => unreachable!("view created only for element kinds"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ModelTokenView<'a> {
    Open,
    Close,
    Pcdata,
    Name(&'a str),
    Connector(Connector),
    Occurrence(Occurrence),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ModelTokens<'a> {
    unit: &'a DtdUnit<'a>,
    next: usize,
    end: usize,
}

impl<'a> Iterator for ModelTokens<'a> {
    type Item = ModelTokenView<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        while self.next < self.end {
            let index = self.next;
            self.next += 1;
            let token = &self.unit.tokens[index];
            return Some(match token.kind {
                TokenKind::Name => ModelTokenView::Name(self.unit.token_text(index)),
                TokenKind::Model(ModelSyntax::Open) => ModelTokenView::Open,
                TokenKind::Model(ModelSyntax::Close) => ModelTokenView::Close,
                TokenKind::Model(ModelSyntax::Pcdata) => ModelTokenView::Pcdata,
                TokenKind::Model(ModelSyntax::Connector(connector)) => {
                    ModelTokenView::Connector(connector)
                }
                TokenKind::Model(ModelSyntax::Occurrence(occurrence)) => {
                    ModelTokenView::Occurrence(occurrence)
                }
                _ => continue,
            });
        }
        None
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum AttributeTypeView<'a> {
    Cdata,
    Id,
    IdRef,
    IdRefs,
    Entity,
    Entities,
    NmToken,
    NmTokens,
    Notation(ValueTokens<'a>),
    Enumeration(ValueTokens<'a>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DefaultDeclarationView<'a> {
    Required,
    Implied,
    Value {
        fixed: bool,
        quoted_literal: &'a str,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AttributeDefinitionView<'a> {
    unit: &'a DtdUnit<'a>,
    meta: &'a AttributeDefinitionMeta,
}

impl<'a> AttributeDefinitionView<'a> {
    pub(crate) fn name(self) -> &'a str {
        self.unit.token_text(self.meta.name)
    }

    pub(crate) fn attribute_type(self) -> AttributeTypeView<'a> {
        match &self.meta.attribute_type {
            AttributeTypeMeta::Cdata => AttributeTypeView::Cdata,
            AttributeTypeMeta::Id => AttributeTypeView::Id,
            AttributeTypeMeta::IdRef => AttributeTypeView::IdRef,
            AttributeTypeMeta::IdRefs => AttributeTypeView::IdRefs,
            AttributeTypeMeta::Entity => AttributeTypeView::Entity,
            AttributeTypeMeta::Entities => AttributeTypeView::Entities,
            AttributeTypeMeta::NmToken => AttributeTypeView::NmToken,
            AttributeTypeMeta::NmTokens => AttributeTypeView::NmTokens,
            AttributeTypeMeta::Notation(range) => AttributeTypeView::Notation(ValueTokens {
                unit: self.unit,
                next: range.start,
                end: range.end,
            }),
            AttributeTypeMeta::Enumeration(range) => AttributeTypeView::Enumeration(ValueTokens {
                unit: self.unit,
                next: range.start,
                end: range.end,
            }),
        }
    }

    pub(crate) fn default(self) -> DefaultDeclarationView<'a> {
        match self.meta.default {
            DefaultMeta::Required => DefaultDeclarationView::Required,
            DefaultMeta::Implied => DefaultDeclarationView::Implied,
            DefaultMeta::Value { fixed, literal } => DefaultDeclarationView::Value {
                fixed,
                quoted_literal: self.unit.token_text(literal),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ValueTokens<'a> {
    unit: &'a DtdUnit<'a>,
    next: usize,
    end: usize,
}

impl<'a> Iterator for ValueTokens<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        while self.next < self.end {
            let index = self.next;
            self.next += 1;
            if matches!(
                self.unit.tokens[index].kind,
                TokenKind::Name | TokenKind::NmToken
            ) {
                return Some(self.unit.token_text(index));
            }
        }
        None
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AttributeDefinitions<'a> {
    unit: &'a DtdUnit<'a>,
    next: usize,
}

impl<'a> Iterator for AttributeDefinitions<'a> {
    type Item = AttributeDefinitionView<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        while self.next < self.unit.tokens.len() {
            let index = self.next;
            self.next += 1;
            if let TokenKind::AttributeDefinition(meta) = &self.unit.tokens[index].kind {
                return Some(AttributeDefinitionView {
                    unit: self.unit,
                    meta,
                });
            }
        }
        None
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AttributeListView<'a> {
    unit: &'a DtdUnit<'a>,
}

impl<'a> AttributeListView<'a> {
    pub(crate) fn element_name(self) -> &'a str {
        self.unit.token_text(0)
    }

    pub(crate) fn definition_count(self) -> usize {
        let TokenKind::AttributeListName { definitions } = self.unit.tokens[0].kind else {
            unreachable!("validated ATTLIST starts with its element-name token")
        };
        definitions
    }

    pub(crate) fn definitions(self) -> AttributeDefinitions<'a> {
        AttributeDefinitions {
            unit: self.unit,
            next: 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EntityDefinitionView<'a> {
    Internal {
        quoted_literal: &'a str,
    },
    External {
        identifier: ExternalIdView<'a>,
        notation: Option<&'a str>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EntityDeclarationView<'a> {
    unit: &'a DtdUnit<'a>,
    kind: DtdUnitKind,
}

impl<'a> EntityDeclarationView<'a> {
    pub(crate) fn is_parameter(self) -> bool {
        matches!(
            self.kind,
            DtdUnitKind::ParameterInternalEntity
                | DtdUnitKind::ParameterSystemEntity
                | DtdUnitKind::ParameterPublicEntity
        )
    }

    pub(crate) fn name(self) -> &'a str {
        self.unit.token_text(0)
    }

    pub(crate) fn definition(self) -> EntityDefinitionView<'a> {
        match self.kind {
            DtdUnitKind::GeneralInternalEntity | DtdUnitKind::ParameterInternalEntity => {
                EntityDefinitionView::Internal {
                    quoted_literal: self.unit.token_text(self.unit.tokens.len() - 1),
                }
            }
            DtdUnitKind::GeneralSystemEntity | DtdUnitKind::ParameterSystemEntity => {
                EntityDefinitionView::External {
                    identifier: ExternalMeta {
                        public: None,
                        system: Some(1),
                    }
                    .view(self.unit),
                    notation: None,
                }
            }
            DtdUnitKind::GeneralPublicEntity | DtdUnitKind::ParameterPublicEntity => {
                EntityDefinitionView::External {
                    identifier: ExternalMeta {
                        public: Some(1),
                        system: Some(2),
                    }
                    .view(self.unit),
                    notation: None,
                }
            }
            DtdUnitKind::GeneralSystemNdataEntity => EntityDefinitionView::External {
                identifier: ExternalMeta {
                    public: None,
                    system: Some(1),
                }
                .view(self.unit),
                notation: Some(self.unit.token_text(self.unit.tokens.len() - 1)),
            },
            DtdUnitKind::GeneralPublicNdataEntity => EntityDefinitionView::External {
                identifier: ExternalMeta {
                    public: Some(1),
                    system: Some(2),
                }
                .view(self.unit),
                notation: Some(self.unit.token_text(self.unit.tokens.len() - 1)),
            },
            _ => unreachable!("view created only for entity kinds"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct NotationDeclarationView<'a> {
    unit: &'a DtdUnit<'a>,
    kind: DtdUnitKind,
}

impl<'a> NotationDeclarationView<'a> {
    pub(crate) fn name(self) -> &'a str {
        self.unit.token_text(0)
    }

    pub(crate) fn identifier(self) -> ExternalIdView<'a> {
        match self.kind {
            DtdUnitKind::SystemNotation => ExternalMeta {
                public: None,
                system: Some(1),
            },
            DtdUnitKind::PublicNotation => ExternalMeta {
                public: Some(1),
                system: None,
            },
            DtdUnitKind::PublicSystemNotation => ExternalMeta {
                public: Some(1),
                system: Some(2),
            },
            _ => unreachable!("view created only for notation kinds"),
        }
        .view(self.unit)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ProcessingInstructionView<'a> {
    pub(crate) target: &'a str,
    pub(crate) data: Option<&'a str>,
}

struct Parser<'a> {
    raw: &'a str,
    position: usize,
    limits: DtdLimits,
    tokens: Vec<DtdToken>,
}

#[derive(Clone, Copy, Debug)]
struct GroupFrame {
    connector: Option<Connector>,
    particles: usize,
}

impl<'a> Parser<'a> {
    fn new(raw: &'a str, limits: DtdLimits) -> Self {
        Self {
            raw,
            position: 0,
            limits,
            tokens: Vec::new(),
        }
    }

    fn parse(mut self) -> Result<(DtdUnitKind, Vec<DtdToken>), DtdError> {
        let kind = if self.raw.starts_with("<!DOCTYPE") {
            self.header()?
        } else if self.raw.starts_with("<!ELEMENT") {
            self.element()?
        } else if self.raw.starts_with("<!ATTLIST") {
            self.attribute_list()?
        } else if self.raw.starts_with("<!ENTITY") {
            self.entity()?
        } else if self.raw.starts_with("<!NOTATION") {
            self.notation()?
        } else if self.raw.starts_with("<!--") {
            self.comment()?
        } else if self.raw.starts_with("<?") {
            self.processing_instruction()?
        } else {
            return Err(DtdError::new(
                DtdErrorKind::Syntax,
                dispatch_error_offset(self.raw),
            ));
        };
        if self.position != self.raw.len() {
            return Err(self.error(DtdErrorKind::Syntax));
        }
        Ok((kind, self.tokens))
    }

    fn push(&mut self, kind: TokenKind, range: Range<usize>) -> Result<usize, DtdError> {
        if self.tokens.len() >= self.limits.max_tokens {
            return Err(DtdError::new(
                DtdErrorKind::LimitExceeded(DtdLimit::Tokens),
                range.start,
            ));
        }
        self.tokens
            .try_reserve(1)
            .map_err(|_| DtdError::new(DtdErrorKind::Allocation, range.start))?;
        let index = self.tokens.len();
        self.tokens.push(DtdToken { kind, range });
        Ok(index)
    }

    fn error(&self, kind: DtdErrorKind) -> DtdError {
        DtdError::new(kind, self.position)
    }

    fn remaining(&self) -> &'a str {
        &self.raw[self.position..]
    }

    fn consume(&mut self, expected: &str) -> Result<(), DtdError> {
        let remaining = self.remaining().as_bytes();
        for (relative, expected) in expected.bytes().enumerate() {
            match remaining.get(relative) {
                Some(actual) if *actual == expected => {}
                Some(_) => {
                    return Err(DtdError::new(
                        DtdErrorKind::Syntax,
                        self.position + relative,
                    ));
                }
                None => {
                    return Err(DtdError::new(DtdErrorKind::Syntax, self.raw.len()));
                }
            }
        }
        self.position += expected.len();
        Ok(())
    }

    fn byte(&self) -> Option<u8> {
        self.raw.as_bytes().get(self.position).copied()
    }

    fn whitespace(&mut self) -> bool {
        let start = self.position;
        while self
            .byte()
            .is_some_and(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r'))
        {
            self.position += 1;
        }
        self.position != start
    }

    fn require_whitespace(&mut self) -> Result<(), DtdError> {
        if self.whitespace() {
            Ok(())
        } else {
            Err(self.error(DtdErrorKind::Syntax))
        }
    }

    fn expected_one_of(&self, expected: &[&str]) -> DtdError {
        DtdError::new(
            DtdErrorKind::Syntax,
            self.position + longest_common_prefix(self.remaining(), expected),
        )
    }

    fn keyword(&mut self, keyword: &str) -> bool {
        if !self.remaining().starts_with(keyword) {
            return false;
        }
        let end = self.position + keyword.len();
        if self.raw[end..]
            .chars()
            .next()
            .is_some_and(|character| name_char(character, true))
        {
            return false;
        }
        self.position = end;
        true
    }

    fn name(&mut self) -> Result<usize, DtdError> {
        let start = self.position;
        let Some(first) = self.remaining().chars().next() else {
            return Err(self.error(DtdErrorKind::InvalidName));
        };
        if !name_start(first, true) {
            return Err(self.error(DtdErrorKind::InvalidName));
        }
        self.position += first.len_utf8();
        while let Some(character) = self.remaining().chars().next() {
            if !name_char(character, true) {
                break;
            }
            self.position += character.len_utf8();
        }
        self.push(TokenKind::Name, start..self.position)
    }

    fn nmtoken(&mut self) -> Result<usize, DtdError> {
        let start = self.position;
        while let Some(character) = self.remaining().chars().next() {
            if !name_char(character, true) {
                break;
            }
            self.position += character.len_utf8();
        }
        if self.position == start {
            return Err(self.error(DtdErrorKind::InvalidNmtoken));
        }
        self.push(TokenKind::NmToken, start..self.position)
    }

    fn header(&mut self) -> Result<DtdUnitKind, DtdError> {
        self.consume("<!DOCTYPE")?;
        self.require_whitespace()?;
        self.name()?;
        let had_whitespace = self.whitespace();
        let external = if !matches!(self.byte(), Some(b'[' | b'>')) {
            if !had_whitespace {
                return Err(self.error(DtdErrorKind::Syntax));
            }
            let external = self.external_identifier(false)?;
            self.whitespace();
            Some(external)
        } else {
            None
        };
        let has_internal_subset = match self.byte() {
            Some(b'[') => {
                self.position += 1;
                true
            }
            Some(b'>') => {
                self.position += 1;
                false
            }
            _ => return Err(self.error(DtdErrorKind::Syntax)),
        };
        Ok(match (external, has_internal_subset) {
            (None, false) => DtdUnitKind::Header,
            (None, true) => DtdUnitKind::HeaderSubset,
            (Some(ExternalMeta { public: None, .. }), false) => DtdUnitKind::HeaderSystem,
            (Some(ExternalMeta { public: None, .. }), true) => DtdUnitKind::HeaderSystemSubset,
            (
                Some(ExternalMeta {
                    public: Some(_), ..
                }),
                false,
            ) => DtdUnitKind::HeaderPublic,
            (
                Some(ExternalMeta {
                    public: Some(_), ..
                }),
                true,
            ) => DtdUnitKind::HeaderPublicSubset,
        })
    }

    fn external_identifier(&mut self, public_only_allowed: bool) -> Result<ExternalMeta, DtdError> {
        if self.keyword("SYSTEM") {
            self.require_whitespace()?;
            let system = self.system_literal()?;
            Ok(ExternalMeta {
                public: None,
                system: Some(system),
            })
        } else if self.keyword("PUBLIC") {
            self.require_whitespace()?;
            let public = self.public_literal()?;
            let before_space = self.position;
            if !self.whitespace() {
                if public_only_allowed {
                    return Ok(ExternalMeta {
                        public: Some(public),
                        system: None,
                    });
                }
                return Err(self.error(DtdErrorKind::Syntax));
            }
            if matches!(self.byte(), Some(b'\'' | b'"')) {
                let system = self.system_literal()?;
                Ok(ExternalMeta {
                    public: Some(public),
                    system: Some(system),
                })
            } else if public_only_allowed {
                self.position = before_space;
                Ok(ExternalMeta {
                    public: Some(public),
                    system: None,
                })
            } else {
                Err(self.error(DtdErrorKind::Syntax))
            }
        } else {
            Err(self.expected_one_of(&["SYSTEM", "PUBLIC"]))
        }
    }

    fn system_literal(&mut self) -> Result<usize, DtdError> {
        self.literal(LiteralKind::SystemIdentifier)
    }

    fn public_literal(&mut self) -> Result<usize, DtdError> {
        self.literal(LiteralKind::PublicIdentifier)
    }

    fn entity_value(&mut self) -> Result<usize, DtdError> {
        self.literal(LiteralKind::EntityValue)
    }

    fn attribute_value(&mut self) -> Result<usize, DtdError> {
        self.literal(LiteralKind::AttributeValue)
    }

    fn literal(&mut self, kind: LiteralKind) -> Result<usize, DtdError> {
        let start = self.position;
        let Some(quote @ (b'\'' | b'"')) = self.byte() else {
            return Err(self.error(DtdErrorKind::Syntax));
        };
        self.position += 1;
        loop {
            let Some(byte) = self.byte() else {
                return Err(self.error(DtdErrorKind::Syntax));
            };
            if byte == quote {
                self.position += 1;
                return self.push(TokenKind::Literal(kind), start..self.position);
            }
            match kind {
                LiteralKind::Comment | LiteralKind::ProcessingInstructionData => {
                    unreachable!("comment and PI data use their own delimiters")
                }
                LiteralKind::SystemIdentifier => {
                    if byte == b'#' {
                        return Err(self.error(DtdErrorKind::SystemIdentifierFragment));
                    }
                    self.advance_character();
                }
                LiteralKind::PublicIdentifier => {
                    let character = self.remaining().chars().next().expect("byte exists");
                    if !pubid_char(character) || (quote == b'\'' && character == '\'') {
                        return Err(self.error(DtdErrorKind::InvalidPublicIdentifier));
                    }
                    self.position += character.len_utf8();
                }
                LiteralKind::EntityValue => match byte {
                    b'%' => {
                        self.reference(ReferenceKind::Parameter)?;
                    }
                    b'&' => {
                        self.reference_from_ampersand()?;
                    }
                    _ => self.advance_character(),
                },
                LiteralKind::AttributeValue => match byte {
                    b'<' => return Err(self.error(DtdErrorKind::Syntax)),
                    b'&' => {
                        self.reference_from_ampersand()?;
                    }
                    _ => self.advance_character(),
                },
            }
        }
    }

    fn advance_character(&mut self) {
        let character = self.remaining().chars().next().expect("byte exists");
        self.position += character.len_utf8();
    }

    fn reference_from_ampersand(&mut self) -> Result<usize, DtdError> {
        if self.remaining().starts_with("&#x") {
            self.character_reference(true)
        } else if self.remaining().starts_with("&#") {
            self.character_reference(false)
        } else {
            self.reference(ReferenceKind::General)
        }
    }

    fn reference(&mut self, kind: ReferenceKind) -> Result<usize, DtdError> {
        let start = self.position;
        let delimiter = match kind {
            ReferenceKind::General => b'&',
            ReferenceKind::Parameter => b'%',
            ReferenceKind::DecimalCharacter | ReferenceKind::HexadecimalCharacter => {
                unreachable!("character references have their own parser")
            }
        };
        if self.byte() != Some(delimiter) {
            return Err(self.error(DtdErrorKind::InvalidReference));
        }
        self.position += 1;
        if let Err(error) = self.name() {
            return Err(if error.kind == DtdErrorKind::InvalidName {
                DtdError::new(DtdErrorKind::InvalidReference, error.offset)
            } else {
                error
            });
        }
        if self.byte() != Some(b';') {
            return Err(self.error(DtdErrorKind::InvalidReference));
        }
        self.position += 1;
        self.push(TokenKind::Reference(kind), start..self.position)
    }

    fn character_reference(&mut self, hexadecimal: bool) -> Result<usize, DtdError> {
        let start = self.position;
        self.position += if hexadecimal { 3 } else { 2 };
        let digits = self.position;
        let mut value = 0_u32;
        while let Some(byte) = self.byte() {
            let digit = if hexadecimal {
                match byte {
                    b'0'..=b'9' => Some(u32::from(byte - b'0')),
                    b'a'..=b'f' => Some(u32::from(byte - b'a') + 10),
                    b'A'..=b'F' => Some(u32::from(byte - b'A') + 10),
                    _ => None,
                }
            } else {
                match byte {
                    b'0'..=b'9' => Some(u32::from(byte - b'0')),
                    _ => None,
                }
            };
            let Some(digit) = digit else { break };
            value = value
                .checked_mul(if hexadecimal { 16 } else { 10 })
                .and_then(|value| value.checked_add(digit))
                .ok_or_else(|| self.error(DtdErrorKind::InvalidReference))?;
            self.position += 1;
        }
        if self.position == digits || self.byte() != Some(b';') {
            return Err(self.error(DtdErrorKind::InvalidReference));
        }
        let character = char::from_u32(value)
            .filter(|character| xml_char(*character))
            .ok_or_else(|| DtdError::new(DtdErrorKind::InvalidReference, start))?;
        let _ = character;
        self.position += 1;
        self.push(
            TokenKind::Reference(if hexadecimal {
                ReferenceKind::HexadecimalCharacter
            } else {
                ReferenceKind::DecimalCharacter
            }),
            start..self.position,
        )
    }

    fn comment(&mut self) -> Result<DtdUnitKind, DtdError> {
        self.consume("<!--")?;
        let content_start = self.position;
        let Some(relative) = self.remaining().find("--") else {
            self.position = self.raw.len();
            return Err(self.error(DtdErrorKind::Syntax));
        };
        self.position += relative;
        if !self.remaining().starts_with("-->") {
            return Err(DtdError::new(DtdErrorKind::Syntax, self.position + 1));
        }
        self.push(
            TokenKind::Literal(LiteralKind::Comment),
            content_start..self.position,
        )?;
        self.position += 3;
        Ok(DtdUnitKind::Comment)
    }

    fn processing_instruction(&mut self) -> Result<DtdUnitKind, DtdError> {
        self.consume("<?")?;
        let target = self.name()?;
        if self.tokens[target]
            .range
            .clone()
            .map(|index| self.raw.as_bytes()[index].to_ascii_lowercase())
            .eq(b"xml".iter().copied())
        {
            return Err(DtdError::new(
                DtdErrorKind::ReservedPiTarget,
                self.tokens[target].range.start,
            ));
        }
        let has_data = if self.remaining().starts_with("?>") {
            false
        } else {
            self.require_whitespace()?;
            let start = self.position;
            let Some(relative) = self.remaining().find("?>") else {
                self.position = self.raw.len();
                return Err(self.error(DtdErrorKind::Syntax));
            };
            self.position += relative;
            self.push(
                TokenKind::Literal(LiteralKind::ProcessingInstructionData),
                start..self.position,
            )?;
            true
        };
        self.consume("?>")?;
        Ok(if has_data {
            DtdUnitKind::ProcessingInstructionData
        } else {
            DtdUnitKind::ProcessingInstruction
        })
    }
}

fn pubid_char(character: char) -> bool {
    matches!(
        character,
        ' ' | '\r'
            | '\n'
            | 'a'..='z'
            | 'A'..='Z'
            | '0'..='9'
            | '-'
            | '\''
            | '('
            | ')'
            | '+'
            | ','
            | '.'
            | '/'
            | ':'
            | '='
            | '?'
            | ';'
            | '!'
            | '*'
            | '#'
            | '@'
            | '$'
            | '_'
            | '%'
    )
}

fn dispatch_error_offset(raw: &str) -> usize {
    longest_common_prefix(
        raw,
        &[
            "<!DOCTYPE",
            "<!ELEMENT",
            "<!ATTLIST",
            "<!ENTITY",
            "<!NOTATION",
            "<!--",
            "<?",
        ],
    )
}

fn longest_common_prefix(raw: &str, expected: &[&str]) -> usize {
    expected
        .iter()
        .map(|candidate| {
            raw.bytes()
                .zip(candidate.bytes())
                .take_while(|(actual, expected)| actual == expected)
                .count()
        })
        .max()
        .unwrap_or(0)
}

impl Parser<'_> {
    fn element(&mut self) -> Result<DtdUnitKind, DtdError> {
        self.consume("<!ELEMENT")?;
        self.require_whitespace()?;
        self.name()?;
        self.require_whitespace()?;
        let kind = if self.keyword("EMPTY") {
            DtdUnitKind::ElementEmpty
        } else if self.keyword("ANY") {
            DtdUnitKind::ElementAny
        } else if self.mixed_ahead() {
            self.mixed_model()?;
            DtdUnitKind::ElementMixed
        } else if self.byte() == Some(b'(') {
            self.children_model()?;
            DtdUnitKind::ElementChildren
        } else {
            return Err(self.expected_one_of(&["EMPTY", "ANY", "("]));
        };
        self.whitespace();
        self.consume(">")?;
        Ok(kind)
    }

    fn mixed_ahead(&self) -> bool {
        let Some(mut tail) = self.remaining().strip_prefix('(') else {
            return false;
        };
        tail = tail.trim_start_matches([' ', '\t', '\n', '\r']);
        tail.starts_with("#PCDATA")
    }

    fn mixed_model(&mut self) -> Result<Range<usize>, DtdError> {
        if self.limits.max_model_depth == 0 {
            return Err(self.error(DtdErrorKind::LimitExceeded(DtdLimit::ModelDepth)));
        }
        let start = self.tokens.len();
        let open = self.position;
        self.consume("(")?;
        self.push(TokenKind::Model(ModelSyntax::Open), open..self.position)?;
        self.whitespace();
        let pcdata = self.position;
        self.consume("#PCDATA")?;
        self.push(TokenKind::Model(ModelSyntax::Pcdata), pcdata..self.position)?;
        self.whitespace();
        let mut names = 0_usize;
        while self.byte() == Some(b'|') {
            let connector = self.position;
            self.position += 1;
            self.push(
                TokenKind::Model(ModelSyntax::Connector(Connector::Choice)),
                connector..self.position,
            )?;
            self.whitespace();
            self.name()?;
            names = names
                .checked_add(1)
                .ok_or_else(|| self.error(DtdErrorKind::Allocation))?;
            self.whitespace();
        }
        let close = self.position;
        self.consume(")")?;
        self.push(TokenKind::Model(ModelSyntax::Close), close..self.position)?;
        if names > 0 || self.byte() == Some(b'*') {
            let occurrence = self.position;
            self.consume("*")?;
            self.push(
                TokenKind::Model(ModelSyntax::Occurrence(Occurrence::ZeroOrMore)),
                occurrence..self.position,
            )?;
        }
        Ok(start..self.tokens.len())
    }

    fn children_model(&mut self) -> Result<Range<usize>, DtdError> {
        let start = self.tokens.len();
        let mut stack = Vec::new();
        self.open_group(&mut stack)?;
        let mut expects_particle = true;
        loop {
            self.whitespace();
            if expects_particle {
                if self.byte() == Some(b'(') {
                    self.open_group(&mut stack)?;
                    continue;
                }
                self.name()?;
                self.occurrence()?;
                let frame = stack.last_mut().expect("root group is active");
                frame.particles = frame
                    .particles
                    .checked_add(1)
                    .ok_or_else(|| self.error(DtdErrorKind::Allocation))?;
                expects_particle = false;
                continue;
            }
            match self.byte() {
                Some(byte @ (b'|' | b',')) => {
                    let connector = if byte == b'|' {
                        Connector::Choice
                    } else {
                        Connector::Sequence
                    };
                    let frame = stack.last_mut().expect("root group is active");
                    if frame
                        .connector
                        .is_some_and(|existing| existing != connector)
                    {
                        return Err(self.error(DtdErrorKind::Syntax));
                    }
                    frame.connector = Some(connector);
                    let offset = self.position;
                    self.position += 1;
                    self.push(
                        TokenKind::Model(ModelSyntax::Connector(connector)),
                        offset..self.position,
                    )?;
                    expects_particle = true;
                }
                Some(b')') => {
                    let frame = stack.last().expect("root group is active");
                    if frame.particles == 0
                        || (frame.connector == Some(Connector::Choice) && frame.particles < 2)
                    {
                        return Err(self.error(DtdErrorKind::Syntax));
                    }
                    stack.pop();
                    let offset = self.position;
                    self.position += 1;
                    self.push(TokenKind::Model(ModelSyntax::Close), offset..self.position)?;
                    self.occurrence()?;
                    if let Some(parent) = stack.last_mut() {
                        parent.particles = parent
                            .particles
                            .checked_add(1)
                            .ok_or_else(|| self.error(DtdErrorKind::Allocation))?;
                    } else {
                        return Ok(start..self.tokens.len());
                    }
                }
                _ => return Err(self.error(DtdErrorKind::Syntax)),
            }
        }
    }

    fn open_group(&mut self, stack: &mut Vec<GroupFrame>) -> Result<(), DtdError> {
        if stack.len() >= self.limits.max_model_depth {
            return Err(self.error(DtdErrorKind::LimitExceeded(DtdLimit::ModelDepth)));
        }
        stack
            .try_reserve(1)
            .map_err(|_| self.error(DtdErrorKind::Allocation))?;
        let offset = self.position;
        self.consume("(")?;
        self.push(TokenKind::Model(ModelSyntax::Open), offset..self.position)?;
        stack.push(GroupFrame {
            connector: None,
            particles: 0,
        });
        Ok(())
    }

    fn occurrence(&mut self) -> Result<(), DtdError> {
        let Some((syntax, occurrence)) = self.byte().and_then(|byte| match byte {
            b'?' => Some((Occurrence::Optional, self.position)),
            b'*' => Some((Occurrence::ZeroOrMore, self.position)),
            b'+' => Some((Occurrence::OneOrMore, self.position)),
            _ => None,
        }) else {
            return Ok(());
        };
        self.position += 1;
        self.push(
            TokenKind::Model(ModelSyntax::Occurrence(syntax)),
            occurrence..self.position,
        )?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/internal/dtd_unit_author.rs"]
mod tests;

#[cfg(test)]
#[path = "../tests/internal/dtd_unit_spec.rs"]
mod spec_tests;

impl Parser<'_> {
    fn entity(&mut self) -> Result<DtdUnitKind, DtdError> {
        self.consume("<!ENTITY")?;
        self.require_whitespace()?;
        let parameter = if self.byte() == Some(b'%') {
            self.position += 1;
            self.require_whitespace()?;
            true
        } else {
            false
        };
        self.name()?;
        self.require_whitespace()?;
        let kind = if matches!(self.byte(), Some(b'\'' | b'"')) {
            self.entity_value()?;
            if parameter {
                DtdUnitKind::ParameterInternalEntity
            } else {
                DtdUnitKind::GeneralInternalEntity
            }
        } else {
            let identifier = self.external_identifier(false)?;
            let had_whitespace = self.whitespace();
            let notation = if !parameter && (had_whitespace && self.byte() != Some(b'>')) {
                self.consume("NDATA")?;
                self.require_whitespace()?;
                Some(self.name()?)
            } else {
                None
            };
            match (parameter, identifier.public.is_some(), notation.is_some()) {
                (true, false, false) => DtdUnitKind::ParameterSystemEntity,
                (true, true, false) => DtdUnitKind::ParameterPublicEntity,
                (false, false, false) => DtdUnitKind::GeneralSystemEntity,
                (false, true, false) => DtdUnitKind::GeneralPublicEntity,
                (false, false, true) => DtdUnitKind::GeneralSystemNdataEntity,
                (false, true, true) => DtdUnitKind::GeneralPublicNdataEntity,
                (true, _, true) => unreachable!("parameter entities cannot have NDATA"),
            }
        };
        self.whitespace();
        self.consume(">")?;
        Ok(kind)
    }

    fn notation(&mut self) -> Result<DtdUnitKind, DtdError> {
        self.consume("<!NOTATION")?;
        self.require_whitespace()?;
        self.name()?;
        self.require_whitespace()?;
        let identifier = self.external_identifier(true)?;
        self.whitespace();
        self.consume(">")?;
        Ok(
            match (identifier.public.is_some(), identifier.system.is_some()) {
                (false, true) => DtdUnitKind::SystemNotation,
                (true, false) => DtdUnitKind::PublicNotation,
                (true, true) => DtdUnitKind::PublicSystemNotation,
                (false, false) => unreachable!("validated external identifier has a literal"),
            },
        )
    }
}

impl Parser<'_> {
    fn attribute_list(&mut self) -> Result<DtdUnitKind, DtdError> {
        self.consume("<!ATTLIST")?;
        self.require_whitespace()?;
        let element = self.name()?;
        let mut definitions = 0_usize;
        loop {
            let had_whitespace = self.whitespace();
            if self.byte() == Some(b'>') {
                self.position += 1;
                break;
            }
            if !had_whitespace {
                return Err(self.error(DtdErrorKind::Syntax));
            }
            if definitions >= self.limits.max_attribute_definitions {
                return Err(self.error(DtdErrorKind::LimitExceeded(DtdLimit::AttributeDefinitions)));
            }
            let definition_start = self.position;
            let name = self.name()?;
            self.require_whitespace()?;
            let attribute_type = self.attribute_type()?;
            self.require_whitespace()?;
            let default = self.default_declaration()?;
            definitions = definitions
                .checked_add(1)
                .ok_or_else(|| self.error(DtdErrorKind::Allocation))?;
            self.push(
                TokenKind::AttributeDefinition(AttributeDefinitionMeta {
                    name,
                    attribute_type,
                    default,
                }),
                definition_start..self.position,
            )?;
        }
        self.tokens[element].kind = TokenKind::AttributeListName { definitions };
        Ok(DtdUnitKind::AttributeList)
    }

    fn attribute_type(&mut self) -> Result<AttributeTypeMeta, DtdError> {
        if self.keyword("CDATA") {
            Ok(AttributeTypeMeta::Cdata)
        } else if self.keyword("IDREFS") {
            Ok(AttributeTypeMeta::IdRefs)
        } else if self.keyword("IDREF") {
            Ok(AttributeTypeMeta::IdRef)
        } else if self.keyword("ID") {
            Ok(AttributeTypeMeta::Id)
        } else if self.keyword("ENTITIES") {
            Ok(AttributeTypeMeta::Entities)
        } else if self.keyword("ENTITY") {
            Ok(AttributeTypeMeta::Entity)
        } else if self.keyword("NMTOKENS") {
            Ok(AttributeTypeMeta::NmTokens)
        } else if self.keyword("NMTOKEN") {
            Ok(AttributeTypeMeta::NmToken)
        } else if self.keyword("NOTATION") {
            self.require_whitespace()?;
            Ok(AttributeTypeMeta::Notation(self.value_list(true)?))
        } else if self.byte() == Some(b'(') {
            Ok(AttributeTypeMeta::Enumeration(self.value_list(false)?))
        } else {
            Err(self.expected_one_of(&[
                "CDATA", "IDREFS", "IDREF", "ID", "ENTITIES", "ENTITY", "NMTOKENS", "NMTOKEN",
                "NOTATION", "(",
            ]))
        }
    }

    fn value_list(&mut self, names: bool) -> Result<Range<usize>, DtdError> {
        self.consume("(")?;
        self.whitespace();
        let start = self.tokens.len();
        if names {
            self.name()?;
        } else {
            self.nmtoken()?;
        }
        loop {
            self.whitespace();
            if self.byte() == Some(b')') {
                self.position += 1;
                return Ok(start..self.tokens.len());
            }
            self.consume("|")?;
            self.whitespace();
            if names {
                self.name()?;
            } else {
                self.nmtoken()?;
            }
        }
    }

    fn default_declaration(&mut self) -> Result<DefaultMeta, DtdError> {
        if self.keyword("#REQUIRED") {
            Ok(DefaultMeta::Required)
        } else if self.keyword("#IMPLIED") {
            Ok(DefaultMeta::Implied)
        } else if self.keyword("#FIXED") {
            self.require_whitespace()?;
            let literal = self.attribute_value()?;
            Ok(DefaultMeta::Value {
                fixed: true,
                literal,
            })
        } else if matches!(self.byte(), Some(b'\'' | b'"')) {
            let literal = self.attribute_value()?;
            Ok(DefaultMeta::Value {
                fixed: false,
                literal,
            })
        } else {
            Err(self.expected_one_of(&["#REQUIRED", "#IMPLIED", "#FIXED", "'", "\""]))
        }
    }
}
