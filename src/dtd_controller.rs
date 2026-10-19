//! Private internal-subset composition. No public XML parser enables this yet.

use crate::dtd::{DtdLimits, DtdUnit, DtdUnitView};
use crate::dtd_catalog::{
    CatalogErrorKind, DeclarationContext, DtdCatalog, EntityId, EntityLookup, LiteralSource,
    ValueLocation,
};
use crate::dtd_frames::{
    ByteSpan, EntityDeclarationId, EntityFrames, EntityKind, EntityResolution, FragmentOrigin,
    FrameBoundary, FrameErrorKind, LookupFailure,
};
use crate::dtd_scan::{DtdScanner, ScanErrorKind, ScanKind, ScanMode};
use crate::encoding::{SourceEncoding, TokenSpan};
use crate::parser::Budget;
use crate::{Error, ErrorKind};

pub(crate) mod content;

/// libxml2's `XML_ENT_FIXED_COST`: charged per expansion so that empty or
/// tiny entities cannot be referenced without bound.
pub(crate) const ENTITY_FIXED_COST: usize = 20;

/// Splits the shared expansion allowance between the two expanding components.
pub(crate) fn share_expansion(
    limits: &DtdLimits,
    consumed: usize,
    frames: &mut EntityFrames,
    catalog: &mut DtdCatalog,
) {
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
pub(crate) enum ControllerErrorKind {
    Scan(ScanErrorKind),
    Catalog(CatalogErrorKind),
    Frame(FrameErrorKind),
    Ownership(ErrorKind),
    Limit(ControllerLimit),
    PositionOverflow,
    InvalidUnitFamily,
    InvalidState,
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
pub(crate) enum DtdEvent {
    Unit,
    End,
}

#[derive(Clone, Copy)]
enum Ready {
    Begin,
    Item,
    End,
}
#[derive(Clone, Copy)]
enum Side {
    Physical,
    Replacement,
}

#[derive(Clone, Copy)]
enum PhysicalInput<'input> {
    Streaming,
    StableUtf8 {
        source: &'input str,
        source_start: usize,
        source_end: usize,
    },
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
    active: Vec<EntityId>,
    input: PhysicalInput<'input>,
    encoding: SourceEncoding,
    start: usize,
    decoded_start: usize,
    position: usize,
    unit_start: usize,
    limits: DtdLimits,
    counters: ControllerCounters,
    unit: Option<DtdUnit<'input>>,
    ready: Option<Ready>,
    origin: Option<FragmentOrigin>,
    held_end: Option<FragmentOrigin>,
    started: bool,
    finalizing: bool,
    complete: bool,
    failure: Option<ControllerError>,
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
            PhysicalInput::Streaming,
        )
    }

    pub(crate) fn stable_utf8<'input>(
        source: &'input str,
        source_start: usize,
        physical_start: usize,
        limits: DtdLimits,
        standalone: bool,
    ) -> Result<BorrowedDtdController<'input>, ControllerError> {
        let source_end = source_start.checked_add(source.len()).ok_or_else(|| {
            standalone_error(ControllerErrorKind::PositionOverflow, physical_start)
        })?;
        let relative = physical_start
            .checked_sub(source_start)
            .ok_or_else(|| standalone_error(ControllerErrorKind::InvalidState, physical_start))?;
        if physical_start > source_end || !source.is_char_boundary(relative) {
            return Err(standalone_error(
                ControllerErrorKind::InvalidState,
                physical_start,
            ));
        }
        Ok(DtdControllerCore::with_input(
            SourceEncoding::Utf8,
            physical_start,
            physical_start,
            limits,
            standalone,
            PhysicalInput::StableUtf8 {
                source,
                source_start,
                source_end,
            },
        ))
    }
}

impl<'input> DtdControllerCore<'input> {
    fn with_input(
        encoding: SourceEncoding,
        physical_start: usize,
        decoded_start: usize,
        limits: DtdLimits,
        standalone: bool,
        input: PhysicalInput<'input>,
    ) -> Self {
        Self {
            physical: DtdScanner::new(ScanMode::Doctype, limits.token, limits.work),
            fragment: DtdScanner::new(ScanMode::SubsetFragment, limits.token, limits.work),
            catalog: DtdCatalog::new(limits, standalone),
            frames: EntityFrames::new(limits),
            active: Vec::new(),
            input,
            encoding,
            start: physical_start,
            decoded_start,
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
    pub(crate) fn is_complete(&self) -> bool {
        self.complete
    }
    pub(crate) fn start(&self) -> usize {
        self.start
    }
    pub(crate) fn current(&self) -> Option<(DtdEvent, FragmentOrigin)> {
        let event = match self.ready? {
            Ready::Begin | Ready::Item => DtdEvent::Unit,
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
        self.finish_release(ready);
    }

    fn finish_release(&mut self, ready: Ready) {
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
            if !matches!(controller.input, PhysicalInput::Streaming) {
                return Err(controller.simple_error(ControllerErrorKind::InvalidState));
            }
            controller.drive(if controller.finalizing { &[] } else { input }, budget)
        })
    }

    pub(crate) fn advance_borrowed(
        &mut self,
        budget: &mut Budget,
    ) -> Result<usize, ControllerError> {
        self.remember(|controller| {
            let PhysicalInput::StableUtf8 {
                source,
                source_start,
                source_end,
            } = controller.input
            else {
                return Err(controller.simple_error(ControllerErrorKind::InvalidState));
            };
            if controller.finalizing {
                return controller.drive(&[], budget);
            }
            let relative = controller
                .position
                .checked_sub(source_start)
                .ok_or_else(|| controller.simple_error(ControllerErrorKind::InvalidState))?;
            let remaining = source
                .get(relative..)
                .ok_or_else(|| controller.simple_error(ControllerErrorKind::InvalidState))?;
            let consumed = controller.drive(remaining.as_bytes(), budget)?;
            if controller.ready.is_none()
                && !controller.complete
                && controller.position == source_end
            {
                controller.finalizing = true;
                controller.drive(&[], budget)?;
            }
            Ok(consumed)
        })
    }

    pub(crate) fn finish_step(&mut self, budget: &mut Budget) -> Result<(), ControllerError> {
        self.remember(|controller| {
            if !matches!(controller.input, PhysicalInput::Streaming) {
                return Err(controller.simple_error(ControllerErrorKind::InvalidState));
            }
            controller.finalizing = true;
            controller.drive(&[], budget).map(|_| ())
        })
    }

    pub(crate) fn set_loader(&mut self, loader: crate::resolve::Loader) {
        self.catalog.set_loader(loader);
    }

    /// Completes the declarations, loading the external subset last as libxml2 does.
    pub(crate) fn into_body_state(mut self) -> Result<DtdBodyState, ControllerError> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        if self.complete
            && let Err(error) = self.catalog.include_subset()
        {
            return Err(self.simple_error(ControllerErrorKind::Catalog(error.kind)));
        }
        if !self.complete
            || self.ready.is_some()
            || self.unit.is_some()
            || self.origin.is_some()
            || self.held_end.is_some()
            || !self.active.is_empty()
            || self.frames.depth() != 0
        {
            return Err(self.simple_error(ControllerErrorKind::InvalidState));
        }
        let decoded_position = self
            .decoded_start
            .checked_add(self.physical.position())
            .ok_or_else(|| self.simple_error(ControllerErrorKind::PositionOverflow))?;
        Ok(DtdBodyState {
            catalog: self.catalog,
            frames: self.frames,
            limits: self.limits,
            counters: self.counters,
            decoded_position,
            physical_position: self.position,
            encoding: self.encoding,
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
                .is_none_or(|count| count > self.limits.declarations);
            let global_remaining = self.limits.work - self.counters.work;
            let scan_remaining = self.limits.work - self.counters.scanned_bytes;
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
        let global_remaining = self.limits.work - self.counters.work;
        let scan_remaining = self.limits.work - self.counters.scanned_bytes;
        let remaining = global_remaining.min(scan_remaining);
        self.fragment
            .cap_work(self.fragment.position().saturating_add(remaining));
        let before = self.fragment.position();
        // The scanner stops at the first complete unit, so whole slices are safe.
        let result = self.fragment.advance(&replacement.as_bytes()[cursor..]);
        let consumed = self.fragment.position() - before;
        self.counters.work += consumed;
        self.counters.scanned_bytes += consumed;
        if let Err(error) = result {
            // A slice can exhaust the cap mid-way; the tighter ceiling names it.
            if error.kind == ScanErrorKind::Limit(crate::dtd_scan::ScanLimit::Work)
                && global_remaining <= scan_remaining
            {
                return Err(self.simple_error(ControllerErrorKind::Limit(ControllerLimit::Work)));
            }
            return Err(self.scan_error(Side::Replacement, error.kind, error.offset));
        }
        Ok(())
    }

    fn process_ready(&mut self, side: Side, budget: &mut Budget) -> Result<(), ControllerError> {
        share_expansion(
            &self.limits,
            self.position,
            &mut self.frames,
            &mut self.catalog,
        );
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
        // Grammar is always checked before an input-mode-dependent raw copy.
        self.charge_work(raw_len, Some(origin))?;
        let stable_raw = if matches!(side, Side::Physical) {
            self.stable_physical_raw(origin)?
        } else {
            None
        };
        let unit = if let Some(raw) = stable_raw {
            let unit = DtdUnit::parse(raw, self.limits).map_err(|error| {
                map_unit_error(
                    self.encoding,
                    origin,
                    raw,
                    error.offset,
                    ControllerErrorKind::Ownership(error.kind),
                )
            })?;
            if !valid_unit(kind, side, self.started, &unit) {
                return Err(self.error_for_origin(
                    ControllerErrorKind::InvalidUnitFamily,
                    origin,
                    None,
                ));
            }
            unit
        } else {
            let raw = match side {
                Side::Physical => self.physical.current(),
                Side::Replacement => self.fragment.current(),
            }
            .expect("ready raw")
            .1;
            let raw = std::str::from_utf8(raw).expect("scanner validates ready UTF-8");
            let borrowed = DtdUnit::parse(raw, self.limits).map_err(|error| {
                map_unit_error(
                    self.encoding,
                    origin,
                    raw,
                    error.offset,
                    ControllerErrorKind::Ownership(error.kind),
                )
            })?;
            if !valid_unit(kind, side, self.started, &borrowed) {
                return Err(self.error_for_origin(
                    ControllerErrorKind::InvalidUnitFamily,
                    origin,
                    None,
                ));
            }
            let next_owned = self
                .counters
                .owned_bytes
                .checked_add(raw_len)
                .filter(|count| *count <= self.limits.storage)
                .ok_or_else(|| {
                    self.error_for_origin(
                        ControllerErrorKind::Limit(ControllerLimit::OwnedBytes),
                        origin,
                        None,
                    )
                })?;
            let next_work = self
                .counters
                .work
                .checked_add(raw_len)
                .filter(|work| *work <= self.limits.work)
                .ok_or_else(|| {
                    self.error_for_origin(
                        ControllerErrorKind::Limit(ControllerLimit::Work),
                        origin,
                        None,
                    )
                })?;
            let owned = borrowed
                .try_into_owned(budget, origin.physical_context().start)
                .map_err(|error| {
                    map_unit_error(
                        self.encoding,
                        origin,
                        raw,
                        0,
                        ControllerErrorKind::Ownership(error.kind),
                    )
                })?;
            self.counters.work = next_work;
            self.counters.owned_bytes = next_owned;
            owned
        };
        let header = matches!(unit.view(), DtdUnitView::Header(_));
        self.commit_source(side, raw_len)?;
        self.release_scanner(side);
        let literals = if matches!(side, Side::Physical) {
            LiteralSource::Physical
        } else {
            LiteralSource::Replacement
        };
        let before = self.catalog.usage().work;
        let ceiling = before.saturating_add(self.limits.work - self.counters.work);
        self.catalog.cap_work(ceiling);
        let result = self.catalog.apply(
            &unit,
            DeclarationContext {
                internal_subset: true,
                literals,
            },
        );
        self.counters.work += self.catalog.usage().work - before;
        result.map_err(|error| {
            let kind = if error.kind
                == CatalogErrorKind::Limit(crate::dtd_catalog::CatalogLimit::Work)
                && ceiling <= self.limits.work
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
            self.ready = Some(Ready::Item);
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
        // libxml2 sets hasPErefs before resolving the name.
        self.catalog.note_parameter_reference();
        let raw_len = match side {
            Side::Physical => self.physical.current(),
            Side::Replacement => self.fragment.current(),
        }
        .map_or(0, |(_, raw, _)| raw.len());
        let (id, resolution) = {
            let raw = match side {
                Side::Physical => self.physical.current(),
                Side::Replacement => self.fragment.current(),
            }
            .expect("parameter raw")
            .1;
            let raw = std::str::from_utf8(raw).expect("validated PE reference");
            let name = &raw[1..raw.len() - 1];
            match self.catalog.lookup_parameter(name) {
                EntityLookup::Internal { id, replacement } => (
                    id,
                    EntityResolution::Internal {
                        kind: EntityKind::Parameter,
                        declaration: EntityDeclarationId::from_index(id.index()),
                        replacement_len: replacement.len(),
                    },
                ),
                // External text is processed whole when the caller's resolver
                // supplies it; otherwise, like libxml2's failed load, it is skipped.
                EntityLookup::ExternalUnavailable { id, .. } => {
                    // A skipped reference still consumes (and pays for) its spelling.
                    self.commit_source(side, raw_len)?;
                    self.release_scanner(side);
                    return self
                        .catalog
                        .include_parameter(id, &self.active)
                        .map_err(|error| {
                            self.error_for_origin(
                                ControllerErrorKind::Catalog(error.kind),
                                origin,
                                None,
                            )
                        });
                }
                // Like libxml2's undeclared validity error, the reference is skipped.
                EntityLookup::UnreadEntity => {
                    let message = format!("Entity '{name}' not defined");
                    let offset = origin.input_range().map(|range| range.start);
                    self.catalog
                        .note(crate::valid::Code::UndeclaredEntity, offset, message);
                    self.commit_source(side, raw_len)?;
                    self.release_scanner(side);
                    return Ok(());
                }
                EntityLookup::Unparsed { .. } => {
                    return Err(self.error_for_origin(
                        ControllerErrorKind::Frame(FrameErrorKind::Lookup(LookupFailure::Unparsed)),
                        origin,
                        None,
                    ));
                }
                EntityLookup::Predefined(_) | EntityLookup::Undeclared => {
                    return Err(self.error_for_origin(
                        ControllerErrorKind::Frame(FrameErrorKind::Lookup(
                            LookupFailure::Undeclared,
                        )),
                        origin,
                        None,
                    ));
                }
            }
        };
        // Keep the ID stack ready before frames publish a child or move its parent.
        // Failed lookups and depth failures never need another active slot.
        if self.active.len() < self.limits.depth {
            self.active.try_reserve(1).map_err(|_| {
                self.error_for_origin(ControllerErrorKind::Allocation, origin, None)
            })?;
        }
        self.release_scanner(side);
        let before = self.frames.counters().work;
        let ceiling = before.saturating_add(self.limits.work - self.counters.work);
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
            return Err(self.error_for_origin(
                if error.kind == FrameErrorKind::LimitExceeded(crate::dtd_frames::FrameLimit::Work)
                    && ceiling <= self.limits.work
                {
                    ControllerErrorKind::Limit(ControllerLimit::Work)
                } else {
                    ControllerErrorKind::Frame(error.kind)
                },
                origin,
                origin.replacement_range().map(|range| range.start),
            ));
        }

        self.active.push(id);
        Ok(())
    }

    fn commit_source(&mut self, side: Side, bytes: usize) -> Result<(), ControllerError> {
        if matches!(side, Side::Physical) {
            return Ok(());
        }
        let id = *self.active.last().expect("active unit source");
        let origin = self.pending_origin(Side::Replacement)?;
        let before = self.frames.counters().work;
        let ceiling = before.saturating_add(self.limits.work - self.counters.work);
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
                && ceiling <= self.limits.work
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

    fn stable_physical_raw(
        &self,
        origin: FragmentOrigin,
    ) -> Result<Option<&'input str>, ControllerError> {
        let PhysicalInput::StableUtf8 {
            source,
            source_start,
            ..
        } = self.input
        else {
            return Ok(None);
        };
        let range = origin.input_range().ok_or_else(|| {
            self.error_for_origin(ControllerErrorKind::InvalidState, origin, None)
        })?;
        let start = range.start.checked_sub(source_start).ok_or_else(|| {
            self.error_for_origin(ControllerErrorKind::InvalidState, origin, None)
        })?;
        let end = range.end.checked_sub(source_start).ok_or_else(|| {
            self.error_for_origin(ControllerErrorKind::InvalidState, origin, None)
        })?;
        source
            .get(start..end)
            .map(Some)
            .ok_or_else(|| self.error_for_origin(ControllerErrorKind::InvalidState, origin, None))
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
            .filter(|work| *work <= self.limits.work)
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

impl DtdBodyState {
    pub(crate) fn resume_position(&self) -> (usize, usize) {
        (self.decoded_position, self.physical_position)
    }
}

fn valid_unit(kind: ScanKind, side: Side, started: bool, unit: &DtdUnit<'_>) -> bool {
    let header = matches!(unit.view(), DtdUnitView::Header(_));
    match kind {
        ScanKind::Header => matches!(side, Side::Physical) && !started && header,
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
        ScanKind::Instruction => {
            matches!(unit.view(), DtdUnitView::ProcessingInstruction(_))
        }
        _ => false,
    }
}

fn standalone_error(kind: ControllerErrorKind, physical_offset: usize) -> ControllerError {
    ControllerError {
        kind,
        physical_offset,
        origin: None,
        local_offset: None,
        value_location: None,
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
