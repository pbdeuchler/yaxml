//! Internal-subset composition: scanned units, parameter entities and the catalog.

use crate::dtd::{DtdLimits, DtdUnit, DtdUnitView};
use crate::dtd_catalog::{DeclarationContext, DtdCatalog, EntityLookup, LiteralSource};
use crate::dtd_frames::{EntityFrames, FragmentOrigin};
use crate::dtd_scan::{DtdScanner, ScanKind, ScanMode};
use crate::encoding::{SourceEncoding, TokenSpan};
use crate::parser::Budget;
use crate::{Error, ErrorKind, Limit};

pub(crate) mod content;

/// libxml2's `XML_ENT_FIXED_COST`: charged per expansion so that empty or
/// tiny entities cannot be referenced without bound.
pub(crate) const ENTITY_FIXED_COST: usize = 20;

const WORK: ErrorKind = ErrorKind::LimitExceeded(Limit::EntityExpansion);

/// Splits the shared expansion allowance between the two expanding
/// components; returns what is left of it.
pub(crate) fn share_expansion(
    limits: &DtdLimits,
    consumed: usize,
    frames: &mut EntityFrames,
    catalog: &mut DtdCatalog,
) -> usize {
    let consumed = consumed.saturating_add(catalog.external_bytes());
    let counters = frames.counters();
    // Frames count entered replacement bytes; each reference adds libxml2's fixed cost.
    let frames_used = counters.expanded_bytes.saturating_add(
        counters
            .reference_attempts
            .saturating_mul(ENTITY_FIXED_COST),
    );
    let catalog_used = catalog.usage().amplification;
    let left = limits
        .expansion_allowance(consumed)
        .saturating_sub(frames_used)
        .saturating_sub(catalog_used);
    frames.set_expansion_ceiling(counters.expanded_bytes.saturating_add(left));
    catalog.set_amplification_ceiling(catalog_used.saturating_add(left));
    left
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ControllerCounters {
    pub(crate) work: usize,
    pub(crate) owned_bytes: usize,
    pub(crate) physical_bytes: usize,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum DtdEvent<'a> {
    Header,
    /// A declaration, comment or processing instruction, after the catalog applied it.
    Item(&'a DtdUnit<'a>),
    End,
}

/// The ready event; an item carries its unit for consumers that read
/// declarations.
enum Ready<'input> {
    Begin,
    Item(DtdUnit<'input>),
    End,
}
#[derive(Clone, Copy)]
enum Side {
    Physical,
    Replacement,
}

pub(crate) struct DtdBodyState {
    catalog: DtdCatalog,
    frames: EntityFrames,
    limits: DtdLimits,
    counters: ControllerCounters,
    decoded_position: usize,
    physical_position: usize,
    encoding: SourceEncoding,
}

pub(crate) struct DtdControllerCore<'input> {
    physical: DtdScanner,
    fragment: DtdScanner,
    catalog: DtdCatalog,
    frames: EntityFrames,
    /// Complete UTF-8 input whose declarations are parsed in place, with the
    /// physical offset of its first byte; absent for streamed input.
    stable: Option<(&'input str, usize)>,
    encoding: SourceEncoding,
    start: usize,
    decoded_start: usize,
    position: usize,
    unit_start: usize,
    limits: DtdLimits,
    counters: ControllerCounters,
    ready: Option<(Ready<'input>, FragmentOrigin)>,
    held_end: Option<FragmentOrigin>,
    started: bool,
    finalizing: bool,
    complete: bool,
    cut: Option<(Option<usize>, usize)>,
    resume: Option<(usize, usize)>,
}

pub(crate) type DtdController = DtdControllerCore<'static>;
pub(crate) type BorrowedDtdController<'input> = DtdControllerCore<'input>;

impl DtdControllerCore<'static> {
    pub(crate) fn new_at(
        encoding: SourceEncoding,
        physical_start: usize,
        decoded_start: usize,
        limits: DtdLimits,
        standalone: bool,
    ) -> Self {
        Self::with_input(
            encoding,
            physical_start,
            decoded_start,
            limits,
            standalone,
            None,
        )
    }

    /// What content parsed in a document's context reads with (from offset
    /// 0): `dtd`'s entities and notations. `xmlCtxtParseContent` looks
    /// entities up in the document, and its fresh context has no attribute
    /// defaults or normalization.
    pub(crate) fn seeded(
        limits: DtdLimits,
        dtd: &crate::valid::Dtd,
        loader: Option<crate::resolve::Loader>,
    ) -> Result<DtdBodyState, Error> {
        let mut controller = Self::new_at(SourceEncoding::Utf8, 0, 0, limits, false);
        controller.catalog.seed(dtd);
        if let Some(loader) = loader {
            controller.set_loader(loader);
        }
        controller.into_body_state()
    }

    /// Parses the DOCTYPE starting at `physical_start` of `source`, whose
    /// first byte is at physical offset `source_start`.
    pub(crate) fn stable_utf8<'input>(
        source: &'input str,
        source_start: usize,
        physical_start: usize,
        limits: DtdLimits,
        standalone: bool,
    ) -> BorrowedDtdController<'input> {
        debug_assert!(source.is_char_boundary(physical_start - source_start));
        DtdControllerCore::with_input(
            SourceEncoding::Utf8,
            physical_start,
            physical_start,
            limits,
            standalone,
            Some((source, source_start)),
        )
    }
}

impl<'input> DtdControllerCore<'input> {
    fn with_input(
        encoding: SourceEncoding,
        physical_start: usize,
        decoded_start: usize,
        limits: DtdLimits,
        standalone: bool,
        stable: Option<(&'input str, usize)>,
    ) -> Self {
        Self {
            physical: DtdScanner::new(ScanMode::Doctype, limits.token, limits.work),
            fragment: DtdScanner::new(ScanMode::SubsetFragment, limits.token, limits.work),
            catalog: DtdCatalog::new(limits, standalone),
            frames: EntityFrames::new(limits),
            stable,
            encoding,
            start: physical_start,
            decoded_start,
            position: physical_start,
            unit_start: physical_start,
            limits,
            counters: ControllerCounters::default(),
            ready: None,
            held_end: None,
            started: false,
            finalizing: false,
            complete: false,
            cut: None,
            resume: None,
        }
    }

    pub(crate) fn physical_position(&self) -> usize {
        self.position
    }
    pub(crate) fn is_complete(&self) -> bool {
        self.complete
    }
    pub(crate) fn start(&self) -> usize {
        self.start
    }
    pub(crate) fn catalog(&self) -> &DtdCatalog {
        &self.catalog
    }
    pub(crate) fn catalog_mut(&mut self) -> &mut DtdCatalog {
        &mut self.catalog
    }
    pub(crate) fn encoding(&self) -> SourceEncoding {
        self.encoding
    }
    /// Decoded bytes the physical scanner consumed, from `decoded_start`.
    pub(crate) fn decoded_position(&self) -> usize {
        self.decoded_start + self.physical.position()
    }

    /// `Budget::recover` from a declaration error: under `RECOVER` the
    /// declarations end here and the DOCTYPE is ready to publish.
    pub(crate) fn recover(&mut self, error: Error, budget: &mut Budget) -> Result<(), Error> {
        budget.recover(error)?;
        self.cut = Some((self.started.then_some(self.kept()), error.offset));
        self.ready = Some((Ready::End, input_origin(self.start..self.position)));
        Ok(())
    }

    /// The end of the internal subset recovery keeps: the start of the
    /// unit or parameter-entity reference being read.
    fn kept(&self) -> usize {
        self.frames
            .stack()
            .first()
            .map_or(self.unit_start, |frame| frame.physical_reference.start)
    }

    /// After `recover`: where the internal subset recovery keeps ends (once
    /// the header was read), and the error's offset.
    pub(crate) fn cut(&self) -> Option<(Option<usize>, usize)> {
        self.cut
    }

    /// Where the document resumes after `abandon`, decoded and physical.
    pub(crate) fn resume_at(&mut self, decoded: usize, physical: usize) {
        self.resume = Some((decoded, physical));
    }
    pub(crate) fn current(&self) -> Option<(DtdEvent<'_>, FragmentOrigin)> {
        let (ready, origin) = self.ready.as_ref()?;
        let event = match ready {
            Ready::Begin => DtdEvent::Header,
            Ready::Item(unit) => DtdEvent::Item(unit),
            Ready::End => DtdEvent::End,
        };
        Some((event, *origin))
    }

    /// Releases the ready event; a header or salvaged item can hold the end.
    pub(crate) fn release(&mut self) {
        match self.ready.take() {
            Some((Ready::End, _)) => self.complete = true,
            _ => self.ready = self.held_end.take().map(|end| (Ready::End, end)),
        }
    }

    /// Scans `input`; callers stop before `finish_step`.
    pub(crate) fn advance(&mut self, input: &[u8], budget: &mut Budget) -> Result<usize, Error> {
        self.drive(input, budget)
    }

    /// Scans the rest of the stable input, which declarations end within:
    /// drive stops early only at a ready event.
    pub(crate) fn advance_borrowed(&mut self, budget: &mut Budget) -> Result<usize, Error> {
        let (source, source_start) = self.stable.expect("stable input");
        let consumed = self.drive(&source.as_bytes()[self.position - source_start..], budget)?;
        if self.ready.is_none() {
            self.finalizing = true;
            self.drive(&[], budget)?;
        }
        Ok(consumed)
    }

    pub(crate) fn finish_step(&mut self, budget: &mut Budget) -> Result<(), Error> {
        self.finalizing = true;
        self.drive(&[], budget).map(|_| ())
    }

    pub(crate) fn set_loader(&mut self, loader: crate::resolve::Loader) {
        self.catalog.set_loader(loader);
    }

    /// Completes the declarations, loading the external subset last as libxml2 does.
    pub(crate) fn into_body_state(mut self) -> Result<DtdBodyState, Error> {
        if let Err(error) = self.catalog.include_subset() {
            return Err(self.simple_error(error.kind));
        }
        let (decoded_position, physical_position) = self
            .resume
            .unwrap_or((self.decoded_position(), self.position));
        // Recovery leaves the parameter entities it was expanding.
        let frames = match self.cut {
            Some(_) => self.frames.unwound(),
            None => self.frames,
        };
        Ok(DtdBodyState {
            catalog: self.catalog,
            frames,
            limits: self.limits,
            counters: self.counters,
            decoded_position,
            physical_position,
            encoding: self.encoding,
        })
    }

    fn drive(&mut self, input: &[u8], budget: &mut Budget) -> Result<usize, Error> {
        let mut cursor = 0;
        while self.ready.is_none() {
            if self.frames.top().is_some() {
                self.drive_replacement(budget)?;
                continue;
            }
            if self.physical.current().is_some() {
                self.process_ready(Side::Physical, budget)?;
                continue;
            }
            if cursor == input.len() {
                if self.finalizing {
                    // Physical EOF either readies trailing whitespace or fails.
                    if let Err(error) = self.physical.finish_step() {
                        return Err(self.scan_error(Side::Physical, error));
                    }
                    continue;
                }
                return Ok(cursor);
            }
            // UTF-8 bytes each cost one physical byte, so the scanner takes
            // as many as the DOCTYPE ceiling allows; other encodings go
            // byte by byte.
            let rest = &input[cursor..];
            let utf8 = self.encoding == SourceEncoding::Utf8;
            let allowed = self.limits.declarations - self.counters.physical_bytes;
            let length = if utf8 {
                rest.len().min(allowed).max(1)
            } else {
                1
            };
            let cost = |bytes: &[u8]| match utf8 {
                true => bytes.len(),
                false => self.encoding.source_length(bytes),
            };
            let physical_block = self
                .counters
                .physical_bytes
                .checked_add(cost(&rest[..1]))
                .is_none_or(|count| count > self.limits.declarations);
            // The scanner's own work ceiling stops it when either budget runs out.
            let remaining = if physical_block {
                0
            } else {
                self.limits.work - self.counters.work
            };
            self.physical
                .cap_work(self.physical.position().saturating_add(remaining));
            let before = self.physical.position();
            let result = self.physical.advance(&rest[..length]);
            let consumed = self.physical.position() - before;
            self.counters.work += consumed;
            if consumed != 0 {
                let cost = cost(&rest[..consumed]);
                self.position += cost;
                self.counters.physical_bytes += cost;
                cursor += consumed;
            }
            if let Err(error) = result {
                return Err(match error.kind {
                    WORK if physical_block => {
                        self.simple_error(ErrorKind::LimitExceeded(Limit::DtdBytes))
                    }
                    WORK => self.simple_error(WORK),
                    _ => self.scan_error(Side::Physical, error),
                });
            }
        }
        Ok(cursor)
    }

    fn drive_replacement(&mut self, budget: &mut Budget) -> Result<(), Error> {
        if self.fragment.current().is_some() {
            return self.process_ready(Side::Replacement, budget);
        }
        let frame = self.frames.top().expect("active parameter source");
        let replacement = self.catalog.replacement(frame.entity).expect("replacement");
        let cursor = frame.cursor + self.fragment.pending().0.len();
        if cursor == replacement.len() {
            if let Err(error) = self.fragment.finish_step() {
                return Err(self.scan_error(Side::Replacement, error));
            }
            if self.fragment.current().is_some() {
                return self.process_ready(Side::Replacement, budget);
            }
            self.frames.leave();
            self.fragment.restart_fragment();
            return Ok(());
        }
        let ceiling = self.limits.work - self.counters.work;
        self.fragment
            .cap_work(self.fragment.position().saturating_add(ceiling));
        let before = self.fragment.position();
        // The scanner stops at the first complete unit, so whole slices are safe.
        let result = self.fragment.advance(&replacement.as_bytes()[cursor..]);
        self.counters.work += self.fragment.position() - before;
        result.map(drop).map_err(|error| match error.kind {
            WORK => self.simple_error(WORK),
            _ => self.scan_error(Side::Replacement, error),
        })
    }

    fn process_ready(&mut self, side: Side, budget: &mut Budget) -> Result<(), Error> {
        share_expansion(
            &self.limits,
            self.position,
            &mut self.frames,
            &mut self.catalog,
        );
        let (kind, raw_len) = self
            .scanner(side)
            .current()
            .map(|(kind, raw, _)| (kind, raw.len()))
            .expect("ready unit");
        let origin = self.pending_origin(side);
        if kind == ScanKind::ParameterReference {
            return self.enter_parameter(side, origin, budget);
        }
        if kind == ScanKind::Whitespace {
            self.commit_source(side, raw_len)?;
            self.release_scanner(side);
            return Ok(());
        }
        // Only the physical scanner reads `]>`.
        if kind == ScanKind::End {
            self.release_scanner(side);
            self.ready = Some((Ready::End, origin));
            return Ok(());
        }
        // Grammar is always checked before an input-mode-dependent raw copy.
        self.charge_work(raw_len, origin)?;
        let (encoding, started, limits) = (self.encoding, self.started, self.limits);
        let mut input = None;
        let (unit, salvaged) = match (side, self.stable) {
            (Side::Physical, Some((source, source_start))) => {
                let range = origin.input_range().expect("physical unit");
                let raw = &source[range.start - source_start..range.end - source_start];
                input = Some(range.start - source_start);
                parse_unit(raw, limits, (kind, side, started), encoding, origin, budget)?
            }
            _ => {
                let raw = std::str::from_utf8(self.scanner(side).current().expect("ready").1)
                    .expect("scanner validates ready UTF-8");
                let (borrowed, salvaged) =
                    parse_unit(raw, limits, (kind, side, started), encoding, origin, budget)?;
                // A streamed unit is retained beyond its scanner as an owned copy.
                let limit = |count: usize, ceiling| {
                    count.checked_add(raw_len).filter(|count| *count <= ceiling)
                };
                let owned =
                    limit(self.counters.owned_bytes, self.limits.storage).ok_or(Error::new(
                        ErrorKind::LimitExceeded(Limit::OwnedBytes),
                        origin.error_offset(),
                    ))?;
                let work = limit(self.counters.work, self.limits.work)
                    .ok_or(Error::new(WORK, origin.error_offset()))?;
                let unit = borrowed
                    .try_into_owned(budget, origin.error_offset())
                    .map_err(|error| {
                        Error::new(error.kind, unit_offset(encoding, origin, raw, 0))
                    })?;
                (self.counters.work, self.counters.owned_bytes) = (work, owned);
                (unit, salvaged)
            }
        };
        self.commit_source(side, raw_len)?;
        // A colon in an entity or notation name is a namespace error
        // (libxml2's `XML_NS_ERR_COLON`).
        let colon = match unit.view() {
            DtdUnitView::Entity(entity) => entity.name().contains(':'),
            DtdUnitView::Notation(notation) => notation.name().contains(':'),
            _ => false,
        };
        if colon {
            let offset = unit_offset(encoding, origin, unit.raw(), 0);
            budget.recover(Error::new(ErrorKind::Namespace, offset))?;
        }
        let literals = match side {
            Side::Physical => LiteralSource::Physical,
            Side::Replacement => LiteralSource::Replacement,
        };
        let before = self.catalog.usage().work;
        self.catalog
            .cap_work(before.saturating_add(self.limits.work - self.counters.work));
        let result = self.catalog.apply(
            &unit,
            DeclarationContext {
                internal_subset: true,
                literals,
                input,
            },
        );
        self.counters.work += self.catalog.usage().work - before;
        let redeclared = result.map_err(|error| {
            let offset = match error.offset {
                Some(offset) => unit_offset(encoding, origin, unit.raw(), offset),
                None => origin.error_offset(),
            };
            Error::new(error.kind, offset)
        })?;
        if redeclared {
            let offset = unit_offset(encoding, origin, unit.raw(), 0);
            let warning = ErrorKind::Warning(crate::Warning::PredefinedEntity);
            budget.recover(Error::new(warning, offset))?;
        }
        // Released once applied: recovery cuts the subset before a unit the
        // catalog rejects.
        self.release_scanner(side);
        // Recovery keeps the declaration libxml2 makes before an error,
        // where it then stops.
        if let Some(at) = salvaged {
            self.cut = Some((Some(self.position), at));
            self.held_end = FragmentOrigin::input(self.start..self.position);
        }
        let subset = match unit.view() {
            DtdUnitView::Header(header) => Some(header.has_internal_subset()),
            _ => None,
        };
        self.ready = Some(match subset {
            None => (Ready::Item(unit), origin),
            Some(subset) => {
                self.started = true;
                match subset {
                    true => (Ready::Begin, origin),
                    false => {
                        let range = origin.input_range().expect("physical header");
                        let delimiter = range.end - self.encoding.source_length(b">");
                        self.held_end = Some(input_origin(delimiter..range.end));
                        (Ready::Begin, input_origin(range.start..delimiter))
                    }
                }
            }
        });
        Ok(())
    }

    fn enter_parameter(
        &mut self,
        side: Side,
        origin: FragmentOrigin,
        budget: &mut Budget,
    ) -> Result<(), Error> {
        // libxml2 sets hasPErefs before resolving the name.
        self.catalog.note_parameter_reference();
        let raw = self.scanner(side).current().expect("parameter raw").1;
        let raw_len = raw.len();
        let raw = std::str::from_utf8(raw).expect("validated PE reference");
        let name = &raw[1..raw.len() - 1];
        let fail = |kind| Error::new(kind, origin.error_offset());
        let (id, len) = match self.catalog.lookup_parameter(name) {
            EntityLookup::Internal { id, replacement } => (id, replacement.len()),
            // External text is processed whole when the caller's resolver
            // supplies it; otherwise, like libxml2's failed load, it is skipped.
            EntityLookup::ExternalUnavailable { id } => {
                // A skipped reference still consumes (and pays for) its spelling.
                self.commit_source(side, raw_len)?;
                self.release_scanner(side);
                return self
                    .catalog
                    .include_parameter(id, self.frames.stack())
                    .map_err(|error| fail(error.kind));
            }
            // Like libxml2's undeclared validity error, the reference is skipped.
            EntityLookup::UnreadEntity => {
                let message = format!("Entity '{name}' not defined");
                let offset = origin.input_range().map(|range| range.start);
                self.catalog
                    .note(crate::valid::Code::UndeclaredEntity, offset, message);
                if let Some(warning) = self.catalog.undeclared_warning() {
                    budget.recover(Error::new(
                        ErrorKind::Warning(warning),
                        origin.error_offset(),
                    ))?;
                }
                self.commit_source(side, raw_len)?;
                self.release_scanner(side);
                return Ok(());
            }
            // Parameter entities are never unparsed.
            EntityLookup::Unparsed | EntityLookup::Undeclared => {
                return Err(fail(ErrorKind::UnknownEntity));
            }
        };
        self.release_scanner(side);
        let before = self.frames.counters().work;
        self.frames
            .cap_work(before.saturating_add(self.limits.work - self.counters.work));
        let reference = match side {
            Side::Physical => origin.physical_context(),
            Side::Replacement => origin.replacement_range().expect("nested reference"),
        };
        let result = self.frames.enter(id, len, reference, None);
        self.counters.work += self.frames.counters().work - before;
        result.map(drop).map_err(|error| fail(error.kind))
    }

    fn commit_source(&mut self, side: Side, bytes: usize) -> Result<(), Error> {
        if matches!(side, Side::Physical) {
            return Ok(());
        }
        let before = self.frames.counters().work;
        self.frames
            .cap_work(before.saturating_add(self.limits.work - self.counters.work));
        let result = self.frames.advance(bytes);
        self.counters.work += self.frames.counters().work - before;
        result
    }

    fn scanner(&self, side: Side) -> &DtdScanner {
        match side {
            Side::Physical => &self.physical,
            Side::Replacement => &self.fragment,
        }
    }

    fn release_scanner(&mut self, side: Side) {
        match side {
            Side::Physical => {
                self.physical.release();
                self.unit_start = self.position;
            }
            Side::Replacement => {
                self.fragment.release();
                self.fragment.restart_fragment();
            }
        }
    }

    fn pending_origin(&self, side: Side) -> FragmentOrigin {
        let Side::Replacement = side else {
            return FragmentOrigin::input(self.unit_start..self.position).expect("ordered");
        };
        let frame = self.frames.top().expect("replacement origin");
        let local = frame.cursor..frame.cursor + self.fragment.pending().0.len();
        FragmentOrigin::Replacement {
            declaration: frame.entity,
            invocation: frame.invocation,
            local: crate::dtd_frames::ByteSpan::new(local).expect("ordered"),
            physical_reference: frame.physical_reference,
        }
    }

    fn scan_error(&self, side: Side, error: Error) -> Error {
        let (raw, range) = self.scanner(side).pending();
        if raw.is_empty() && matches!(side, Side::Physical) {
            return self.simple_error(error.kind);
        }
        let prefix = match std::str::from_utf8(raw) {
            Ok(raw) => raw,
            Err(error) => {
                std::str::from_utf8(&raw[..error.valid_up_to()]).expect("UTF-8 valid prefix")
            }
        };
        let relative = error.offset.saturating_sub(range.start);
        let origin = self.pending_origin(side);
        Error::new(
            error.kind,
            unit_offset(self.encoding, origin, prefix, relative),
        )
    }

    /// Fails at the active replacement's reference, else the current position.
    fn simple_error(&self, kind: ErrorKind) -> Error {
        let offset = match self.frames.top() {
            Some(_) => self.pending_origin(Side::Replacement).error_offset(),
            None => self.position,
        };
        Error::new(kind, offset)
    }

    fn charge_work(&mut self, bytes: usize, origin: FragmentOrigin) -> Result<(), Error> {
        self.counters.work = self
            .counters
            .work
            .checked_add(bytes)
            .filter(|work| *work <= self.limits.work)
            .ok_or(Error::new(WORK, origin.error_offset()))?;
        Ok(())
    }
}

/// The origin of physical input `range`, which callers keep ordered.
fn input_origin(range: std::ops::Range<usize>) -> FragmentOrigin {
    FragmentOrigin::Input {
        span: crate::dtd_frames::ByteSpan {
            start: range.start,
            end: range.end,
        },
    }
}

impl DtdBodyState {
    pub(crate) fn resume_position(&self) -> (usize, usize) {
        (self.decoded_position, self.physical_position)
    }
}

/// Parses `raw`, read from `origin`, as a unit the scanner classified as
/// `kind`; with the offset of a recovered error after which libxml2 stops.
fn parse_unit<'raw>(
    raw: &'raw str,
    limits: DtdLimits,
    (kind, side, started): (ScanKind, Side, bool),
    encoding: SourceEncoding,
    origin: FragmentOrigin,
    budget: &mut Budget,
) -> Result<(DtdUnit<'raw>, Option<usize>), Error> {
    let located =
        |error: Error| Error::new(error.kind, unit_offset(encoding, origin, raw, error.offset));
    let mut recover = |error: Error| {
        let failure = budget.recover(located(error));
        failure.map_err(|failure| Error::new(failure.kind, error.offset))
    };
    let (unit, cut) = DtdUnit::parse_recovering(raw, limits, &mut recover).map_err(located)?;
    match valid_unit(kind, side, started, &unit) {
        true => Ok((unit, cut.map(|at| unit_offset(encoding, origin, raw, at)))),
        false => Err(Error::new(ErrorKind::InvalidDtd, origin.error_offset())),
    }
}

fn valid_unit(kind: ScanKind, side: Side, started: bool, unit: &DtdUnit<'_>) -> bool {
    match (kind, unit.view()) {
        (ScanKind::Header, DtdUnitView::Header(_)) => matches!(side, Side::Physical) && !started,
        (
            ScanKind::Declaration,
            DtdUnitView::Element(_)
            | DtdUnitView::AttributeList(_)
            | DtdUnitView::Entity(_)
            | DtdUnitView::Notation(_),
        )
        | (ScanKind::Comment, DtdUnitView::Comment(_))
        | (ScanKind::Instruction, DtdUnitView::ProcessingInstruction(_)) => true,
        _ => false,
    }
}

/// The physical offset of `relative` within `raw`, a unit read from `origin`.
/// Replacement text has no physical position of its own.
fn unit_offset(
    encoding: SourceEncoding,
    origin: FragmentOrigin,
    raw: &str,
    relative: usize,
) -> usize {
    let Some(source) = origin.input_range() else {
        return origin.error_offset();
    };
    let offset = match encoding {
        SourceEncoding::Utf8 => source.start.saturating_add(relative),
        _ => relative,
    };
    let span = TokenSpan {
        decoded: 0..raw.len(),
        source,
        encoding,
    };
    span.map_error(raw, Error::new(ErrorKind::InvalidState, offset))
        .offset
}
