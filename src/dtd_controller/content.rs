//! General-entity content composition; physical events lend scratch, generated events own.
use std::borrow::Cow;

use super::*;
use crate::dtd_frames::ByteSpan;
use crate::parser::{DtdLeafKind, ParserState};
use crate::tokenizer::local::LocalTokenizer;
use crate::tokenizer::text::{
    TextCoordinator, TextError, TextLocation, TextOutcome, TextShared, TextSourceId,
    TextSourceLease,
};
use crate::tokenizer::{TokenKind, Tokenizer};
use crate::valid::Code;
use crate::{DtdError, Event, SourceOrigin};

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

pub(crate) struct ContentControllerCore<'input> {
    body: DtdBodyState,
    parser: ParserState<'input>,
    coordinator: TextCoordinator<'input>,
    paused_input: Option<TextSourceLease<'input>>,
    /// The replacement-text source, parked while physical input is read.
    local: Option<TextSourceLease<'input>>,
    finalizing: bool,
    complete: bool,
    /// Report entity references as `EntityStart`/`EntityEnd` (no `NOENT`).
    markers: bool,
    /// A boundary to publish before any further token.
    pending: Option<(Marker, FragmentOrigin)>,
    /// The `End` of a pending `Start` whose reference expands to nothing.
    queued: Option<FragmentOrigin>,
    /// Origins of the references being expanded, for their `End`s.
    open: Vec<FragmentOrigin>,
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
        let input = TextSourceLease::resume(
            tokenizer,
            body.encoding,
            body.decoded_position,
            body.physical_position,
        );
        Self::with_source(body, parser, input)
    }
    pub(crate) fn stable_utf8(
        body: DtdBodyState,
        parser: ParserState<'input>,
        tokenizer: Tokenizer,
        source: &'input str,
    ) -> Result<Self, Error> {
        let input = TextSourceLease::resume_stable_utf8(tokenizer, source, body.physical_position);
        Self::with_source(body, parser, input)
    }
    fn with_source(
        body: DtdBodyState,
        parser: ParserState<'input>,
        input: TextSourceLease<'input>,
    ) -> Result<Self, Error> {
        let markers = !body.limits.flags.substitutes();
        let fail = |kind| Error::new(kind, body.physical_position);
        let shared = TextShared::new(body.limits, body.counters.work).map_err(fail)?;
        let coordinator = TextCoordinator::new(shared, input);
        Ok(Self {
            body,
            parser,
            coordinator,
            paused_input: None,
            local: None,
            finalizing: false,
            complete: false,
            markers,
            pending: None,
            queued: None,
            open: Vec::new(),
        })
    }

    pub(crate) fn position(&self) -> (usize, usize) {
        self.body.resume_position()
    }
    pub(crate) fn owned_budget(&mut self) -> &mut Budget {
        self.parser.owned_budget()
    }
    pub(crate) fn catalog(&self) -> &DtdCatalog {
        &self.body.catalog
    }

    pub(crate) fn advance(&mut self, input: &[u8]) -> Result<ContentStep, Error> {
        if self.finalizing && !input.is_empty() {
            return Err(self.simple(ErrorKind::InvalidState));
        }
        self.drive(input)
    }

    pub(crate) fn advance_borrowed(&mut self) -> Result<ContentStep, Error> {
        self.drive(&[])
    }
    fn stable_input(&self) -> bool {
        self.paused_input
            .as_ref()
            .is_some_and(TextSourceLease::is_stable_input)
            || self.coordinator.is_stable_input()
    }
    pub(crate) fn finish_step(&mut self) -> Result<ContentStep, Error> {
        self.finalizing = true;
        self.drive(&[])
    }

    pub(crate) fn is_non_event(&self) -> bool {
        self.pending.is_none()
            && self
                .coordinator
                .current()
                .is_some_and(|token| self.parser.is_non_event(token.kind, token.raw))
    }

    /// Releases the lent token's source storage.
    pub(crate) fn acknowledge(&mut self) {
        match self.pending.take() {
            Some(_) => self.pending = self.queued.take().map(|origin| (Marker::End, origin)),
            None => self.coordinator.release_token(),
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
        if self.pending.is_some() {
            return step(0, ContentOutcome::EventReady);
        }
        if self.coordinator.current().is_some() {
            return step(0, ContentOutcome::EventReady);
        }
        if self.complete {
            return step(0, ContentOutcome::Complete);
        }
        let stable = self.stable_input();
        let mut consumed = 0;
        loop {
            if self.coordinator.general().is_some() {
                self.enter_general()?;
                return step(consumed, ContentOutcome::Progress);
            }
            let result = if let Some(frame) = self.body.frames.top() {
                let replacement = self
                    .body
                    .catalog
                    .replacement(frame.entity)
                    .expect("active declaration");
                match frame.scanned == replacement.len() {
                    true => self.coordinator.finish_step(),
                    false => self
                        .coordinator
                        .advance(&replacement.as_bytes()[frame.scanned..]),
                }
            } else {
                let available = match stable {
                    true => self.coordinator.stable_remaining().expect("stable input"),
                    false => &input[consumed..],
                };
                if stable && available.is_empty() {
                    self.finalizing = true;
                }
                match self.finalizing {
                    true => self.coordinator.finish_step(),
                    false => self.coordinator.advance(available),
                }
            };
            // Include accepted work even when scanning fails; never replay it.
            self.sync_usage();
            if self.body.frames.top().is_none() {
                let (decoded, physical) = self.coordinator.input_position().expect("input lease");
                self.body.decoded_position = decoded;
                self.body.physical_position = physical;
            }
            let text = result.map_err(|error| self.text_error(error))?;
            match self.body.frames.top_mut() {
                Some(frame) => frame.scanned += text.consumed,
                None => consumed += text.consumed,
            }
            let active = self.body.frames.top().is_some();
            match text.outcome {
                TextOutcome::TokenReady => return step(consumed, ContentOutcome::EventReady),
                TextOutcome::GeneralReferenceReady => {
                    self.enter_general()?;
                    return step(consumed, ContentOutcome::Progress);
                }
                TextOutcome::NeedInput if active || (stable && !self.finalizing) => {}
                TextOutcome::NeedInput if consumed < input.len() && !self.finalizing => {
                    // A transport quota can split one decoder scalar's UTF-8
                    // representation. Drain its zero-width continuations before
                    // reporting that the supplied decoded window was exhausted.
                    if text.consumed == 0 {
                        return Err(self.simple(ErrorKind::InvalidState));
                    }
                }
                TextOutcome::NeedInput => return step(consumed, ContentOutcome::NeedPhysicalInput),
                TextOutcome::Complete if active => {
                    self.finish_general()?;
                    return step(consumed, ContentOutcome::Progress);
                }
                TextOutcome::Complete => {
                    self.parser.validate_finish(self.body.physical_position)?;
                    self.complete = true;
                    return step(consumed, ContentOutcome::Complete);
                }
            }
        }
    }

    fn enter_general(&mut self) -> Result<(), Error> {
        self.share_expansion();
        let (reference, extent, source) = self.coordinator.general().expect("reference ready");
        let origin = match source {
            TextSourceId::Input => {
                FragmentOrigin::input(extent.0.source.clone()).expect("real reference")
            }
            TextSourceId::Replacement(invocation) => {
                let frame = self.body.frames.top().expect("active source");
                FragmentOrigin::Replacement {
                    declaration: frame.entity,
                    invocation,
                    local: ByteSpan::new(extent.0.decoded.clone()).expect("reference range"),
                    physical_reference: frame.physical_reference,
                }
            }
        };
        let fail = |kind| Error::new(kind, origin.error_offset());
        if !self.parser.in_content() {
            return Err(fail(ErrorKind::TextOutsideRoot));
        }
        let name = &reference.raw()[1..reference.raw().len() - 1];
        let marker = match self.markers {
            true => Some(owned_name(
                name,
                self.parser.owned_budget(),
                origin.error_offset(),
            )?),
            false => None,
        };
        let catalog = &mut self.body.catalog;
        catalog
            .load_general(name)
            .map_err(|error| fail(error.kind))?;
        let resolved = match catalog.lookup_general(name) {
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
            EntityLookup::Predefined => return Err(self.simple(ErrorKind::InvalidState)),
        };
        let Some((id, len)) = resolved else {
            // The skipped reference is consumed from its replacement frame too.
            if let Some(local) = origin.replacement_range() {
                self.body.frames.advance(local.len())?;
            }
            self.coordinator.discard_general();
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
            .cap_work(before.saturating_add(self.coordinator.remaining_work()));
        let reference = origin
            .replacement_range()
            .unwrap_or_else(|| origin.physical_context());
        let depth = Some(self.parser.element_depth());
        let entered = self.body.frames.enter(id, len, reference, depth);
        self.sync_frame_work(before, origin)?;
        let invocation = entered?;
        let was_physical = self.body.frames.stack().len() == 1;
        self.coordinator.release_general();
        if was_physical {
            let local = match self.local.take() {
                None => {
                    let limits = self.body.limits;
                    let tokenizer = LocalTokenizer::new(0, limits.token, limits.work);
                    TextSourceLease::local(tokenizer, invocation)
                }
                Some(mut lease) => {
                    lease.switch_local_at_boundary(invocation, 0);
                    lease
                }
            };
            let paused = std::mem::replace(self.coordinator.source_mut(), local);
            self.paused_input = Some(paused);
        } else {
            self.coordinator
                .source_mut()
                .switch_local_at_boundary(invocation, 0);
        }
        self.coordinator.resume();
        if let Some(name) = marker {
            self.open
                .try_reserve(1)
                .map_err(|_| Error::new(ErrorKind::Allocation, origin.error_offset()))?;
            self.open.push(origin);
            self.pending = Some((Marker::Start(name), origin));
        }
        Ok(())
    }

    fn finish_general(&mut self) -> Result<(), Error> {
        let depth = Some(self.parser.element_depth());
        self.body.frames.finish(depth)?;
        if let Some(origin) = self.open.pop() {
            self.pending = Some((Marker::End, origin));
        }
        match self.body.frames.top_mut() {
            Some(parent) => {
                parent.scanned = parent.cursor;
                let (invocation, start) = (parent.invocation, parent.cursor);
                self.coordinator
                    .source_mut()
                    .switch_local_at_boundary(invocation, start);
            }
            None => {
                let paused = self.paused_input.take().expect("paused primary input");
                let local = std::mem::replace(self.coordinator.source_mut(), paused);
                self.local = Some(local);
            }
        }
        self.coordinator.resume();
        Ok(())
    }

    fn share_expansion(&mut self) {
        let body = &mut self.body;
        share_expansion(
            &body.limits,
            body.physical_position,
            &mut body.frames,
            &mut body.catalog,
        );
    }

    /// Charges the ready token and its replacement-text progress, and caps the
    /// catalog's work; returns the token's kind, origin, offset and the
    /// catalog work spent before publication.
    fn prepare_publication(&mut self) -> Result<(TokenKind, FragmentOrigin, usize, usize), Error> {
        self.share_expansion();
        let token = self.coordinator.current().expect("ready token");
        let (kind, length) = (token.kind, token.raw.len());
        let (origin, offset) = match token.location {
            TextLocation::Input(span) => (
                FragmentOrigin::input(span.source.clone()).expect("token range"),
                span.decoded.start,
            ),
            TextLocation::Local { invocation, range } => {
                let frame = self.body.frames.top().expect("active replacement");
                (
                    FragmentOrigin::Replacement {
                        declaration: frame.entity,
                        invocation,
                        local: ByteSpan::new(range.clone()).expect("local token"),
                        physical_reference: frame.physical_reference,
                    },
                    range.start,
                )
            }
        };
        self.charge(length, origin, offset)?;
        if let FragmentOrigin::Replacement { .. } = origin {
            let before = self.body.frames.counters().work;
            self.body
                .frames
                .cap_work(before.saturating_add(self.coordinator.remaining_work()));
            let result = self.body.frames.advance(length);
            self.sync_frame_work(before, origin)?;
            result?;
            if kind == TokenKind::End {
                self.body
                    .frames
                    .check_before_end_element(self.parser.element_depth())?;
            }
        }
        let catalog_before = self.body.catalog.usage().work;
        self.body
            .catalog
            .cap_work(catalog_before.saturating_add(self.coordinator.remaining_work()));
        Ok((kind, origin, offset, catalog_before))
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
        let (kind, origin, offset, catalog_before) = self.prepare_publication()?;
        let (token, work) = self.coordinator.current_with_work().expect("ready token");
        let raw = match kind {
            TokenKind::InvalidMarkup => crate::stream::valid_prefix(token.raw),
            _ => std::str::from_utf8(token.raw)
                .map_err(|_| Error::new(ErrorKind::InvalidUtf8, origin.error_offset()))?,
        };
        let event = publish_raw(
            &mut self.parser,
            &mut self.body.catalog,
            RawPublication {
                kind,
                raw,
                offset,
                location: token.location,
                origin,
                stable_physical: false,
            },
            &mut |value, offset, budget| budget.copy(value, offset),
        );
        let delta = self.body.catalog.usage().work - catalog_before;
        let event = account_publication(event, work, delta, origin, offset);
        self.body.counters.work = work.work();
        Ok(LentContent {
            event: event?,
            origin,
            budget: self.parser.owned_budget(),
        })
    }

    /// Moves a stable-input event out; no field can borrow reusable controller storage.
    pub(crate) fn take_event(&mut self) -> Result<(Option<Event<'input>>, SourceOrigin), Error> {
        if let Some((marker, origin)) = self.pending.take() {
            self.pending = self.queued.take().map(|origin| (Marker::End, origin));
            let event = match marker {
                Marker::Start(name) => Event::EntityStart(Cow::Owned(name)),
                Marker::End => Event::EntityEnd,
            };
            return Ok((Some(event), SourceOrigin::from_repr(origin)));
        }
        let token = self.coordinator.current().expect("ready token");
        let stable = match token.location {
            TextLocation::Input(_) => Some(self.coordinator.stable_current().expect("stable")),
            TextLocation::Local { .. } => None,
        };
        let (kind, origin, offset, catalog_before) = self.prepare_publication()?;
        let event = match stable {
            Some((_, raw, span)) => publish_raw(
                &mut self.parser,
                &mut self.body.catalog,
                RawPublication {
                    kind,
                    raw,
                    offset,
                    location: TextLocation::Input(&span),
                    origin,
                    stable_physical: true,
                },
                &mut |value, _, _| Ok(Cow::Borrowed(value)),
            ),
            None => {
                let token = self.coordinator.current().expect("ready token");
                let TextLocation::Local { range, .. } = token.location else {
                    unreachable!("replacement token")
                };
                let raw = std::str::from_utf8(token.raw)
                    .map_err(|_| Error::new(ErrorKind::InvalidUtf8, origin.error_offset()))?;
                let parser = &mut self.parser;
                publish_generated(parser, &mut self.body.catalog, kind, raw, range, origin)
                    .map(Some)
            }
        };
        let delta = self.body.catalog.usage().work - catalog_before;
        let (_, work) = self.coordinator.current_with_work().expect("ready token");
        let event = account_publication(event, work, delta, origin, offset);
        self.sync_usage();
        let event = event?;
        self.acknowledge();
        Ok((event, SourceOrigin::from_repr(origin)))
    }

    fn charge(
        &mut self,
        amount: usize,
        origin: FragmentOrigin,
        offset: usize,
    ) -> Result<(), Error> {
        let (source, offset) = text_source(origin, offset);
        let result = self
            .coordinator
            .charge_external_work(amount, source, offset);
        self.sync_usage();
        result.map_err(|error| self.text_error(error))
    }
    fn sync_usage(&mut self) {
        self.body.counters.work = self.coordinator.work();
    }
    fn sync_frame_work(&mut self, before: usize, origin: FragmentOrigin) -> Result<(), Error> {
        self.charge(
            self.body.frames.counters().work - before,
            origin,
            origin
                .replacement_range()
                .map_or(origin.error_offset(), |range| range.start),
        )
    }
    /// A replacement-text failure is reported at its outermost reference.
    fn text_error(&self, error: TextError) -> Error {
        let offset = match error.source {
            TextSourceId::Input => error.offset,
            TextSourceId::Replacement(invocation) => self
                .body
                .frames
                .stack()
                .iter()
                .rev()
                .find(|frame| frame.invocation == invocation)
                .map_or(self.body.physical_position, |frame| {
                    frame.physical_reference.start
                }),
        };
        Error::new(error.kind, offset)
    }
    fn simple(&self, kind: ErrorKind) -> Error {
        Error::new(kind, self.body.physical_position)
    }
}

struct RawPublication<'raw, 'span> {
    kind: TokenKind,
    raw: &'raw str,
    offset: usize,
    location: TextLocation<'span>,
    origin: FragmentOrigin,
    stable_physical: bool,
}
fn publish_raw<'source, 'raw>(
    parser: &mut ParserState<'source>,
    catalog: &mut DtdCatalog,
    publication: RawPublication<'raw, '_>,
    retain: &mut impl FnMut(&'raw str, usize, &mut Budget) -> Result<Cow<'source, str>, Error>,
) -> Result<Option<Event<'raw>>, Error>
where
    'source: 'raw,
{
    let RawPublication {
        kind,
        raw,
        offset,
        location,
        origin,
        stable_physical,
    } = publication;
    match location {
        TextLocation::Input(span) if kind == TokenKind::Start => {
            let result = match stable_physical {
                true => parser.process_stable_dtd_start(raw, offset, catalog, retain),
                false => {
                    parser.process_dtd_start(raw, offset, catalog, LiteralSource::Physical, retain)
                }
            };
            result
                .map(Some)
                .map_err(|error| tag_failure(error, origin, Some((span, raw))))
        }
        TextLocation::Input(span) => parser
            .process_token(kind, raw, span.decoded.clone(), retain)
            .map_err(|error| span.map_error(raw, error)),
        TextLocation::Local { range, .. } => {
            publish_generated(parser, catalog, kind, raw, range, origin).map(Some)
        }
    }
}

/// Publishes a replacement-text token. Failures are located at the reference.
fn publish_generated(
    parser: &mut ParserState<'_>,
    catalog: &mut DtdCatalog,
    kind: TokenKind,
    raw: &str,
    range: std::ops::Range<usize>,
    origin: FragmentOrigin,
) -> Result<Event<'static>, Error> {
    let offset = range.start;
    let leaf = |leaf| parser.process_dtd_leaf_owned(leaf, raw, range);
    match kind {
        TokenKind::Start => parser
            .process_dtd_start_owned(raw, offset, catalog)
            .map_err(|error| tag_failure(error, origin, None)),
        TokenKind::End => parser.process_dtd_end_owned(raw, offset),
        TokenKind::Text => leaf(DtdLeafKind::Text),
        TokenKind::Cdata => leaf(DtdLeafKind::Cdata),
        TokenKind::Comment => leaf(DtdLeafKind::Comment),
        TokenKind::Instruction => leaf(DtdLeafKind::ProcessingInstruction),
        TokenKind::Dtd | TokenKind::InvalidMarkup => Err(Error::new(ErrorKind::Syntax, offset)),
    }
    .map_err(|error| Error::new(error.kind, origin.error_offset()))
}

/// Charges publication's catalog work; the event's own failure comes first.
fn account_publication<'event>(
    event: Result<Option<Event<'event>>, Error>,
    work: &mut TextShared,
    delta: usize,
    origin: FragmentOrigin,
    offset: usize,
) -> Result<Option<Event<'event>>, Error> {
    let (source, offset) = text_source(origin, offset);
    let charged = work.charge_external_work(delta, source, offset);
    let event = event?;
    charged.map_err(|error| Error::new(error.kind, origin.error_offset()))?;
    Ok(event)
}

/// The text source of `origin`, and the offset its ledger reports.
fn text_source(origin: FragmentOrigin, offset: usize) -> (TextSourceId, usize) {
    match origin {
        FragmentOrigin::Replacement { invocation, .. } => {
            (TextSourceId::Replacement(invocation), offset)
        }
        _ => (TextSourceId::Input, origin.error_offset()),
    }
}

/// A start-tag failure: its input offset when it lies in physical text,
/// otherwise the reference that produced the tag.
fn tag_failure(
    error: DtdError,
    origin: FragmentOrigin,
    physical: Option<(&TokenSpan, &str)>,
) -> Error {
    let offset = match (physical, error.offset) {
        (Some((span, raw)), Some(offset)) => {
            span.map_error(raw, Error::new(ErrorKind::InvalidState, offset))
                .offset
        }
        _ => origin.error_offset(),
    };
    Error::new(error.kind, offset)
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
