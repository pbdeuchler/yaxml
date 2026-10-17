//! Private internal-subset composition. No public XML parser enables this yet.

use std::ops::Range;

use crate::dtd::{DoctypeHeaderView, DtdErrorKind, DtdLimits, DtdUnit, DtdUnitView};
use crate::dtd_catalog::{
    CatalogErrorKind, CatalogLimits, DeclarationContext, DependencyKind, DtdCatalog, EntityId,
    EntityLookup, LiteralSource, ValueLocation,
};
use crate::dtd_frames::{
    ByteSpan, EntityDeclarationId, EntityFrames, EntityKind, EntityResolution, FragmentOrigin,
    FrameBoundary, FrameErrorKind, FrameLimits, LookupFailure,
};
use crate::dtd_scan::{DtdScanner, ScanErrorKind, ScanKind, ScanLimits, ScanMode};
use crate::encoding::{SourceEncoding, TokenSpan};
use crate::parser::Budget;
use crate::{Error, ErrorKind};

#[derive(Clone, Copy, Debug)]
pub(crate) struct ControllerLimits {
    pub(crate) scan: ScanLimits,
    pub(crate) unit: DtdLimits,
    pub(crate) catalog: CatalogLimits,
    pub(crate) frames: FrameLimits,
    pub(crate) max_physical_bytes: usize,
    pub(crate) max_work: usize,
    pub(crate) max_owned_bytes: usize,
}

impl Default for ControllerLimits {
    fn default() -> Self {
        Self {
            scan: ScanLimits {
                max_unit_bytes: 64 * 1024,
                max_scratch_bytes: 64 * 1024,
                max_work: 64 * 1024 * 1024,
            },
            unit: DtdLimits::default(),
            catalog: CatalogLimits::default(),
            frames: FrameLimits::default(),
            max_physical_bytes: 16 * 1024 * 1024,
            max_work: 64 * 1024 * 1024,
            max_owned_bytes: 16 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ControllerCounters {
    pub(crate) work: usize,
    pub(crate) scanned_bytes: usize,
    pub(crate) owned_bytes: usize,
    pub(crate) physical_bytes: usize,
    pub(crate) reference_attempts: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ControllerLimit {
    PhysicalBytes,
    Work,
    OwnedBytes,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PeUnavailable {
    Undeclared,
    External,
    Unparsed,
    Unread,
    Dependency(DependencyKind),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ControllerErrorKind {
    Scan(ScanErrorKind),
    Grammar(DtdErrorKind),
    Catalog(CatalogErrorKind),
    Frame(FrameErrorKind),
    Unavailable(PeUnavailable),
    ValueUnavailable(DependencyKind),
    Ownership(ErrorKind),
    Limit(ControllerLimit),
    PositionOverflow,
    InvalidUnitFamily,
    Allocation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ControllerError {
    pub(crate) kind: ControllerErrorKind,
    pub(crate) physical_offset: usize,
    pub(crate) origin: Option<FragmentOrigin>,
    pub(crate) local_offset: Option<usize>,
    pub(crate) value_location: Option<ValueLocation>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum DtdEvent<'a> {
    Begin(DoctypeHeaderView<'a>),
    Item {
        unit: &'a DtdUnit<'static>,
        literals: LiteralSource,
    },
    End,
}

#[derive(Clone, Copy)]
enum Ready {
    Begin,
    Item(LiteralSource),
    End,
}
#[derive(Clone, Copy)]
enum Side {
    Physical,
    Replacement,
}

pub(crate) struct DtdController {
    physical: DtdScanner,
    fragment: DtdScanner,
    catalog: DtdCatalog,
    frames: EntityFrames,
    active: Vec<EntityId>,
    encoding: SourceEncoding,
    start: usize,
    position: usize,
    unit_start: usize,
    limits: ControllerLimits,
    counters: ControllerCounters,
    unit: Option<DtdUnit<'static>>,
    ready: Option<Ready>,
    origin: Option<FragmentOrigin>,
    held_end: Option<FragmentOrigin>,
    started: bool,
    finalizing: bool,
    complete: bool,
    failure: Option<ControllerError>,
}

impl DtdController {
    pub(crate) fn new(
        encoding: SourceEncoding,
        physical_start: usize,
        limits: ControllerLimits,
        standalone: bool,
    ) -> Self {
        Self {
            physical: DtdScanner::new(ScanMode::Doctype, limits.scan),
            fragment: DtdScanner::new(ScanMode::SubsetFragment, limits.scan),
            catalog: DtdCatalog::new(limits.catalog, standalone),
            frames: EntityFrames::new(limits.frames),
            active: Vec::new(),
            encoding,
            start: physical_start,
            position: physical_start,
            unit_start: physical_start,
            limits,
            counters: ControllerCounters::default(),
            unit: None,
            ready: None,
            origin: None,
            held_end: None,
            started: false,
            finalizing: false,
            complete: false,
            failure: None,
        }
    }

    pub(crate) fn physical_position(&self) -> usize {
        self.position
    }
    pub(crate) fn counters(&self) -> ControllerCounters {
        self.counters
    }
    pub(crate) fn catalog(&self) -> &DtdCatalog {
        &self.catalog
    }
    pub(crate) fn is_complete(&self) -> bool {
        self.complete
    }
    pub(crate) fn doctype_range(&self) -> Option<Range<usize>> {
        self.complete.then_some(self.start..self.position)
    }

    pub(crate) fn current(&self) -> Option<(DtdEvent<'_>, FragmentOrigin)> {
        let event = match self.ready? {
            Ready::Begin => match self.unit.as_ref()?.view() {
                DtdUnitView::Header(header) => DtdEvent::Begin(header),
                _ => unreachable!("checked header event"),
            },
            Ready::Item(literals) => DtdEvent::Item {
                unit: self.unit.as_ref()?,
                literals,
            },
            Ready::End => DtdEvent::End,
        };
        Some((event, self.origin?))
    }

    pub(crate) fn release(&mut self) {
        let Some(ready) = self.ready.take() else {
            return;
        };
        self.unit = None;
        self.origin = None;
        if matches!(ready, Ready::Begin)
            && let Some(end) = self.held_end.take()
        {
            self.ready = Some(Ready::End);
            self.origin = Some(end);
        } else if matches!(ready, Ready::End) {
            self.complete = true;
        }
    }

    pub(crate) fn advance(
        &mut self,
        input: &[u8],
        budget: &mut Budget,
    ) -> Result<usize, ControllerError> {
        self.remember(|controller| {
            controller.drive(if controller.finalizing { &[] } else { input }, budget)
        })
    }

    pub(crate) fn finish_step(&mut self, budget: &mut Budget) -> Result<(), ControllerError> {
        self.remember(|controller| {
            controller.finalizing = true;
            controller.drive(&[], budget).map(|_| ())
        })
    }

    fn drive(&mut self, input: &[u8], budget: &mut Budget) -> Result<usize, ControllerError> {
        let mut cursor = 0;
        while self.ready.is_none() && !self.complete {
            if !self.active.is_empty() {
                self.drive_replacement(budget)?;
                continue;
            }
            if self.physical.current().is_some() {
                self.process_ready(Side::Physical, budget)?;
                continue;
            }
            if cursor == input.len() {
                if self.finalizing {
                    if let Err(error) = self.physical.finish_step() {
                        return Err(self.scan_error(Side::Physical, error.kind, error.offset));
                    }
                    if self.physical.current().is_some() {
                        continue;
                    }
                }
                return Ok(cursor);
            }
            let byte = input[cursor];
            let cost = self.encoding.source_length(std::slice::from_ref(&byte));
            let next = self
                .position
                .checked_add(cost)
                .ok_or_else(|| self.simple_error(ControllerErrorKind::PositionOverflow))?;
            let physical_block = self
                .counters
                .physical_bytes
                .checked_add(cost)
                .is_none_or(|count| count > self.limits.max_physical_bytes);
            let global_remaining = self.limits.max_work - self.counters.work;
            let scan_remaining = self.limits.scan.max_work - self.counters.scanned_bytes;
            let remaining = global_remaining.min(scan_remaining);
            self.physical
                .cap_work(self.physical.position().saturating_add(remaining));
            if physical_block {
                self.physical.cap_work(self.physical.position());
            }
            let before = self.physical.position();
            let result = self.physical.advance(std::slice::from_ref(&byte));
            let consumed = self.physical.position() - before;
            self.counters.work += consumed;
            self.counters.scanned_bytes += consumed;
            if consumed != 0 {
                self.position = next;
                self.counters.physical_bytes += cost;
                cursor += consumed;
            }
            if let Err(error) = result {
                if error.kind == ScanErrorKind::Limit(crate::dtd_scan::ScanLimit::Work)
                    && (physical_block || global_remaining == 0)
                {
                    return Err(
                        self.simple_error(ControllerErrorKind::Limit(if physical_block {
                            ControllerLimit::PhysicalBytes
                        } else {
                            ControllerLimit::Work
                        })),
                    );
                }
                return Err(self.scan_error(Side::Physical, error.kind, error.offset));
            }
        }
        Ok(cursor)
    }

    fn drive_replacement(&mut self, budget: &mut Budget) -> Result<(), ControllerError> {
        if self.fragment.current().is_some() {
            return self.process_ready(Side::Replacement, budget);
        }
        let id = *self.active.last().expect("active parameter source");
        let declaration = EntityDeclarationId::from_index(id.index());
        let replacement = self
            .catalog
            .replacement(id)
            .expect("active immutable replacement");
        let source = self
            .frames
            .current(declaration, replacement)
            .map_err(|error| self.frame_error(error))?;
        let pending = self.fragment.pending().0.len();
        let cursor = source.local_start + pending;
        if cursor == replacement.len() {
            if let Err(error) = self.fragment.finish_step() {
                return Err(self.scan_error(Side::Replacement, error.kind, error.offset));
            }
            if self.fragment.current().is_some() {
                return self.process_ready(Side::Replacement, budget);
            }
            self.frames
                .finish_current(FrameBoundary::DtdSubset)
                .map_err(|error| self.frame_error(error))?;
            self.active.pop();
            self.fragment
                .restart_fragment()
                .expect("finished complete fragment");
            return Ok(());
        }
        let byte = replacement.as_bytes()[cursor];
        let global_remaining = self.limits.max_work - self.counters.work;
        let scan_remaining = self.limits.scan.max_work - self.counters.scanned_bytes;
        let remaining = global_remaining.min(scan_remaining);
        self.fragment
            .cap_work(self.fragment.position().saturating_add(remaining));
        let before = self.fragment.position();
        let result = self.fragment.advance(std::slice::from_ref(&byte));
        let consumed = self.fragment.position() - before;
        self.counters.work += consumed;
        self.counters.scanned_bytes += consumed;
        if let Err(error) = result {
            if error.kind == ScanErrorKind::Limit(crate::dtd_scan::ScanLimit::Work)
                && global_remaining == 0
            {
                return Err(self.simple_error(ControllerErrorKind::Limit(ControllerLimit::Work)));
            }
            return Err(self.scan_error(Side::Replacement, error.kind, error.offset));
        }
        Ok(())
    }

    fn process_ready(&mut self, side: Side, budget: &mut Budget) -> Result<(), ControllerError> {
        let (kind, raw_len) = match side {
            Side::Physical => self.physical.current(),
            Side::Replacement => self.fragment.current(),
        }
        .map(|(kind, raw, _)| (kind, raw.len()))
        .expect("ready unit");
        let origin = self.pending_origin(side)?;
        if kind == ScanKind::ParameterReference {
            return self.enter_parameter(side, origin);
        }
        if kind == ScanKind::Whitespace {
            self.commit_source(side, raw_len)?;
            self.release_scanner(side);
            return Ok(());
        }
        if kind == ScanKind::End {
            if !matches!(side, Side::Physical) {
                return Err(self.simple_error(ControllerErrorKind::InvalidUnitFamily));
            }
            self.release_scanner(side);
            self.ready = Some(Ready::End);
            self.origin = Some(origin);
            return Ok(());
        }
        self.charge_work(raw_len, Some(origin))?;
        let next_owned = self
            .counters
            .owned_bytes
            .checked_add(raw_len)
            .filter(|count| *count <= self.limits.max_owned_bytes)
            .ok_or_else(|| {
                self.error_for_origin(
                    ControllerErrorKind::Limit(ControllerLimit::OwnedBytes),
                    origin,
                    None,
                )
            })?;
        self.charge_work(raw_len, Some(origin))?;
        let unit = {
            let raw = match side {
                Side::Physical => self.physical.current(),
                Side::Replacement => self.fragment.current(),
            }
            .expect("ready raw")
            .1;
            let raw = std::str::from_utf8(raw).expect("scanner validates ready UTF-8");
            let borrowed = DtdUnit::parse(raw, self.limits.unit).map_err(|error| {
                map_unit_error(
                    self.encoding,
                    origin,
                    raw,
                    error.offset,
                    ControllerErrorKind::Grammar(error.kind),
                )
            })?;
            borrowed
                .try_into_owned(budget, origin.physical_context().start)
                .map_err(|error| {
                    map_unit_error(
                        self.encoding,
                        origin,
                        raw,
                        0,
                        ControllerErrorKind::Ownership(error.kind),
                    )
                })?
        };
        self.counters.owned_bytes = next_owned;
        let header = matches!(unit.view(), DtdUnitView::Header(_));
        let valid = match kind {
            ScanKind::Header => matches!(side, Side::Physical) && !self.started && header,
            ScanKind::Declaration => {
                !header
                    && matches!(
                        unit.view(),
                        DtdUnitView::Element(_)
                            | DtdUnitView::AttributeList(_)
                            | DtdUnitView::Entity(_)
                            | DtdUnitView::Notation(_)
                    )
            }
            ScanKind::Comment => matches!(unit.view(), DtdUnitView::Comment(_)),
            ScanKind::Instruction => matches!(unit.view(), DtdUnitView::ProcessingInstruction(_)),
            _ => false,
        };
        if !valid {
            return Err(self.error_for_origin(
                ControllerErrorKind::InvalidUnitFamily,
                origin,
                None,
            ));
        }
        self.commit_source(side, raw_len)?;
        self.release_scanner(side);
        let literals = if matches!(side, Side::Physical) {
            LiteralSource::Physical
        } else {
            LiteralSource::Replacement
        };
        let before = self.catalog.usage().work;
        let ceiling = before.saturating_add(self.limits.max_work - self.counters.work);
        self.catalog.cap_work(ceiling);
        let result = self.catalog.apply(
            &unit,
            DeclarationContext {
                internal_subset: true,
                literals,
            },
        );
        self.counters.work += self.catalog.usage().work - before;
        let effect = result.map_err(|error| {
            let kind = if error.kind
                == CatalogErrorKind::Limit(crate::dtd_catalog::CatalogLimit::Work)
                && ceiling <= self.limits.catalog.max_work
            {
                ControllerErrorKind::Limit(ControllerLimit::Work)
            } else {
                ControllerErrorKind::Catalog(error.kind)
            };
            let relative = match error.location {
                ValueLocation::Declaration { offset, .. } | ValueLocation::Input(offset) => {
                    Some(offset)
                }
                ValueLocation::Replacement { .. } => None,
            };
            let mut mapped = if let Some(relative) = relative {
                map_unit_error(self.encoding, origin, unit.raw(), relative, kind)
            } else {
                self.error_for_origin(kind, origin, None)
            };
            mapped.value_location = Some(error.location);
            mapped
        })?;
        if let Some(dependency) = effect.unavailable {
            let mut error = self.error_for_origin(
                ControllerErrorKind::ValueUnavailable(dependency.kind),
                origin,
                None,
            );
            error.value_location = Some(dependency.location);
            return Err(error);
        }
        if header {
            self.started = true;
            let DtdUnitView::Header(view) = unit.view() else {
                unreachable!()
            };
            let mut begin = origin;
            if !view.has_internal_subset() {
                let range = origin.input_range().expect("physical header");
                let delimiter = self.encoding.source_length(b">");
                begin = FragmentOrigin::input(range.start..range.end - delimiter)
                    .expect("real header range");
                self.held_end = FragmentOrigin::input(range.end - delimiter..range.end);
            }
            self.ready = Some(Ready::Begin);
            self.origin = Some(begin);
        } else {
            self.ready = Some(Ready::Item(literals));
            self.origin = Some(origin);
        }
        self.unit = Some(unit);
        Ok(())
    }

    fn enter_parameter(
        &mut self,
        side: Side,
        origin: FragmentOrigin,
    ) -> Result<(), ControllerError> {
        let (id, resolution, unavailable) = {
            let raw = match side {
                Side::Physical => self.physical.current(),
                Side::Replacement => self.fragment.current(),
            }
            .expect("parameter raw")
            .1;
            let raw = std::str::from_utf8(raw).expect("validated PE reference");
            let lookup = self.catalog.lookup_parameter(&raw[1..raw.len() - 1]);
            match lookup {
                EntityLookup::Internal { id, replacement } => (
                    Some(id),
                    EntityResolution::Internal {
                        kind: EntityKind::Parameter,
                        declaration: EntityDeclarationId::from_index(id.index()),
                        replacement_len: replacement.len(),
                    },
                    None,
                ),
                other => {
                    let (failure, unavailable) = match other {
                        EntityLookup::ExternalUnavailable { .. } => {
                            (LookupFailure::ExternalUnavailable, PeUnavailable::External)
                        }
                        EntityLookup::Unparsed { .. } => {
                            (LookupFailure::Unparsed, PeUnavailable::Unparsed)
                        }
                        EntityLookup::UnreadEntity => {
                            (LookupFailure::Unread, PeUnavailable::Unread)
                        }
                        EntityLookup::Unavailable(dependency) => (
                            LookupFailure::Unread,
                            PeUnavailable::Dependency(dependency.kind),
                        ),
                        _ => (LookupFailure::Undeclared, PeUnavailable::Undeclared),
                    };
                    (
                        None,
                        EntityResolution::Failed {
                            kind: EntityKind::Parameter,
                            failure,
                        },
                        Some(unavailable),
                    )
                }
            }
        };
        // Keep the ID stack ready before frames publish a child or move its parent.
        // Failed lookups and depth failures never need another active slot.
        if id.is_some() && self.active.len() < self.limits.frames.max_active_depth {
            self.active.try_reserve(1).map_err(|_| {
                self.error_for_origin(ControllerErrorKind::Allocation, origin, None)
            })?;
        }
        if unavailable.is_some() {
            self.catalog.note_unread_parameter();
        }
        self.release_scanner(side);
        let before = self.frames.counters().work;
        let ceiling = before.saturating_add(self.limits.max_work - self.counters.work);
        self.frames.cap_work(ceiling);
        let result = match side {
            Side::Physical => self.frames.enter_physical(
                EntityKind::Parameter,
                resolution,
                origin.physical_context(),
                FrameBoundary::DtdSubset,
            ),
            Side::Replacement => {
                let parent = *self.active.last().expect("nested parent");
                self.frames.enter_nested(
                    EntityKind::Parameter,
                    resolution,
                    EntityDeclarationId::from_index(parent.index()),
                    self.catalog.replacement(parent).expect("parent text"),
                    origin.replacement_range().expect("nested reference"),
                    FrameBoundary::DtdSubset,
                )
            }
        };
        self.counters.work += self.frames.counters().work - before;
        self.counters.reference_attempts = self.frames.counters().reference_attempts;
        if let Err(error) = result {
            if matches!(error.kind, FrameErrorKind::Lookup(_))
                && let Some(unavailable) = unavailable
            {
                return Err(self.error_for_origin(
                    ControllerErrorKind::Unavailable(unavailable),
                    origin,
                    None,
                ));
            }
            return Err(self.error_for_origin(
                if error.kind == FrameErrorKind::LimitExceeded(crate::dtd_frames::FrameLimit::Work)
                    && ceiling <= self.limits.frames.max_work
                {
                    ControllerErrorKind::Limit(ControllerLimit::Work)
                } else {
                    ControllerErrorKind::Frame(error.kind)
                },
                origin,
                origin.replacement_range().map(|range| range.start),
            ));
        }

        self.active.push(id.expect("successful internal PE"));
        Ok(())
    }

    fn commit_source(&mut self, side: Side, bytes: usize) -> Result<(), ControllerError> {
        if matches!(side, Side::Physical) {
            return Ok(());
        }
        let id = *self.active.last().expect("active unit source");
        let origin = self.pending_origin(Side::Replacement)?;
        let before = self.frames.counters().work;
        let ceiling = before.saturating_add(self.limits.max_work - self.counters.work);
        self.frames.cap_work(ceiling);
        let result = self.frames.advance_current(
            EntityDeclarationId::from_index(id.index()),
            self.catalog.replacement(id).expect("replacement source"),
            bytes,
        );
        self.counters.work += self.frames.counters().work - before;
        result.map(|_| ()).map_err(|error| {
            let kind = if error.kind
                == FrameErrorKind::LimitExceeded(crate::dtd_frames::FrameLimit::Work)
                && ceiling <= self.limits.frames.max_work
            {
                ControllerErrorKind::Limit(ControllerLimit::Work)
            } else {
                ControllerErrorKind::Frame(error.kind)
            };
            self.error_for_origin(
                kind,
                origin,
                origin.replacement_range().map(|range| range.start),
            )
        })
    }

    fn release_scanner(&mut self, side: Side) {
        match side {
            Side::Physical => {
                self.physical.release();
                self.unit_start = self.position;
            }
            Side::Replacement => {
                self.fragment.release();
                self.fragment
                    .restart_fragment()
                    .expect("released complete subset unit");
            }
        }
    }

    fn pending_origin(&self, side: Side) -> Result<FragmentOrigin, ControllerError> {
        match side {
            Side::Physical => Ok(FragmentOrigin::input(self.unit_start..self.position)
                .expect("ordered source progress")),
            Side::Replacement => {
                let id = *self.active.last().expect("replacement origin");
                let source = self
                    .frames
                    .current(
                        EntityDeclarationId::from_index(id.index()),
                        self.catalog.replacement(id).expect("replacement text"),
                    )
                    .map_err(|error| self.frame_error(error))?;
                Ok(FragmentOrigin::Replacement {
                    declaration: source.declaration,
                    invocation: source.invocation,
                    local: ByteSpan::new(
                        source.local_start..source.local_start + self.fragment.pending().0.len(),
                    )
                    .expect("bounded replacement range"),
                    physical_reference: ByteSpan::new(source.physical_reference)
                        .expect("real outer reference"),
                })
            }
        }
    }

    fn scan_error(&self, side: Side, kind: ScanErrorKind, offset: usize) -> ControllerError {
        let (raw, range) = match side {
            Side::Physical => self.physical.pending(),
            Side::Replacement => self.fragment.pending(),
        };
        if raw.is_empty() && matches!(side, Side::Physical) {
            return self.simple_error(ControllerErrorKind::Scan(kind));
        }
        let prefix = match std::str::from_utf8(raw) {
            Ok(raw) => raw,
            Err(error) => {
                std::str::from_utf8(&raw[..error.valid_up_to()]).expect("UTF-8 valid prefix")
            }
        };
        let origin = self.pending_origin(side).ok();
        if let Some(origin) = origin {
            map_unit_error(
                self.encoding,
                origin,
                prefix,
                offset.saturating_sub(range.start),
                ControllerErrorKind::Scan(kind),
            )
        } else {
            self.simple_error(ControllerErrorKind::Scan(kind))
        }
    }

    fn frame_error(&self, error: crate::dtd_frames::FrameError) -> ControllerError {
        ControllerError {
            kind: ControllerErrorKind::Frame(error.kind),
            physical_offset: error.physical_offset,
            origin: None,
            local_offset: None,
            value_location: None,
        }
    }

    fn simple_error(&self, kind: ControllerErrorKind) -> ControllerError {
        let origin = if self.active.is_empty() {
            None
        } else {
            self.pending_origin(Side::Replacement).ok()
        };
        match origin {
            Some(origin) => self.error_for_origin(kind, origin, None),
            None => ControllerError {
                kind,
                physical_offset: self.position,
                origin: None,
                local_offset: None,
                value_location: None,
            },
        }
    }

    fn error_for_origin(
        &self,
        kind: ControllerErrorKind,
        origin: FragmentOrigin,
        local_offset: Option<usize>,
    ) -> ControllerError {
        ControllerError {
            kind,
            physical_offset: origin.physical_context().start,
            origin: Some(origin),
            local_offset: local_offset
                .or_else(|| origin.replacement_range().map(|range| range.start)),
            value_location: None,
        }
    }

    fn charge_work(
        &mut self,
        bytes: usize,
        origin: Option<FragmentOrigin>,
    ) -> Result<(), ControllerError> {
        self.counters.work = self
            .counters
            .work
            .checked_add(bytes)
            .filter(|work| *work <= self.limits.max_work)
            .ok_or_else(|| match origin {
                Some(origin) => self.error_for_origin(
                    ControllerErrorKind::Limit(ControllerLimit::Work),
                    origin,
                    None,
                ),
                None => self.simple_error(ControllerErrorKind::Limit(ControllerLimit::Work)),
            })?;
        Ok(())
    }

    fn remember<T>(
        &mut self,
        operation: impl FnOnce(&mut Self) -> Result<T, ControllerError>,
    ) -> Result<T, ControllerError> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        let result = operation(self);
        if let Err(error) = result {
            self.failure = Some(error);
        }
        result
    }
}

fn map_unit_error(
    encoding: SourceEncoding,
    origin: FragmentOrigin,
    raw: &str,
    relative: usize,
    kind: ControllerErrorKind,
) -> ControllerError {
    let (physical_offset, local_offset) = match origin {
        FragmentOrigin::Input { .. } => {
            let source = origin.input_range().expect("input range");
            let span = TokenSpan {
                decoded: 0..raw.len(),
                source: source.clone(),
                encoding,
            };
            let offset = if encoding == SourceEncoding::Utf8 {
                source.start.saturating_add(relative)
            } else {
                relative
            };
            (
                span.map_error(raw, Error::new(ErrorKind::InvalidState, offset))
                    .offset,
                None,
            )
        }
        FragmentOrigin::Replacement { .. } => (
            origin.physical_context().start,
            Some(
                origin
                    .replacement_range()
                    .expect("replacement range")
                    .start
                    .saturating_add(relative),
            ),
        ),
        FragmentOrigin::Mixed { .. } => (origin.physical_context().start, None),
    };
    ControllerError {
        kind,
        physical_offset,
        origin: Some(origin),
        local_offset,
        value_location: None,
    }
}

#[cfg(test)]
#[path = "../tests/internal/dtd_controller_author.rs"]
mod tests;

#[cfg(test)]
#[path = "../tests/internal/dtd_controller_spec.rs"]
mod spec_tests;
