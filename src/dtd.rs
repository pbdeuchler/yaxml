//! One DTD markup unit: a DOCTYPE header, declaration, comment or processing
//! instruction, checked against its grammar and kept as typed token ranges.

use std::borrow::Cow;
use std::ops::Range;

use crate::escape::{invalid_char, xml_char};
use crate::name::{name_char, name_start};
use crate::{Error, ErrorKind, Limit};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LiteralSource {
    Physical,
    Replacement,
}

/// Every DTD-processing ceiling, derived once from the public limits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DtdLimits {
    /// Scanned units, attribute and entity values, references, local tokens.
    pub(crate) token: usize,
    /// One parsed declaration's bytes and grammar tokens.
    pub(crate) unit_bytes: usize,
    pub(crate) unit_tokens: usize,
    pub(crate) model_depth: usize,
    pub(crate) attribute_definitions: usize,
    /// Physical DOCTYPE bytes, and the declarations and bindings it makes.
    pub(crate) declarations: usize,
    /// Retained declaration storage and owned declaration copies.
    pub(crate) storage: usize,
    pub(crate) expansion: usize,
    pub(crate) work: usize,
    pub(crate) references: usize,
    pub(crate) depth: usize,
    /// Token scratch of each content source.
    pub(crate) scratch: usize,
    /// Expansion allowed regardless of input size.
    pub(crate) expansion_floor: usize,
    /// Permitted expansion per consumed input byte above the floor.
    pub(crate) amplification: usize,
    /// The parser options that shape DTD processing.
    pub(crate) flags: crate::options::Flags,
}

impl DtdLimits {
    /// Fixed ceilings of a standalone external DTD read, as `xmlParseDTD`.
    pub(crate) const STANDALONE: Self = Self {
        token: 1024 * 1024,
        unit_bytes: 64 * 1024,
        unit_tokens: 4 * 1024,
        model_depth: 256,
        attribute_definitions: 1024,
        declarations: 16_384,
        storage: 8 * 1024 * 1024,
        expansion: 16 * 1024 * 1024,
        work: 64 * 1024 * 1024,
        references: 1024 * 1024,
        depth: 64,
        scratch: 0,
        expansion_floor: 0,
        amplification: 0,
        flags: crate::options::Flags::LEGACY,
    };

    /// Work ceilings bound scanning and expansion together, so they scale
    /// with both inputs.
    pub(crate) fn new(limits: crate::Limits, decode: crate::DecodeLimits) -> Self {
        let token = limits.max_token_bytes;
        let consumed = limits
            .max_input_bytes
            .saturating_add(limits.max_external_bytes);
        // The largest allowance ever granted; the live one tracks consumption.
        let expansion = limits
            .max_entity_expansion_bytes
            .max(consumed.saturating_mul(limits.max_entity_amplification));
        Self {
            token,
            unit_bytes: token,
            unit_tokens: token,
            model_depth: limits.max_depth,
            attribute_definitions: limits.max_attributes,
            declarations: limits.max_dtd_bytes,
            storage: limits.max_owned_bytes,
            expansion,
            work: consumed.saturating_add(expansion).saturating_mul(8),
            references: limits.max_entity_references,
            depth: limits.max_entity_depth,
            scratch: token.max(decode.max_decoded_token_bytes).saturating_mul(2),
            expansion_floor: limits.max_entity_expansion_bytes,
            amplification: limits.max_entity_amplification,
            flags: crate::options::Flags::LEGACY,
        }
    }

    /// libxml2's `xmlParserEntityCheck` threshold for `consumed` input bytes.
    pub(crate) fn expansion_allowance(&self, consumed: usize) -> usize {
        self.expansion_floor
            .max(consumed.saturating_mul(self.amplification))
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

pub(crate) use crate::valid::AttributeKind;

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DefaultMeta {
    Required,
    Implied,
    Value { fixed: bool, literal: usize },
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AttributeDefinitionMeta {
    name: usize,
    kind: AttributeKind,
    /// Enumerated or notation names; empty for other kinds.
    values: Range<usize>,
    default: DefaultMeta,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum TokenKind {
    Name,
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

/// Token indices of an external identifier's literals.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ExternalMeta {
    public: Option<usize>,
    system: Option<usize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ContentShape {
    Empty,
    Any,
    Mixed,
    Children,
}

/// What a unit is, with the token indices its views need. Names are token 0.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Shape {
    Header {
        external: Option<ExternalMeta>,
        subset: bool,
    },
    Element(ContentShape),
    AttributeList,
    /// An internal entity's literal is its last token; so is an NDATA name.
    Entity {
        parameter: bool,
        external: Option<ExternalMeta>,
        ndata: bool,
    },
    Notation(ExternalMeta),
    Comment,
    Instruction {
        data: bool,
    },
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct DtdUnit<'a> {
    raw: Cow<'a, str>,
    tokens: Vec<DtdToken>,
    shape: Shape,
}

impl<'a> DtdUnit<'a> {
    pub(crate) fn parse(raw: impl Into<Cow<'a, str>>, limits: DtdLimits) -> Result<Self, Error> {
        Self::parse_recovering(raw, limits, &mut Err).map(|(unit, _)| unit)
    }

    /// Parses a unit, `recover`ing from a missing space where libxml2 reads
    /// on (after `<!DOCTYPE`, `SYSTEM`, `PUBLIC`, a public identifier,
    /// `<!ENTITY`, `%`, an entity or element name, around `NDATA`,
    /// `<!ATTLIST`, `#FIXED` and a PI target). From a later error, recovery
    /// keeps what libxml2 declared before it (an entity whose value was
    /// read, the attribute definitions read) and returns where it failed.
    pub(crate) fn parse_recovering(
        raw: impl Into<Cow<'a, str>>,
        limits: DtdLimits,
        recover: &mut dyn FnMut(Error) -> Result<(), Error>,
    ) -> Result<(Self, Option<usize>), Error> {
        let raw = raw.into();
        if raw.len() > limits.unit_bytes {
            return Err(Error::new(
                ErrorKind::LimitExceeded(Limit::TokenBytes),
                limits.unit_bytes,
            ));
        }
        if let Some(offset) = invalid_char(&raw) {
            return Err(Error::new(ErrorKind::InvalidCharacter, offset));
        }
        let (raw, (shape, tokens), cut) = match Parser::new(&raw, limits, recover).parse() {
            Ok(parsed) => (raw, parsed, None),
            Err((error, Some(declared))) => {
                recover(error)?;
                let mut text = String::new();
                text.try_reserve_exact(declared + 1)
                    .map_err(|_| Error::new(ErrorKind::Allocation, declared))?;
                text.push_str(&raw[..declared]);
                text.push('>');
                let parsed = Parser::new(&text, limits, &mut |_| Ok(())).parse();
                let parsed = parsed.map_err(|(error, _)| error)?;
                (Cow::Owned(text), parsed, Some(error.offset))
            }
            Err((error, None)) => return Err(error),
        };
        Ok((Self { raw, tokens, shape }, cut))
    }

    pub(crate) fn raw(&self) -> &str {
        &self.raw
    }

    /// Retain a validated unit before mutating the catalog that supplied its text.
    /// Existing owned text and token metadata move without copying. A borrowed
    /// raw value spends the shared owned-string budget before allocation.
    pub(crate) fn try_into_owned(
        self,
        budget: &mut crate::parser::Budget,
        offset: usize,
    ) -> Result<DtdUnit<'static>, Error> {
        let raw = match self.raw {
            Cow::Owned(raw) => raw,
            Cow::Borrowed(raw) => {
                budget.charge_owned(raw.len(), offset)?;
                let mut copied = String::new();
                copied
                    .try_reserve_exact(raw.len())
                    .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
                copied.push_str(raw);
                copied
            }
        };
        Ok(DtdUnit {
            raw: Cow::Owned(raw),
            tokens: self.tokens,
            shape: self.shape,
        })
    }

    pub(crate) fn view(&self) -> DtdUnitView<'_> {
        match self.shape {
            Shape::Header { external, subset } => DtdUnitView::Header(DoctypeHeaderView {
                unit: self,
                external,
                subset,
            }),
            Shape::Element(content) => DtdUnitView::Element(ElementDeclarationView {
                unit: self,
                content,
            }),
            Shape::AttributeList => DtdUnitView::AttributeList(AttributeListView { unit: self }),
            Shape::Entity {
                parameter,
                external,
                ndata,
            } => DtdUnitView::Entity(EntityDeclarationView {
                unit: self,
                parameter,
                external,
                ndata,
            }),
            Shape::Notation(external) => DtdUnitView::Notation(NotationDeclarationView {
                unit: self,
                external,
            }),
            Shape::Comment => DtdUnitView::Comment(self.token_text(0)),
            Shape::Instruction { data } => {
                DtdUnitView::ProcessingInstruction(ProcessingInstructionView {
                    target: self.token_text(0),
                    data: data.then(|| self.token_text(1)),
                })
            }
        }
    }

    pub(crate) fn reference_positions(&self) -> impl Iterator<Item = (usize, ReferenceView<'_>)> {
        self.tokens.iter().filter_map(|token| match token.kind {
            TokenKind::Reference(kind) => Some((
                token.range.start,
                ReferenceView {
                    kind,
                    spelling: &self.raw[token.range.clone()],
                },
            )),
            _ => None,
        })
    }

    fn token_text(&self, index: usize) -> &str {
        &self.raw[self.tokens[index].range.clone()]
    }

    /// Token `index`'s text and offset in the unit.
    fn literal(&self, index: usize) -> Literal<'_> {
        Literal {
            quoted: self.token_text(index),
            offset: self.tokens[index].range.start,
        }
    }

    /// The name and model tokens in `range`.
    fn tokens(&self, range: Range<usize>) -> impl Iterator<Item = (&TokenKind, &str)> {
        self.tokens[range]
            .iter()
            .map(|token| (&token.kind, &self.raw[token.range.clone()]))
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

/// A quoted literal and its offset in the unit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Literal<'a> {
    pub(crate) quoted: &'a str,
    pub(crate) offset: usize,
}

impl<'a> Literal<'a> {
    /// The text between the quotes, and its offset in the unit.
    pub(crate) fn content(self) -> (&'a str, usize) {
        (&self.quoted[1..self.quoted.len() - 1], self.offset + 1)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ExternalIdView<'a> {
    pub(crate) public: Option<&'a str>,
    pub(crate) system: Option<&'a str>,
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
pub(crate) struct DoctypeHeaderView<'a> {
    unit: &'a DtdUnit<'a>,
    external: Option<ExternalMeta>,
    subset: bool,
}

impl<'a> DoctypeHeaderView<'a> {
    pub(crate) fn root_name(self) -> &'a str {
        self.unit.token_text(0)
    }

    pub(crate) fn external_id(self) -> Option<ExternalIdView<'a>> {
        self.external.map(|external| external.view(self.unit))
    }

    pub(crate) fn has_internal_subset(self) -> bool {
        self.subset
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
    content: ContentShape,
}

impl<'a> ElementDeclarationView<'a> {
    pub(crate) fn name(self) -> &'a str {
        self.unit.token_text(0)
    }

    pub(crate) fn content(self) -> ElementContentView<'a> {
        let model = ModelTokens {
            unit: self.unit,
            next: 1,
        };
        match self.content {
            ContentShape::Empty => ElementContentView::Empty,
            ContentShape::Any => ElementContentView::Any,
            ContentShape::Mixed => ElementContentView::Mixed(model),
            ContentShape::Children => ElementContentView::Children(model),
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
}

impl<'a> Iterator for ModelTokens<'a> {
    type Item = ModelTokenView<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let (kind, text) = self.unit.tokens(self.next..self.unit.tokens.len()).next()?;
        self.next += 1;
        Some(match *kind {
            TokenKind::Model(ModelSyntax::Open) => ModelTokenView::Open,
            TokenKind::Model(ModelSyntax::Close) => ModelTokenView::Close,
            TokenKind::Model(ModelSyntax::Pcdata) => ModelTokenView::Pcdata,
            TokenKind::Model(ModelSyntax::Connector(connector)) => {
                ModelTokenView::Connector(connector)
            }
            TokenKind::Model(ModelSyntax::Occurrence(occurrence)) => {
                ModelTokenView::Occurrence(occurrence)
            }
            _ => ModelTokenView::Name(text),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DefaultDeclarationView<'a> {
    Required,
    Implied,
    Value { fixed: bool, literal: Literal<'a> },
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

    pub(crate) fn kind(self) -> AttributeKind {
        self.meta.kind
    }

    /// The enumerated or notation names; none for other kinds.
    pub(crate) fn values(self) -> impl Iterator<Item = &'a str> {
        self.unit
            .tokens(self.meta.values.clone())
            .map(|(_, text)| text)
    }

    pub(crate) fn default(self) -> DefaultDeclarationView<'a> {
        match self.meta.default {
            DefaultMeta::Required => DefaultDeclarationView::Required,
            DefaultMeta::Implied => DefaultDeclarationView::Implied,
            DefaultMeta::Value { fixed, literal } => DefaultDeclarationView::Value {
                fixed,
                literal: self.unit.literal(literal),
            },
        }
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

    pub(crate) fn definitions(self) -> impl Iterator<Item = AttributeDefinitionView<'a>> {
        let unit = self.unit;
        unit.tokens
            .iter()
            .filter_map(move |token| match &token.kind {
                TokenKind::AttributeDefinition(meta) => {
                    Some(AttributeDefinitionView { unit, meta })
                }
                _ => None,
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EntityDefinitionView<'a> {
    Internal(Literal<'a>),
    External {
        identifier: ExternalIdView<'a>,
        notation: Option<&'a str>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EntityDeclarationView<'a> {
    unit: &'a DtdUnit<'a>,
    parameter: bool,
    external: Option<ExternalMeta>,
    ndata: bool,
}

impl<'a> EntityDeclarationView<'a> {
    pub(crate) fn is_parameter(self) -> bool {
        self.parameter
    }

    pub(crate) fn name(self) -> &'a str {
        self.unit.token_text(0)
    }

    pub(crate) fn definition(self) -> EntityDefinitionView<'a> {
        let last = self.unit.tokens.len() - 1;
        match self.external {
            None => EntityDefinitionView::Internal(self.unit.literal(last)),
            Some(external) => EntityDefinitionView::External {
                identifier: external.view(self.unit),
                notation: self.ndata.then(|| self.unit.token_text(last)),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct NotationDeclarationView<'a> {
    unit: &'a DtdUnit<'a>,
    external: ExternalMeta,
}

impl<'a> NotationDeclarationView<'a> {
    pub(crate) fn name(self) -> &'a str {
        self.unit.token_text(0)
    }

    pub(crate) fn identifier(self) -> ExternalIdView<'a> {
        self.external.view(self.unit)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ProcessingInstructionView<'a> {
    pub(crate) target: &'a str,
    pub(crate) data: Option<&'a str>,
}

struct Parser<'a, 'r> {
    raw: &'a str,
    position: usize,
    limits: DtdLimits,
    tokens: Vec<DtdToken>,
    recover: &'r mut dyn FnMut(Error) -> Result<(), Error>,
    /// The end of what libxml2 has declared so far.
    declared: Option<usize>,
}

impl<'a, 'r> Parser<'a, 'r> {
    fn new(
        raw: &'a str,
        limits: DtdLimits,
        recover: &'r mut dyn FnMut(Error) -> Result<(), Error>,
    ) -> Self {
        Self {
            raw,
            position: 0,
            limits,
            tokens: Vec::new(),
            recover,
            declared: None,
        }
    }

    /// The unit, or its error and the end of what libxml2 declared before.
    fn parse(mut self) -> Result<(Shape, Vec<DtdToken>), (Error, Option<usize>)> {
        match self.unit() {
            Ok(shape) => Ok((shape, self.tokens)),
            Err(error) => Err((error, self.declared)),
        }
    }

    fn unit(&mut self) -> Result<Shape, Error> {
        let shape = if self.raw.starts_with("<!DOCTYPE") {
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
            return Err(Error::new(
                ErrorKind::InvalidDtd,
                dispatch_error_offset(self.raw),
            ));
        };
        if self.position != self.raw.len() {
            return Err(self.error(ErrorKind::InvalidDtd));
        }
        Ok(shape)
    }

    fn push(&mut self, kind: TokenKind, range: Range<usize>) -> Result<usize, Error> {
        if self.tokens.len() >= self.limits.unit_tokens {
            return Err(Error::new(
                ErrorKind::LimitExceeded(Limit::TokenBytes),
                range.start,
            ));
        }
        self.tokens
            .try_reserve(1)
            .map_err(|_| Error::new(ErrorKind::Allocation, range.start))?;
        let index = self.tokens.len();
        self.tokens.push(DtdToken { kind, range });
        Ok(index)
    }

    /// Consumes `spelling` as one model token.
    fn model(&mut self, syntax: ModelSyntax, spelling: &str) -> Result<(), Error> {
        let start = self.position;
        self.consume(spelling)?;
        self.push(TokenKind::Model(syntax), start..self.position)
            .map(drop)
    }

    fn error(&self, kind: ErrorKind) -> Error {
        Error::new(kind, self.position)
    }

    fn remaining(&self) -> &'a str {
        &self.raw[self.position..]
    }

    fn consume(&mut self, expected: &str) -> Result<(), Error> {
        let remaining = self.remaining().as_bytes();
        for (relative, expected) in expected.bytes().enumerate() {
            match remaining.get(relative) {
                Some(actual) if *actual == expected => {}
                Some(_) => {
                    return Err(Error::new(ErrorKind::InvalidDtd, self.position + relative));
                }
                None => {
                    return Err(Error::new(ErrorKind::InvalidDtd, self.raw.len()));
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

    fn require_whitespace(&mut self) -> Result<(), Error> {
        match self.whitespace() {
            true => Ok(()),
            false => Err(self.error(ErrorKind::InvalidDtd)),
        }
    }

    /// Whitespace libxml2 requires but reads on without.
    fn expect_whitespace(&mut self) -> Result<(), Error> {
        match self.whitespace() {
            true => Ok(()),
            false => self.soft(ErrorKind::InvalidDtd),
        }
    }

    /// An error libxml2 reads on from.
    fn soft(&mut self, kind: ErrorKind) -> Result<(), Error> {
        let error = self.error(kind);
        (self.recover)(error)
    }

    fn expected_one_of(&self, expected: &[&str]) -> Error {
        Error::new(
            ErrorKind::InvalidDtd,
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

    /// Steps over name characters; whether any were found.
    fn name_characters(&mut self) -> bool {
        let length = crate::name::name_chars_length(self.remaining(), true);
        self.position += length;
        length != 0
    }

    fn name(&mut self) -> Result<usize, Error> {
        let start = self.position;
        if !self
            .remaining()
            .chars()
            .next()
            .is_some_and(|first| name_start(first, true))
        {
            return Err(self.error(ErrorKind::InvalidName));
        }
        self.name_characters();
        self.push(TokenKind::Name, start..self.position)
    }

    fn nmtoken(&mut self) -> Result<usize, Error> {
        let start = self.position;
        if !self.name_characters() {
            return Err(self.error(ErrorKind::InvalidDtd));
        }
        self.push(TokenKind::NmToken, start..self.position)
    }

    fn header(&mut self) -> Result<Shape, Error> {
        self.consume("<!DOCTYPE")?;
        self.expect_whitespace()?;
        self.name()?;
        let had_whitespace = self.whitespace();
        let external = if !matches!(self.byte(), Some(b'[' | b'>')) {
            if !had_whitespace {
                return Err(self.error(ErrorKind::InvalidDtd));
            }
            let external = self.external_identifier(false)?;
            self.whitespace();
            Some(external)
        } else {
            None
        };
        let subset = match self.byte() {
            Some(b'[') => true,
            Some(b'>') => false,
            _ => return Err(self.error(ErrorKind::InvalidDtd)),
        };
        self.position += 1;
        Ok(Shape::Header { external, subset })
    }

    fn external_identifier(&mut self, public_only_allowed: bool) -> Result<ExternalMeta, Error> {
        if self.keyword("SYSTEM") {
            self.expect_whitespace()?;
            let system = self.literal(LiteralKind::SystemIdentifier)?;
            return Ok(ExternalMeta {
                public: None,
                system: Some(system),
            });
        }
        if !self.keyword("PUBLIC") {
            return Err(self.expected_one_of(&["SYSTEM", "PUBLIC"]));
        }
        self.expect_whitespace()?;
        let public = Some(self.literal(LiteralKind::PublicIdentifier)?);
        let public_only = ExternalMeta {
            public,
            system: None,
        };
        let before_space = self.position;
        if !self.whitespace() {
            if public_only_allowed {
                return Ok(public_only);
            }
            self.soft(ErrorKind::InvalidDtd)?;
        }
        if matches!(self.byte(), Some(b'\'' | b'"')) {
            let system = self.literal(LiteralKind::SystemIdentifier)?;
            Ok(ExternalMeta {
                public,
                system: Some(system),
            })
        } else if public_only_allowed {
            self.position = before_space;
            Ok(public_only)
        } else {
            Err(self.error(ErrorKind::InvalidDtd))
        }
    }

    fn literal(&mut self, kind: LiteralKind) -> Result<usize, Error> {
        let start = self.position;
        let Some(quote @ (b'\'' | b'"')) = self.byte() else {
            return Err(self.error(ErrorKind::InvalidDtd));
        };
        self.position += 1;
        loop {
            let Some(byte) = self.byte() else {
                return Err(self.error(ErrorKind::InvalidDtd));
            };
            if byte == quote {
                self.position += 1;
                return self.push(TokenKind::Literal(kind), start..self.position);
            }
            // Bytes no rule below reads are taken as one run.
            let rest = self.remaining().as_bytes();
            let run = match kind {
                LiteralKind::EntityValue => memchr::memchr3(quote, b'%', b'&', rest),
                LiteralKind::AttributeValue => memchr::memchr3(quote, b'<', b'&', rest),
                LiteralKind::SystemIdentifier => memchr::memchr2(quote, b'#', rest),
                _ => Some(0),
            };
            if let Some(run @ 1..) = run {
                self.position += run;
                continue;
            }
            let character = self.remaining().chars().next().expect("byte exists");
            match (kind, byte) {
                (LiteralKind::SystemIdentifier, b'#') => {
                    return Err(self.error(ErrorKind::InvalidDtd));
                }
                // The closing quote returned above.
                (LiteralKind::PublicIdentifier, _) if !pubid_char(character) => {
                    return Err(self.error(ErrorKind::InvalidDtd));
                }
                (LiteralKind::EntityValue, b'%') => {
                    self.reference(ReferenceKind::Parameter)?;
                }
                (LiteralKind::AttributeValue, b'<') => {
                    return Err(self.error(ErrorKind::InvalidDtd));
                }
                (LiteralKind::EntityValue | LiteralKind::AttributeValue, b'&') => {
                    match self.remaining().as_bytes().get(1) {
                        Some(b'#') => self.character_reference()?,
                        _ => self.reference(ReferenceKind::General)?,
                    };
                }
                _ => self.position += character.len_utf8(),
            }
        }
    }

    fn reference(&mut self, kind: ReferenceKind) -> Result<usize, Error> {
        let start = self.position;
        self.position += 1;
        if let Err(error) = self.name() {
            return Err(match error.kind {
                ErrorKind::InvalidName => Error::new(ErrorKind::InvalidReference, error.offset),
                _ => error,
            });
        }
        if self.byte() != Some(b';') {
            return Err(self.error(ErrorKind::InvalidReference));
        }
        self.position += 1;
        self.push(TokenKind::Reference(kind), start..self.position)
    }

    fn character_reference(&mut self) -> Result<usize, Error> {
        let start = self.position;
        let hexadecimal = self.remaining().starts_with("&#x");
        let (radix, kind) = match hexadecimal {
            true => (16, ReferenceKind::HexadecimalCharacter),
            false => (10, ReferenceKind::DecimalCharacter),
        };
        self.position += if hexadecimal { 3 } else { 2 };
        let digits = self.position;
        let mut value = 0_u32;
        while let Some(digit) = self
            .byte()
            .and_then(|byte| char::from(byte).to_digit(radix))
        {
            value = value
                .checked_mul(radix)
                .and_then(|value| value.checked_add(digit))
                .ok_or_else(|| self.error(ErrorKind::InvalidReference))?;
            self.position += 1;
        }
        if self.position == digits || self.byte() != Some(b';') {
            return Err(self.error(ErrorKind::InvalidReference));
        }
        if !char::from_u32(value).is_some_and(xml_char) {
            return Err(Error::new(ErrorKind::InvalidReference, start));
        }
        self.position += 1;
        self.push(TokenKind::Reference(kind), start..self.position)
    }

    /// Moves to the next `close`; the range moved over.
    fn until(&mut self, close: &str) -> Result<Range<usize>, Error> {
        let start = self.position;
        let Some(relative) = self.remaining().find(close) else {
            self.position = self.raw.len();
            return Err(self.error(ErrorKind::InvalidDtd));
        };
        self.position += relative;
        Ok(start..self.position)
    }

    fn comment(&mut self) -> Result<Shape, Error> {
        self.consume("<!--")?;
        let text = self.until("--")?;
        if !self.remaining().starts_with("-->") {
            return Err(Error::new(ErrorKind::InvalidDtd, self.position + 1));
        }
        self.push(TokenKind::Literal(LiteralKind::Comment), text)?;
        self.position += 3;
        Ok(Shape::Comment)
    }

    fn processing_instruction(&mut self) -> Result<Shape, Error> {
        self.consume("<?")?;
        let target = self.name()?;
        let target = self.tokens[target].range.clone();
        if self.raw[target.clone()].eq_ignore_ascii_case("xml") {
            return Err(Error::new(ErrorKind::InvalidDtd, target.start));
        }
        let data = !self.remaining().starts_with("?>");
        if data {
            self.expect_whitespace()?;
            let text = self.until("?>")?;
            self.push(
                TokenKind::Literal(LiteralKind::ProcessingInstructionData),
                text,
            )?;
        }
        self.consume("?>")?;
        Ok(Shape::Instruction { data })
    }

    fn element(&mut self) -> Result<Shape, Error> {
        self.consume("<!ELEMENT")?;
        self.require_whitespace()?;
        self.name()?;
        self.expect_whitespace()?;
        let content = if self.keyword("EMPTY") {
            ContentShape::Empty
        } else if self.keyword("ANY") {
            ContentShape::Any
        } else if self.mixed_ahead() {
            self.mixed_model()?;
            ContentShape::Mixed
        } else if self.byte() == Some(b'(') {
            self.children_model()?;
            ContentShape::Children
        } else {
            return Err(self.expected_one_of(&["EMPTY", "ANY", "("]));
        };
        self.whitespace();
        self.consume(">")?;
        Ok(Shape::Element(content))
    }

    fn mixed_ahead(&self) -> bool {
        self.remaining().strip_prefix('(').is_some_and(|tail| {
            tail.trim_start_matches([' ', '\t', '\n', '\r'])
                .starts_with("#PCDATA")
        })
    }

    fn mixed_model(&mut self) -> Result<(), Error> {
        if self.limits.model_depth == 0 {
            return Err(self.error(ErrorKind::LimitExceeded(Limit::Depth)));
        }
        self.model(ModelSyntax::Open, "(")?;
        self.whitespace();
        self.model(ModelSyntax::Pcdata, "#PCDATA")?;
        self.whitespace();
        let mut names = false;
        while self.byte() == Some(b'|') {
            self.model(ModelSyntax::Connector(Connector::Choice), "|")?;
            self.whitespace();
            self.name()?;
            names = true;
            self.whitespace();
        }
        self.model(ModelSyntax::Close, ")")?;
        if names || self.byte() == Some(b'*') {
            self.model(ModelSyntax::Occurrence(Occurrence::ZeroOrMore), "*")?;
        }
        Ok(())
    }

    fn children_model(&mut self) -> Result<(), Error> {
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
                expects_particle = false;
                continue;
            }
            match self.byte() {
                Some(byte @ (b'|' | b',')) => {
                    let (connector, spelling) = match byte {
                        b'|' => (Connector::Choice, "|"),
                        _ => (Connector::Sequence, ","),
                    };
                    // Each open group's connector, once it has one.
                    let group = stack.last_mut().expect("root group is active");
                    if group.is_some_and(|existing| existing != connector) {
                        return Err(self.error(ErrorKind::InvalidDtd));
                    }
                    *group = Some(connector);
                    self.model(ModelSyntax::Connector(connector), spelling)?;
                    expects_particle = true;
                }
                // A particle precedes every `)` and follows every connector.
                Some(b')') => {
                    stack.pop();
                    self.model(ModelSyntax::Close, ")")?;
                    self.occurrence()?;
                    if stack.is_empty() {
                        return Ok(());
                    }
                }
                _ => return Err(self.error(ErrorKind::InvalidDtd)),
            }
        }
    }

    fn open_group(&mut self, stack: &mut Vec<Option<Connector>>) -> Result<(), Error> {
        if stack.len() >= self.limits.model_depth {
            return Err(self.error(ErrorKind::LimitExceeded(Limit::Depth)));
        }
        stack
            .try_reserve(1)
            .map_err(|_| self.error(ErrorKind::Allocation))?;
        self.model(ModelSyntax::Open, "(")?;
        stack.push(None);
        Ok(())
    }

    fn occurrence(&mut self) -> Result<(), Error> {
        let (occurrence, spelling) = match self.byte() {
            Some(b'?') => (Occurrence::Optional, "?"),
            Some(b'*') => (Occurrence::ZeroOrMore, "*"),
            Some(b'+') => (Occurrence::OneOrMore, "+"),
            _ => return Ok(()),
        };
        self.model(ModelSyntax::Occurrence(occurrence), spelling)
    }

    fn entity(&mut self) -> Result<Shape, Error> {
        self.consume("<!ENTITY")?;
        self.expect_whitespace()?;
        let parameter = self.byte() == Some(b'%');
        if parameter {
            self.position += 1;
            self.expect_whitespace()?;
        }
        self.name()?;
        self.expect_whitespace()?;
        let (external, ndata) = if matches!(self.byte(), Some(b'\'' | b'"')) {
            self.literal(LiteralKind::EntityValue)?;
            (None, false)
        } else {
            let identifier = self.external_identifier(false)?;
            let ndata = !parameter
                && !self
                    .remaining()
                    .trim_start_matches([' ', '\t', '\n', '\r'])
                    .starts_with('>');
            if ndata {
                self.expect_whitespace()?;
                self.consume("NDATA")?;
                self.expect_whitespace()?;
                self.name()?;
            }
            (Some(identifier), ndata)
        };
        self.declared = Some(self.position);
        self.whitespace();
        self.consume(">")?;
        Ok(Shape::Entity {
            parameter,
            external,
            ndata,
        })
    }

    fn notation(&mut self) -> Result<Shape, Error> {
        self.consume("<!NOTATION")?;
        self.require_whitespace()?;
        self.name()?;
        self.require_whitespace()?;
        let external = self.external_identifier(true)?;
        self.whitespace();
        self.consume(">")?;
        Ok(Shape::Notation(external))
    }

    fn attribute_list(&mut self) -> Result<Shape, Error> {
        self.consume("<!ATTLIST")?;
        self.expect_whitespace()?;
        self.name()?;
        self.declared = Some(self.position);
        let mut definitions = 0_usize;
        loop {
            let had_whitespace = self.whitespace();
            if self.byte() == Some(b'>') {
                self.position += 1;
                return Ok(Shape::AttributeList);
            }
            if !had_whitespace {
                return Err(self.error(ErrorKind::InvalidDtd));
            }
            if definitions >= self.limits.attribute_definitions {
                return Err(self.error(ErrorKind::LimitExceeded(Limit::Attributes)));
            }
            let definition_start = self.position;
            let name = self.name()?;
            self.require_whitespace()?;
            let (kind, values) = self.attribute_type()?;
            self.require_whitespace()?;
            let default = self.default_declaration()?;
            definitions += 1;
            self.push(
                TokenKind::AttributeDefinition(AttributeDefinitionMeta {
                    name,
                    kind,
                    values,
                    default,
                }),
                definition_start..self.position,
            )?;
            self.declared = Some(self.position);
        }
    }

    fn attribute_type(&mut self) -> Result<(AttributeKind, Range<usize>), Error> {
        const KEYWORDS: [(&str, AttributeKind); 8] = [
            ("CDATA", AttributeKind::Cdata),
            ("IDREFS", AttributeKind::IdRefs),
            ("IDREF", AttributeKind::IdRef),
            ("ID", AttributeKind::Id),
            ("ENTITIES", AttributeKind::Entities),
            ("ENTITY", AttributeKind::Entity),
            ("NMTOKENS", AttributeKind::NmTokens),
            ("NMTOKEN", AttributeKind::NmToken),
        ];
        let none = self.tokens.len()..self.tokens.len();
        if let Some(&(_, kind)) = KEYWORDS.iter().find(|(keyword, _)| self.keyword(keyword)) {
            Ok((kind, none))
        } else if self.keyword("NOTATION") {
            self.require_whitespace()?;
            Ok((AttributeKind::Notation, self.value_list(true)?))
        } else if self.byte() == Some(b'(') {
            Ok((AttributeKind::Enumeration, self.value_list(false)?))
        } else {
            Err(self.expected_one_of(&[
                "CDATA", "IDREFS", "IDREF", "ID", "ENTITIES", "ENTITY", "NMTOKENS", "NMTOKEN",
                "NOTATION", "(",
            ]))
        }
    }

    fn value_list(&mut self, names: bool) -> Result<Range<usize>, Error> {
        self.consume("(")?;
        let start = self.tokens.len();
        loop {
            self.whitespace();
            match names {
                true => self.name()?,
                false => self.nmtoken()?,
            };
            self.whitespace();
            if self.byte() == Some(b')') {
                self.position += 1;
                return Ok(start..self.tokens.len());
            }
            self.consume("|")?;
        }
    }

    fn default_declaration(&mut self) -> Result<DefaultMeta, Error> {
        if self.keyword("#REQUIRED") {
            Ok(DefaultMeta::Required)
        } else if self.keyword("#IMPLIED") {
            Ok(DefaultMeta::Implied)
        } else if self.keyword("#FIXED") {
            self.expect_whitespace()?;
            let literal = self.literal(LiteralKind::AttributeValue)?;
            Ok(DefaultMeta::Value {
                fixed: true,
                literal,
            })
        } else if matches!(self.byte(), Some(b'\'' | b'"')) {
            let literal = self.literal(LiteralKind::AttributeValue)?;
            Ok(DefaultMeta::Value {
                fixed: false,
                literal,
            })
        } else {
            Err(self.expected_one_of(&["#REQUIRED", "#IMPLIED", "#FIXED", "'", "\""]))
        }
    }
}

pub(crate) fn pubid_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || " \r\n-'()+,./:=?;!*#@$_%".contains(character)
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

#[cfg(test)]
#[path = "../tests/internal/dtd_unit_spec.rs"]
mod spec_tests;
