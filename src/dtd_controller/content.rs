//! General-entity content composition. Stable input and replacement text
//! are complete, so their tokens are scanned in place; streamed input is
//! tokenized incrementally. Physical events lend scratch, generated events own.
use std::borrow::Cow;
use std::ops::Range;

use memchr::{memchr, memchr2};

use super::*;
use crate::dtd_frames::{ByteSpan, EntityFrame, EntityId};
use crate::dtd_reference::{ReferenceKind, classify};
use crate::parser::{ParserState, owned_event};
use crate::tokenizer::text::{TextCoordinator, TextOutcome, Work};
use crate::tokenizer::{TokenKind, Tokenizer, markup_after_lt};
use crate::valid::Code;
use crate::{Event, SourceOrigin};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ContentOutcome {
    EventReady,
    NeedPhysicalInput,
    Progress,
    Complete,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ContentStep {
    /// Newly accepted decoded input bytes; replacement-only transitions use zero.
    pub(crate) consumed: usize,
    pub(crate) outcome: ContentOutcome,
}

/// A published event, borrowing source storage until acknowledged.
pub(crate) struct LentContent<'event> {
    pub(crate) event: Option<Event<'event>>,
    pub(crate) origin: FragmentOrigin,
    pub(crate) budget: &'event mut Budget,
}

/// An entity reference boundary, published when references are kept.
enum Marker {
    Start(String),
    End,
}

enum Source<'input> {
    /// Complete UTF-8 input, read from `DtdBodyState::decoded_position`.
    Stable(&'input str, TokenLimit),
    Stream(Box<TextCoordinator>),
}

/// A token ceiling: its bytes and the limit reported beyond them.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TokenLimit {
    bytes: usize,
    kind: Limit,
}

impl TokenLimit {
    /// The tighter of the source and decoded ceilings of UTF-8 text.
    pub(crate) fn new(source: usize, decoded: usize) -> Self {
        match decoded < source {
            true => Self {
                bytes: decoded,
                kind: Limit::DecodedTokenBytes,
            },
            false => Self {
                bytes: source,
                kind: Limit::TokenBytes,
            },
        }
    }
    fn error(self, offset: usize) -> Error {
        Error::new(ErrorKind::LimitExceeded(self.kind), offset)
    }
}

/// The streamed token ready for publication.
#[derive(Clone)]
enum Ready {
    /// The coordinator's token.
    Stream,
    /// Bytes of the innermost entity's replacement text.
    Replacement(TokenKind, Range<usize>),
}

pub(crate) struct ContentControllerCore<'input> {
    body: DtdBodyState,
    parser: ParserState<'input>,
    source: Source<'input>,
    work: Work,
    ready: Option<Ready>,
    /// A replacement token's or reference name's text, copied out of the
    /// catalog that publication updates.
    scratch: String,
    finalizing: bool,
    /// Report entity references as `EntityStart`/`EntityEnd` (no `NOENT`).
    markers: bool,
    /// A boundary to publish before any further token.
    pending: Option<(Marker, FragmentOrigin)>,
    /// The `End` of a pending `Start` whose reference expands to nothing.
    queued: Option<FragmentOrigin>,
    /// Origins of the references being expanded, for their `End`s.
    open: Vec<FragmentOrigin>,
    /// What the frames' expansion ceiling was last shared from (see
    /// `share_frame_expansion`).
    shared: (usize, usize, usize),
    /// The origin of the event `next_stable` returned last.
    origin: FragmentOrigin,
    /// Each entered entity's replacement text, scanned once (by `EntityId`).
    pieces: Vec<Option<Box<[Piece]>>>,
    /// The next piece of each entity being expanded, innermost last.
    replay: Vec<usize>,
}

/// A token or reference of replacement text, from the previous piece's end.
#[derive(Clone, Copy, Debug)]
struct Piece {
    end: usize,
    kind: PieceKind,
}

#[derive(Clone, Copy, Debug)]
enum PieceKind {
    Token(TokenKind),
    /// A general reference, with the entity it names when that is internal.
    Reference(Option<EntityId>),
    /// The text fails to scan here (reported at the outermost reference).
    Fail(ErrorKind),
}

pub(crate) type ContentController = ContentControllerCore<'static>;
pub(crate) type BorrowedContentController<'input> = ContentControllerCore<'input>;

impl<'input> ContentControllerCore<'input> {
    /// Resume the same decoded source with its original transport encoding.
    pub(crate) fn new(
        body: DtdBodyState,
        parser: ParserState<'input>,
        tokenizer: Tokenizer,
    ) -> Result<Self, Error> {
        let position = (body.decoded_position, body.physical_position);
        let coordinator = TextCoordinator::new(tokenizer, body.encoding, position, body.limits);
        Self::with_source(body, parser, Source::Stream(Box::new(coordinator)))
    }
    pub(crate) fn stable_utf8(
        body: DtdBodyState,
        parser: ParserState<'input>,
        source: &'input str,
        decoded_token: usize,
    ) -> Result<Self, Error> {
        let limit = TokenLimit::new(body.limits.token, decoded_token);
        Self::with_source(body, parser, Source::Stable(source, limit))
    }
    fn with_source(
        body: DtdBodyState,
        parser: ParserState<'input>,
        source: Source<'input>,
    ) -> Result<Self, Error> {
        let limits = body.limits;
        // Two token slots and the reference scratch must be addressable.
        limits
            .scratch
            .checked_add(limits.scratch)
            .and_then(|tokens| tokens.checked_add(limits.token))
            .ok_or(Error::new(
                ErrorKind::LimitExceeded(Limit::TokenBytes),
                body.physical_position,
            ))?;
        Ok(Self {
            markers: !limits.flags.substitutes(),
            work: Work::new(limits.work, body.counters.work),
            body,
            parser,
            source,
            ready: None,
            scratch: String::new(),
            finalizing: false,
            pending: None,
            queued: None,
            open: Vec::new(),
            shared: (usize::MAX, 0, 0),
            pieces: Vec::new(),
            replay: Vec::new(),
            origin: FragmentOrigin::input(0..0).expect("empty range"),
        })
    }

    pub(crate) fn position(&self) -> (usize, usize) {
        self.body.resume_position()
    }
    pub(crate) fn owned_budget(&mut self) -> &mut Budget {
        self.parser.owned_budget()
    }
    pub(crate) fn errors(&self) -> &[Error] {
        self.parser.errors()
    }
    pub(crate) fn catalog(&self) -> &DtdCatalog {
        &self.body.catalog
    }
    pub(crate) fn catalog_mut(&mut self) -> &mut DtdCatalog {
        &mut self.body.catalog
    }

    /// The origin of the event `next_stable` returned last.
    pub(crate) fn origin(&self) -> SourceOrigin {
        SourceOrigin::from_repr(self.origin)
    }

    /// The next event of stable input, its replacement text included;
    /// `None` at the end of the document.
    pub(crate) fn next_stable(&mut self) -> Result<Option<Event<'input>>, Error> {
        loop {
            if let Some((marker, origin)) = self.pending.take() {
                self.pending = self.queued.take().map(|origin| (Marker::End, origin));
                self.origin = origin;
                return Ok(Some(match marker {
                    Marker::Start(name) => Event::EntityStart(Cow::Owned(name)),
                    Marker::End => Event::EntityEnd,
                }));
            }
            if let Some(&frame) = self.body.frames.top() {
                let Some((kind, range)) = self.replacement_step(frame)? else {
                    continue;
                };
                let event = self.take_replacement(kind, range);
                if !matches!(event, Ok(None)) {
                    return event;
                }
                continue;
            }
            let Source::Stable(input, limit) = self.source else {
                unreachable!("streamed input is lent, not taken")
            };
            let at = self.body.decoded_position;
            let scanned = scan(input, at, limit)?;
            let end = scanned.end().unwrap_or(at);
            (self.body.decoded_position, self.body.physical_position) = (end, end);
            self.work.charge(end - at, at)?;
            match scanned {
                Scanned::Token(kind, end) => {
                    let range = at..end;
                    let raw = match kind {
                        TokenKind::InvalidMarkup => {
                            crate::stream::valid_prefix(&input.as_bytes()[range.clone()])
                        }
                        _ => &input[range.clone()],
                    };
                    let span = TokenSpan {
                        decoded: range.clone(),
                        source: range.clone(),
                        encoding: SourceEncoding::Utf8,
                    };
                    let origin = FragmentOrigin::input(range).expect("token range");
                    let catalog_before = self.prepare_physical(kind, origin)?;
                    let event = publish_physical(
                        &mut self.parser,
                        &mut self.body.catalog,
                        (kind, raw, &span),
                        &mut |value, _, _| Ok(Cow::Borrowed(value)),
                    );
                    self.spend_catalog_work(catalog_before);
                    if !matches!(event, Ok(None)) {
                        self.origin = origin;
                        return event;
                    }
                }
                Scanned::Reference(end) => self.enter_general(at..end, None)?,
                Scanned::End => {
                    self.parser.validate_finish(at)?;
                    return Ok(None);
                }
            }
        }
    }

    /// Publishes a token of the innermost replacement text: in place when
    /// the input spells the text verbatim, otherwise owned.
    fn take_replacement(
        &mut self,
        kind: TokenKind,
        range: Range<usize>,
    ) -> Result<Option<Event<'input>>, Error> {
        let (origin, catalog_before) = self.prepare_replacement(kind, range.clone())?;
        let input = match &self.source {
            Source::Stable(input, _) => Some(*input),
            Source::Stream(_) => None,
        };
        let event = match self.replacement_input().zip(input) {
            Some((base, input)) => publish_replacement(
                &mut self.parser,
                &mut self.body.catalog,
                (
                    kind,
                    &input[base + range.start..base + range.end],
                    range.start,
                    origin,
                ),
                &mut |value, _, _| Ok(Cow::Borrowed(value)),
            ),
            None => {
                self.copy_replacement(range.clone());
                let copy =
                    &mut |value: &str, offset, budget: &mut Budget| budget.copy(value, offset);
                let (parser, catalog) = (&mut self.parser, &mut self.body.catalog);
                let event = (kind, self.scratch.as_str(), range.start, origin);
                publish_replacement(parser, catalog, event, copy).and_then(|event| {
                    let budget = self.parser.owned_budget();
                    event
                        .map(|event| owned_event(event, Some(budget), origin.error_offset()))
                        .transpose()
                })
            }
        };
        self.spend_catalog_work(catalog_before);
        self.origin = origin;
        event
    }

    /// Publishes the complete tokens at the front of a plain UTF-8 chunk of
    /// streamed input in place, with the expansion of their references, as
    /// the slice reader reads them; returns the bytes taken. Only streamed
    /// input idle at a token boundary qualifies, and the chunk stops before
    /// anything the incremental tokenizer must classify or diagnose. `skip`
    /// accounts each token's bytes as consumed transport before its event.
    pub(crate) fn drive_chunk(
        &mut self,
        input: &[u8],
        skip: &mut impl FnMut(usize),
        emit: &mut impl FnMut(&Event<'_>, SourceOrigin, &mut Budget) -> Result<(), Error>,
    ) -> Result<usize, Error> {
        let Source::Stream(coordinator) = &self.source else {
            return Ok(0);
        };
        let idle = self.pending.is_none()
            && self.ready.is_none()
            && self.body.frames.stack().is_empty()
            && !self.finalizing;
        let Some(base) = coordinator.idle_utf8().filter(|_| idle) else {
            return Ok(0);
        };
        let text = crate::stream::valid_prefix(input);
        let limit = TokenLimit::new(self.body.limits.token, self.parser.token_limit());
        let mut at = 0;
        while let Some(scanned) = scan(text, at, limit).ok().filter(|scanned| match scanned {
            // Text at the end of the chunk may continue; invalid markup
            // and DOCTYPEs are the tokenizer's to diagnose.
            Scanned::Token(TokenKind::Text, end) => *end < text.len(),
            Scanned::Token(TokenKind::Dtd | TokenKind::InvalidMarkup, _) | Scanned::End => false,
            _ => true,
        }) {
            let end = scanned.end().expect("a token or reference");
            let range = base + at..base + end;
            let Source::Stream(coordinator) = &mut self.source else {
                unreachable!("streamed input")
            };
            coordinator.resume_at(range.end);
            (self.body.decoded_position, self.body.physical_position) = (range.end, range.end);
            skip(end - at);
            self.work.charge(end - at, range.start)?;
            let origin = FragmentOrigin::input(range.clone()).expect("token range");
            match scanned {
                Scanned::Token(kind, _) => {
                    let raw = &text[at..end];
                    let span = TokenSpan {
                        decoded: range.clone(),
                        source: range.clone(),
                        encoding: SourceEncoding::Utf8,
                    };
                    let catalog_before = self.prepare_physical(kind, origin)?;
                    let event = publish_physical(
                        &mut self.parser,
                        &mut self.body.catalog,
                        (kind, raw, &span),
                        &mut |value, offset, budget| budget.copy(value, offset),
                    );
                    self.spend_catalog_work(catalog_before);
                    if let Some(event) = event? {
                        emit(
                            &event,
                            SourceOrigin::from_repr(origin),
                            self.parser.owned_budget(),
                        )?;
                    }
                }
                _ => {
                    self.scratch.clear();
                    self.scratch.push_str(&text[at..end]);
                    self.enter(origin, None)?;
                    self.drain(emit)?;
                }
            }
            at = end;
        }
        Ok(at)
    }

    /// Publishes the expansion of entered references.
    fn drain(
        &mut self,
        emit: &mut impl FnMut(&Event<'_>, SourceOrigin, &mut Budget) -> Result<(), Error>,
    ) -> Result<(), Error> {
        while self.pending.is_some() || !self.body.frames.stack().is_empty() {
            if self.drive(&[])?.outcome != ContentOutcome::EventReady {
                continue;
            }
            let lent = self.lend_with_budget()?;
            if let Some(event) = &lent.event {
                emit(event, SourceOrigin::from_repr(lent.origin), lent.budget)?;
            }
            self.acknowledge();
        }
        Ok(())
    }

    /// Scans streamed input (none after `finish_step`).
    pub(crate) fn advance(&mut self, input: &[u8]) -> Result<ContentStep, Error> {
        self.drive(input)
    }
    pub(crate) fn finish_step(&mut self) -> Result<ContentStep, Error> {
        self.finalizing = true;
        self.drive(&[])
    }

    pub(crate) fn is_non_event(&self) -> bool {
        let token = match (self.ready.as_ref(), &self.source) {
            (Some(Ready::Stream), Source::Stream(coordinator)) => {
                coordinator.current().map(|(kind, raw, _)| (kind, raw))
            }
            (Some(Ready::Replacement(kind, range)), _) => {
                let frame = self.body.frames.top().expect("active replacement");
                let text = self.body.catalog.replacement(frame.entity);
                Some((*kind, &text.expect("replacement").as_bytes()[range.clone()]))
            }
            _ => None,
        };
        self.pending.is_none()
            && token.is_some_and(|(kind, raw)| self.parser.is_non_event(kind, raw))
    }

    /// Releases the lent token's source storage.
    pub(crate) fn acknowledge(&mut self) {
        match self.pending.take() {
            Some(_) => self.pending = self.queued.take().map(|origin| (Marker::End, origin)),
            None => {
                if let (Some(Ready::Stream), Source::Stream(coordinator)) =
                    (self.ready.take(), &mut self.source)
                {
                    coordinator.release_token();
                }
            }
        }
    }

    /// Processes and releases a ready token that publishes no event.
    pub(crate) fn skip_non_event(&mut self) -> Result<(), Error> {
        self.lend_with_budget()?;
        self.acknowledge();
        Ok(())
    }

    fn drive(&mut self, input: &[u8]) -> Result<ContentStep, Error> {
        let step = |consumed, outcome| Ok(ContentStep { consumed, outcome });
        if self.pending.is_some() || self.ready.is_some() {
            return step(0, ContentOutcome::EventReady);
        }
        if let Some(&frame) = self.body.frames.top() {
            return match self.replacement_step(frame)? {
                Some((kind, range)) => {
                    self.ready = Some(Ready::Replacement(kind, range));
                    step(0, ContentOutcome::EventReady)
                }
                None => step(0, ContentOutcome::Progress),
            };
        }
        let Source::Stream(coordinator) = &mut self.source else {
            unreachable!("stable input is taken, not lent")
        };
        let result = match self.finalizing {
            true => coordinator.finish_step(&mut self.work),
            false => coordinator.advance(input, &mut self.work),
        };
        (self.body.decoded_position, self.body.physical_position) = coordinator.input_position();
        let text = result?;
        match text.outcome {
            TextOutcome::TokenReady => {
                self.ready = Some(Ready::Stream);
                step(text.consumed, ContentOutcome::EventReady)
            }
            TextOutcome::GeneralReferenceReady => {
                let (reference, extent) = coordinator.general().expect("reference ready");
                let origin = FragmentOrigin::input(extent.0.source.clone()).expect("reference");
                self.scratch.clear();
                self.scratch.push_str(reference.raw());
                coordinator.release_general();
                self.enter(origin, None)?;
                step(text.consumed, ContentOutcome::Progress)
            }
            TextOutcome::NeedInput => step(text.consumed, ContentOutcome::NeedPhysicalInput),
            TextOutcome::Complete => {
                self.parser.validate_finish(self.body.physical_position)?;
                step(text.consumed, ContentOutcome::Complete)
            }
        }
    }

    /// Scans the innermost replacement text at its cursor: a token's kind and
    /// range, or `None` after entering a nested reference or leaving the
    /// entity. Its failures are reported at the outermost reference.
    fn replacement_step(
        &mut self,
        frame: EntityFrame,
    ) -> Result<Option<(TokenKind, Range<usize>)>, Error> {
        let at = frame.cursor;
        let next = self.replay.last_mut().expect("replayed entity");
        let pieces = self.pieces[frame.entity.0]
            .as_deref()
            .expect("scanned entity");
        let Some(&Piece { end, kind }) = pieces.get(*next) else {
            return self.finish_general().map(|()| None);
        };
        *next += 1;
        let reference = frame.physical_reference.start;
        self.work.charge(end - at, reference)?;
        match kind {
            PieceKind::Token(kind) => Ok(Some((kind, at..end))),
            PieceKind::Reference(known) => self.enter_general(at..end, known).map(|()| None),
            PieceKind::Fail(kind) => Err(Error::new(kind, reference)),
        }
    }

    /// Scans `id`'s replacement text once, resolving the internal entities
    /// its references name; a failure ends the pieces. They are retained
    /// with the declarations, within the same storage ceiling.
    fn scan_pieces(&mut self, id: EntityId) -> Result<(), Error> {
        let offset = self
            .body
            .frames
            .top()
            .map_or(0, |frame| frame.physical_reference.start);
        if self.pieces.len() <= id.0 {
            self.pieces
                .try_reserve(id.0 + 1 - self.pieces.len())
                .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
            self.pieces.resize(id.0 + 1, None);
        }
        if self.pieces[id.0].is_some() {
            return Ok(());
        }
        let catalog = &self.body.catalog;
        let text = catalog.replacement(id).expect("internal entity");
        let limit = TokenLimit::new(self.body.limits.token, self.body.limits.token);
        let (mut pieces, mut at) = (Vec::new(), 0);
        loop {
            let piece = match scan(text, at, limit) {
                Ok(Scanned::End) => break,
                Ok(Scanned::Token(TokenKind::Dtd | TokenKind::InvalidMarkup, end)) => Piece {
                    end,
                    kind: PieceKind::Fail(ErrorKind::Syntax),
                },
                Ok(Scanned::Token(kind, end)) => Piece {
                    end,
                    kind: PieceKind::Token(kind),
                },
                Ok(Scanned::Reference(end)) => {
                    let known = match catalog.lookup_general(&text[at + 1..end - 1]) {
                        EntityLookup::Internal { id, .. } => Some(id),
                        _ => None,
                    };
                    Piece {
                        end,
                        kind: PieceKind::Reference(known),
                    }
                }
                Err(error) => Piece {
                    end: at,
                    kind: PieceKind::Fail(error.kind),
                },
            };
            pieces
                .try_reserve(1)
                .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
            pieces.push(piece);
            match piece.kind {
                PieceKind::Fail(_) => break,
                _ => at = piece.end,
            }
        }
        let bytes = pieces.len().saturating_mul(size_of::<Piece>());
        let stored = self.body.catalog.store_bytes(bytes);
        stored.map_err(|error| Error::new(error.kind, offset))?;
        self.pieces[id.0] = Some(pieces.into_boxed_slice());
        Ok(())
    }

    /// Enters the reference spelled at `range` of stable input or of the
    /// innermost replacement text, to the internal entity `known` when it
    /// was resolved before.
    fn enter_general(&mut self, range: Range<usize>, known: Option<EntityId>) -> Result<(), Error> {
        let (text, origin) = match self.body.frames.top() {
            Some(frame) => (
                self.body.catalog.replacement(frame.entity).expect("entity"),
                FragmentOrigin::Replacement {
                    declaration: frame.entity,
                    invocation: frame.invocation,
                    local: ByteSpan::new(range.clone()).expect("reference range"),
                    physical_reference: frame.physical_reference,
                },
            ),
            None => match self.source {
                Source::Stable(input, _) => (
                    input,
                    FragmentOrigin::input(range.clone()).expect("ordered"),
                ),
                Source::Stream(_) => unreachable!("streamed references come from the coordinator"),
            },
        };
        self.scratch.clear();
        self.scratch.push_str(&text[range]);
        self.enter(origin, known)
    }

    /// Enters the reference in `scratch`, spelled at `origin`, to the
    /// internal entity `known` when it was resolved before.
    fn enter(&mut self, origin: FragmentOrigin, known: Option<EntityId>) -> Result<(), Error> {
        self.share_expansion();
        let fail = |kind| Error::new(kind, origin.error_offset());
        if !self.parser.in_content() {
            return Err(fail(ErrorKind::TextOutsideRoot));
        }
        let name = &self.scratch[1..self.scratch.len() - 1];
        let marker = match self.markers {
            true => Some(owned_name(
                name,
                self.parser.owned_budget(),
                origin.error_offset(),
            )?),
            false => None,
        };
        let catalog = &mut self.body.catalog;
        let lookup = match known {
            Some(id) => Ok(catalog.entity_lookup(id)),
            None => catalog.resolve_general(name),
        };
        let resolved = match lookup.map_err(|error| fail(error.kind))? {
            EntityLookup::Internal { id, replacement } => Some((id, replacement.len())),
            // libxml2 drops a reference that unread markup could declare,
            // and one to an external entity that it cannot load.
            EntityLookup::UnreadEntity => {
                let offset = origin.input_range().map(|range| range.start);
                let message = format!("Entity '{name}' not defined");
                catalog.note(Code::UndeclaredEntity, offset, message);
                None
            }
            EntityLookup::ExternalUnavailable { .. } => None,
            // Recovery keeps a reference it cannot expand, empty, as libxml2 does.
            EntityLookup::Unparsed => {
                self.parser
                    .owned_budget()
                    .recover(fail(ErrorKind::UnparsedEntity))?;
                None
            }
            EntityLookup::Undeclared => {
                self.parser
                    .owned_budget()
                    .recover(fail(ErrorKind::UnknownEntity))?;
                None
            }
        };
        let Some((id, len)) = resolved else {
            // The skipped reference is consumed from its replacement frame too.
            if let Some(local) = origin.replacement_range() {
                self.body.frames.advance(local.len())?;
            }
            // libxml2 still keeps a reference node, with no content.
            if let Some(name) = marker {
                self.pending = Some((Marker::Start(name), origin));
                self.queued = Some(origin);
            }
            return Ok(());
        };
        let before = self.body.frames.counters().work;
        self.body
            .frames
            .cap_work(before.saturating_add(self.work.remaining()));
        let reference = origin
            .replacement_range()
            .unwrap_or_else(|| origin.physical_context());
        let depth = Some(self.parser.element_depth());
        let entered = self.body.frames.enter(id, len, reference, depth);
        self.sync_frame_work(before);
        entered?;
        self.scan_pieces(id)?;
        self.replay
            .try_reserve(1)
            .map_err(|_| Error::new(ErrorKind::Allocation, origin.error_offset()))?;
        self.replay.push(0);
        if let Some(name) = marker {
            self.open
                .try_reserve(1)
                .map_err(|_| Error::new(ErrorKind::Allocation, origin.error_offset()))?;
            self.open.push(origin);
            self.pending = Some((Marker::Start(name), origin));
        }
        Ok(())
    }

    /// Leaves the innermost entity, whose replacement text was read whole;
    /// its parent resumes after the reference.
    fn finish_general(&mut self) -> Result<(), Error> {
        self.body.frames.finish(self.parser.element_depth())?;
        self.replay.pop();
        if let Some(origin) = self.open.pop() {
            self.pending = Some((Marker::End, origin));
        }
        Ok(())
    }

    /// Under `RECOVER`, the next event that closes, at `at`, what a parse
    /// stopped by an unrepaired error left open: the innermost entity's
    /// elements, then its reference, out to the document's elements; `None`
    /// once nothing is open.
    pub(crate) fn close_open(&mut self, at: usize) -> Result<Option<Event<'input>>, Error> {
        self.origin = FragmentOrigin::input(at..at).expect("empty range");
        while let Some(frame) = self.body.frames.top() {
            if frame.element_depth < Some(self.parser.element_depth()) {
                break;
            }
            self.body.frames.leave();
            if self.open.pop().is_some() {
                return Ok(Some(Event::EntityEnd));
            }
        }
        self.parser.close_open(at)
    }

    fn share_expansion(&mut self) {
        let body = &mut self.body;
        share_expansion(
            &body.limits,
            body.physical_position,
            &mut body.frames,
            &mut body.catalog,
        );
        self.shared = (usize::MAX, 0, 0);
    }

    /// Shares the allowance before the frames expand replacement text. Their
    /// ceiling (the allowance less the fixed costs and the catalog's
    /// amplification) depends only on the input consumed, the references
    /// entered and the catalog's amplification, so it is kept while those
    /// are; tags, which spend the catalog's, always share.
    fn share_frame_expansion(&mut self) {
        let body = &self.body;
        let consumed = body
            .physical_position
            .saturating_add(body.catalog.external_bytes());
        let attempts = body.frames.counters().reference_attempts;
        let key = (consumed, attempts, body.catalog.usage().amplification);
        if key != self.shared {
            self.share_expansion();
            self.shared = key;
        }
    }

    /// Charges a physical token before publication; returns the catalog
    /// work spent before it.
    fn prepare_physical(
        &mut self,
        kind: TokenKind,
        origin: FragmentOrigin,
    ) -> Result<usize, Error> {
        // Of physical tokens only tags (attribute values and defaults) spend
        // amplification.
        if kind == TokenKind::Start {
            self.share_expansion();
        }
        let length = origin.physical_context().len();
        self.work.charge(length, origin.error_offset())?;
        Ok(self.cap_catalog_work())
    }

    /// Charges a replacement token at `range` of the innermost entity as
    /// its expansion; returns its origin and the catalog work spent before.
    fn prepare_replacement(
        &mut self,
        kind: TokenKind,
        range: Range<usize>,
    ) -> Result<(FragmentOrigin, usize), Error> {
        let frame = self.body.frames.top().expect("active replacement");
        let origin = FragmentOrigin::Replacement {
            declaration: frame.entity,
            invocation: frame.invocation,
            local: ByteSpan::new(range.clone()).expect("local token"),
            physical_reference: frame.physical_reference,
        };
        match kind {
            TokenKind::Start => self.share_expansion(),
            _ => self.share_frame_expansion(),
        }
        self.work.charge(range.len(), origin.error_offset())?;
        let before = self.body.frames.counters().work;
        self.body
            .frames
            .cap_work(before.saturating_add(self.work.remaining()));
        let result = self.body.frames.advance(range.len());
        self.sync_frame_work(before);
        result?;
        if kind == TokenKind::End {
            self.body
                .frames
                .check_before_end_element(self.parser.element_depth())?;
        }
        Ok((origin, self.cap_catalog_work()))
    }

    /// Caps the catalog's work at the remaining work; returns what it spent.
    fn cap_catalog_work(&mut self) -> usize {
        let catalog_before = self.body.catalog.usage().work;
        self.body
            .catalog
            .cap_work(catalog_before.saturating_add(self.work.remaining()));
        catalog_before
    }

    /// Charges the catalog work spent since `before`, which its cap kept
    /// within the remaining work.
    fn spend_catalog_work(&mut self, before: usize) {
        self.work.spend(self.body.catalog.usage().work - before);
    }

    /// Where the innermost entity's replacement text appears in the input.
    fn replacement_input(&self) -> Option<usize> {
        let frame = self.body.frames.top()?;
        self.body.catalog.replacement_input(frame.entity)
    }

    /// Copies `range` of the innermost replacement text into `scratch`, out
    /// of the catalog that publication updates.
    fn copy_replacement(&mut self, range: Range<usize>) {
        let frame = self.body.frames.top().expect("active replacement");
        let text = self.body.catalog.replacement(frame.entity).expect("entity");
        self.scratch.clear();
        self.scratch.push_str(&text[range]);
    }

    /// Publishes the pending entity boundary.
    fn lend_marker(&mut self) -> Option<LentContent<'_>> {
        let (marker, origin) = self.pending.as_ref()?;
        let event = match marker {
            Marker::Start(name) => Event::EntityStart(Cow::Borrowed(name)),
            Marker::End => Event::EntityEnd,
        };
        Some(LentContent {
            event: Some(event),
            origin: *origin,
            budget: self.parser.owned_budget(),
        })
    }

    /// Publishes the ready token; its event borrows source storage until
    /// `acknowledge`.
    pub(crate) fn lend_with_budget(&mut self) -> Result<LentContent<'_>, Error> {
        if self.pending.is_some() {
            return Ok(self.lend_marker().expect("pending marker"));
        }
        if let Some(Ready::Replacement(kind, range)) = self.ready.clone() {
            let (origin, catalog_before) = self.prepare_replacement(kind, range.clone())?;
            self.copy_replacement(range.clone());
            let event = publish_replacement(
                &mut self.parser,
                &mut self.body.catalog,
                (kind, &self.scratch, range.start, origin),
                &mut |value, offset, budget| budget.copy(value, offset),
            );
            self.work
                .spend(self.body.catalog.usage().work - catalog_before);
            return Ok(LentContent {
                event: event?,
                origin,
                budget: self.parser.owned_budget(),
            });
        }
        let (event, origin, catalog_before) = match (self.ready.clone(), &mut self.source) {
            (Some(Ready::Stream), Source::Stream(coordinator)) => {
                let (kind, bytes, span) = coordinator.current().expect("ready token");
                let origin = FragmentOrigin::input(span.source.clone()).expect("token range");
                let raw = match kind {
                    TokenKind::InvalidMarkup => crate::stream::valid_prefix(bytes),
                    _ => std::str::from_utf8(bytes)
                        .map_err(|_| Error::new(ErrorKind::InvalidUtf8, origin.error_offset()))?,
                };
                if kind == TokenKind::Start {
                    let body = &mut self.body;
                    share_expansion(
                        &body.limits,
                        body.physical_position,
                        &mut body.frames,
                        &mut body.catalog,
                    );
                }
                self.work.charge(raw.len(), origin.error_offset())?;
                let catalog_before = self.body.catalog.usage().work;
                self.body
                    .catalog
                    .cap_work(catalog_before.saturating_add(self.work.remaining()));
                let event = publish_physical(
                    &mut self.parser,
                    &mut self.body.catalog,
                    (kind, raw, span),
                    &mut |value, offset, budget| budget.copy(value, offset),
                );
                (event, origin, catalog_before)
            }
            _ => unreachable!("a ready token belongs to its source"),
        };
        // Capped at the remaining work by `prepare_publication`.
        self.work
            .spend(self.body.catalog.usage().work - catalog_before);
        Ok(LentContent {
            event: event?,
            origin,
            budget: self.parser.owned_budget(),
        })
    }

    /// Charges frame work spent since `before`, which the frames' cap keeps
    /// within the remaining work.
    fn sync_frame_work(&mut self, before: usize) {
        let spent = self.body.frames.counters().work - before;
        self.work.spend(spent);
    }
}

/// What [`scan`] finds: a token or a general reference ending at the offset,
/// or the end of the text.
#[derive(Clone, Copy, Debug)]
enum Scanned {
    Token(TokenKind, usize),
    Reference(usize),
    End,
}

impl Scanned {
    fn end(self) -> Option<usize> {
        match self {
            Self::Token(_, end) | Self::Reference(end) => Some(end),
            Self::End => None,
        }
    }
}

/// The token at `at` of complete UTF-8 `text`, as the incremental tokenizer
/// reads it with general references ending text: character and predefined
/// references stay in their text. A token over `limit` fails at its start,
/// as does one the end of the text cuts off.
fn scan(text: &str, at: usize, token: TokenLimit) -> Result<Scanned, Error> {
    let bytes = text.as_bytes();
    let tail = &bytes[at..];
    let (limit, overflow) = (token.bytes, token.error(at));
    match tail.first() {
        None => return Ok(Scanned::End),
        // Invalid markup recognized on a leading byte ended inside a scalar.
        Some(_) if !text.is_char_boundary(at) => {
            return Err(Error::new(ErrorKind::InvalidUtf8, at));
        }
        Some(b'<') => {}
        Some(_) => {
            let mut end = 0;
            while let Some(found) = memchr2(b'<', b'&', &tail[end..]) {
                end += found;
                if end > limit {
                    return Err(overflow);
                }
                if tail[end] == b'<' {
                    return Ok(Scanned::Token(TokenKind::Text, at + end));
                }
                let (kind, after) = reference(text, at + end, token)?;
                if kind == ReferenceKind::General {
                    return Ok(match end {
                        0 => Scanned::Reference(after),
                        _ => Scanned::Token(TokenKind::Text, at + end),
                    });
                }
                end = after - at;
            }
            return match tail.len() > limit {
                true => Err(overflow),
                false => Ok(Scanned::Token(TokenKind::Text, text.len())),
            };
        }
    }
    let after_lt = || markup_after_lt(&tail[1..]).map(|(kind, length)| (kind, length + 1));
    let markup = match (tail.get(1), tail.get(2)) {
        // A `<!` prefix is classified through the byte that leaves its family.
        (Some(b'!'), Some(third)) => {
            let family: &[u8] = match third {
                b'-' => b"<!--",
                b'[' => b"<![CDATA[",
                b'D' => b"<!DOCTYPE",
                _ => b"",
            };
            match tail
                .iter()
                .zip(family)
                .position(|(byte, expected)| byte != expected)
            {
                _ if family.is_empty() => Some((TokenKind::InvalidMarkup, 3)),
                Some(mismatch) => Some((TokenKind::InvalidMarkup, mismatch + 1)),
                // A prefix cut off by the end of the text is invalid markup.
                None if tail.len() < family.len() => Some((TokenKind::InvalidMarkup, tail.len())),
                None if family == b"<!DOCTYPE" => Some((TokenKind::Dtd, 9)),
                None => after_lt(),
            }
        }
        (Some(b'!'), None) => Some((TokenKind::InvalidMarkup, 2)),
        (Some(_), _) => after_lt(),
        (None, _) => None,
    };
    match markup {
        Some((kind, length)) if length <= limit => Ok(Scanned::Token(kind, at + length)),
        Some(_) => Err(overflow),
        None if tail.len() > limit => Err(overflow),
        None => Err(Error::new(ErrorKind::UnexpectedEof, at)),
    }
}

/// The kind and end of the reference spelled at `start` of `text`, through
/// its `;`, as the reference scratch collects it.
fn reference(text: &str, start: usize, token: TokenLimit) -> Result<(ReferenceKind, usize), Error> {
    let bytes = text.as_bytes();
    let window = start.saturating_add(token.bytes).saturating_add(1);
    let window = &bytes[start..bytes.len().min(window)];
    match memchr(b';', window) {
        Some(at) => {
            let end = start + at + 1;
            Ok((classify(&text[start..end], start, true)?, end))
        }
        None if window.len() > token.bytes => Err(token.error(start)),
        None => {
            classify(&text[start..], start, false)?;
            Err(Error::new(ErrorKind::UnexpectedEof, text.len()))
        }
    }
}

/// Publishes a physical token: tags see the DTD, other tokens are ordinary.
fn publish_physical<'source, 'raw>(
    parser: &mut ParserState<'source>,
    catalog: &mut DtdCatalog,
    (kind, raw, span): (TokenKind, &'raw str, &TokenSpan),
    retain: &mut impl FnMut(&'raw str, usize, &mut Budget) -> Result<Cow<'source, str>, Error>,
) -> Result<Option<Event<'raw>>, Error>
where
    'source: 'raw,
{
    // Recovered errors count source bytes, as returned ones do.
    let recorded = (span.encoding != SourceEncoding::Utf8).then(|| parser.errors().len());
    let result = match kind {
        TokenKind::Start if !catalog.plain_tag(raw) => {
            let declarations = (&mut *catalog, LiteralSource::Physical);
            parser.process_dtd_start(raw, span.decoded.start, declarations, retain)
        }
        _ => parser.process_token(kind, raw, span.decoded.clone(), retain),
    };
    note_catalogs(parser, catalog, kind);
    note_content(parser, catalog, &result);
    if let Some(recorded) = recorded {
        (parser.owned_budget()).remap(recorded, |error| span.map_error(raw, error));
    }
    result.map_err(|error| span.map_error(raw, error))
}

/// Under `NOBLANKS`, records a start tag's declared content for the blank
/// filters (`Budget::content`).
fn note_content(
    parser: &mut ParserState<'_>,
    catalog: &mut DtdCatalog,
    event: &Result<Option<Event<'_>>, Error>,
) {
    if let Ok(Some(Event::Start(element) | Event::Empty(element))) = event
        && parser.drops_blanks()
    {
        let declared = catalog.element_decl(element.name.qualified());
        let content = declared.and_then(|index| catalog.dtd().elements[index].element_content());
        parser.owned_budget().set_content(content);
    }
}

/// Hands the catalogs of `oasis-xml-catalog` instructions to the resolver.
fn note_catalogs(parser: &mut ParserState<'_>, catalog: &mut DtdCatalog, kind: TokenKind) {
    if kind == TokenKind::Instruction {
        (parser.take_catalogs().iter()).for_each(|url| catalog.catalog_pi(url));
    }
}

/// Publishes a replacement-text token from `raw`: as physical input, but
/// with line ends normalized when it was declared, stopping at the markup
/// errors physical input recovers from, and reading a tag's literals as
/// replacement text. Failures are located at the reference.
pub(crate) fn publish_replacement<'source, 'raw>(
    parser: &mut ParserState<'source>,
    catalog: &mut DtdCatalog,
    (kind, raw, offset, origin): (TokenKind, &'raw str, usize, FragmentOrigin),
    retain: &mut impl FnMut(&'raw str, usize, &mut Budget) -> Result<Cow<'source, str>, Error>,
) -> Result<Option<Event<'raw>>, Error>
where
    'source: 'raw,
{
    parser.set_replacement(true);
    let recorded = parser.errors().len();
    let result = parser
        .check_token_limit(raw.len(), offset)
        .and_then(|()| match kind {
            TokenKind::Start if !catalog.plain_tag(raw) => {
                let declarations = (&mut *catalog, LiteralSource::Replacement);
                parser.process_dtd_start(raw, offset, declarations, retain)
            }
            _ => parser.process_token(kind, raw, offset..offset + raw.len(), retain),
        });
    parser.set_replacement(false);
    note_catalogs(parser, catalog, kind);
    note_content(parser, catalog, &result);
    let at = |error: Error| Error::new(error.kind, origin.error_offset());
    (parser.owned_budget()).remap(recorded, at);
    result.map_err(at)
}

/// A budgeted copy of an entity name for its `EntityStart` marker.
fn owned_name(name: &str, budget: &mut Budget, offset: usize) -> Result<String, Error> {
    budget.charge_owned(name.len(), offset)?;
    let mut copy = String::new();
    copy.try_reserve_exact(name.len())
        .map_err(|_| Error::new(ErrorKind::Allocation, offset))?;
    copy.push_str(name);
    Ok(copy)
}
