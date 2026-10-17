//! Private Writer-side DTD state. Public Writer activation remains disabled.

use crate::dtd::{DtdErrorKind, DtdLimit, DtdLimits, DtdUnit, DtdUnitView, LiteralSource};
use crate::dtd_catalog::{
    CatalogErrorKind, CatalogLimit, CatalogLimits, DeclarationContext, DependencyKind, DtdCatalog,
    ValueLocation,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DtdWriteLimits {
    pub(crate) unit: DtdLimits,
    pub(crate) catalog: CatalogLimits,
    pub(crate) max_encoded_bytes: usize,
    pub(crate) max_events: usize,
    pub(crate) max_items: usize,
    pub(crate) max_work: usize,
}

impl Default for DtdWriteLimits {
    fn default() -> Self {
        Self {
            unit: DtdLimits::default(),
            catalog: CatalogLimits::default(),
            max_encoded_bytes: 16 * 1024 * 1024,
            max_events: 16_386,
            max_items: 16_384,
            max_work: 64 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct DtdWriteCounters {
    pub(crate) encoded_bytes: usize,
    pub(crate) events: usize,
    pub(crate) items: usize,
    pub(crate) work: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DtdWriteLimit {
    EncodedBytes,
    Events,
    Items,
    Work,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DtdWriteErrorKind {
    Unit(DtdErrorKind),
    Catalog(CatalogErrorKind),
    Unavailable(DependencyKind),
    Limit(DtdWriteLimit),
    InvalidUnitFamily,
    InvalidSequence,
}

/// Catalog locations can refer to an earlier declaration or replacement. The
/// output anchor therefore remains the current event start unless the encoder
/// separately proves a raw-to-encoded mapping for the current unit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DtdWriteError {
    pub(crate) kind: DtdWriteErrorKind,
    pub(crate) output_offset: usize,
    pub(crate) unit_offset: Option<usize>,
    pub(crate) value_location: Option<ValueLocation>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DtdWritePhase {
    Open { has_internal_subset: bool },
    Complete,
}

pub(crate) struct DtdWriterState {
    catalog: DtdCatalog,
    limits: DtdWriteLimits,
    counters: DtdWriteCounters,
    phase: DtdWritePhase,
}

impl DtdWriterState {
    pub(crate) fn begin(
        unit: &DtdUnit<'_>,
        encoded_bytes: usize,
        output_offset: usize,
        standalone: bool,
        limits: DtdWriteLimits,
    ) -> Result<Self, DtdWriteError> {
        let DtdUnitView::Header(header) = unit.view() else {
            return Err(simple_error(
                DtdWriteErrorKind::InvalidUnitFamily,
                output_offset,
            ));
        };
        check_count(1, limits.max_events, DtdWriteLimit::Events, output_offset)?;
        check_count(
            encoded_bytes,
            limits.max_encoded_bytes,
            DtdWriteLimit::EncodedBytes,
            output_offset,
        )?;
        let inspection = unit
            .limit_inspection_work(limits.unit)
            .map_err(|error| unit_error(error.kind, error.offset, output_offset))?;
        check_count(
            inspection,
            limits.max_work,
            DtdWriteLimit::Work,
            output_offset,
        )?;
        unit.check_inspected_limits(limits.unit)
            .map_err(|error| unit_error(error.kind, error.offset, output_offset))?;

        let mut state = Self {
            catalog: DtdCatalog::new(limits.catalog, standalone),
            limits,
            counters: DtdWriteCounters {
                work: inspection,
                ..DtdWriteCounters::default()
            },
            phase: DtdWritePhase::Open {
                has_internal_subset: header.has_internal_subset(),
            },
        };
        state.apply(unit, LiteralSource::Physical, output_offset)?;
        state.counters.encoded_bytes = encoded_bytes;
        state.counters.events = 1;
        Ok(state)
    }

    pub(crate) fn item(
        &mut self,
        unit: &DtdUnit<'_>,
        literals: LiteralSource,
        encoded_bytes: usize,
        output_offset: usize,
    ) -> Result<(), DtdWriteError> {
        if !matches!(
            self.phase,
            DtdWritePhase::Open {
                has_internal_subset: true
            }
        ) {
            return Err(simple_error(
                DtdWriteErrorKind::InvalidSequence,
                output_offset,
            ));
        }
        if matches!(unit.view(), DtdUnitView::Header(_)) {
            return Err(simple_error(
                DtdWriteErrorKind::InvalidUnitFamily,
                output_offset,
            ));
        }
        let next_events = checked_count(
            self.counters.events,
            1,
            self.limits.max_events,
            DtdWriteLimit::Events,
            output_offset,
        )?;
        let next_items = checked_count(
            self.counters.items,
            1,
            self.limits.max_items,
            DtdWriteLimit::Items,
            output_offset,
        )?;
        let next_encoded = checked_count(
            self.counters.encoded_bytes,
            encoded_bytes,
            self.limits.max_encoded_bytes,
            DtdWriteLimit::EncodedBytes,
            output_offset,
        )?;
        let inspection = unit
            .limit_inspection_work(self.limits.unit)
            .map_err(|error| unit_error(error.kind, error.offset, output_offset))?;
        self.charge_work(inspection, output_offset)?;
        unit.check_inspected_limits(self.limits.unit)
            .map_err(|error| unit_error(error.kind, error.offset, output_offset))?;
        self.apply(unit, literals, output_offset)?;
        self.counters.events = next_events;
        self.counters.items = next_items;
        self.counters.encoded_bytes = next_encoded;
        Ok(())
    }

    pub(crate) fn end(
        &mut self,
        encoded_bytes: usize,
        output_offset: usize,
    ) -> Result<(), DtdWriteError> {
        if !matches!(self.phase, DtdWritePhase::Open { .. }) {
            return Err(simple_error(
                DtdWriteErrorKind::InvalidSequence,
                output_offset,
            ));
        }
        let next_events = checked_count(
            self.counters.events,
            1,
            self.limits.max_events,
            DtdWriteLimit::Events,
            output_offset,
        )?;
        let next_encoded = checked_count(
            self.counters.encoded_bytes,
            encoded_bytes,
            self.limits.max_encoded_bytes,
            DtdWriteLimit::EncodedBytes,
            output_offset,
        )?;
        self.counters.events = next_events;
        self.counters.encoded_bytes = next_encoded;
        self.phase = DtdWritePhase::Complete;
        Ok(())
    }

    pub(crate) fn is_open(&self) -> bool {
        matches!(self.phase, DtdWritePhase::Open { .. })
    }

    pub(crate) fn is_complete(&self) -> bool {
        matches!(self.phase, DtdWritePhase::Complete)
    }

    pub(crate) fn catalog_mut(&mut self) -> Option<&mut DtdCatalog> {
        self.is_complete().then_some(&mut self.catalog)
    }

    pub(crate) fn counters(&self) -> DtdWriteCounters {
        self.counters
    }

    fn charge_work(
        &mut self,
        additional: usize,
        output_offset: usize,
    ) -> Result<(), DtdWriteError> {
        self.counters.work = checked_count(
            self.counters.work,
            additional,
            self.limits.max_work,
            DtdWriteLimit::Work,
            output_offset,
        )?;
        Ok(())
    }

    fn apply(
        &mut self,
        unit: &DtdUnit<'_>,
        literals: LiteralSource,
        output_offset: usize,
    ) -> Result<(), DtdWriteError> {
        let before = self.catalog.usage().work;
        let remaining = self
            .limits
            .max_work
            .checked_sub(self.counters.work)
            .ok_or_else(|| {
                simple_error(DtdWriteErrorKind::Limit(DtdWriteLimit::Work), output_offset)
            })?;
        let global_ceiling = before.checked_add(remaining).ok_or_else(|| {
            simple_error(DtdWriteErrorKind::Limit(DtdWriteLimit::Work), output_offset)
        })?;
        self.catalog.cap_work(global_ceiling);
        let result = self.catalog.apply(
            unit,
            DeclarationContext {
                internal_subset: true,
                literals,
            },
        );
        let catalog_work = self.catalog.usage().work - before;
        let next_work = checked_count(
            self.counters.work,
            catalog_work,
            self.limits.max_work,
            DtdWriteLimit::Work,
            output_offset,
        )?;
        self.counters.work = next_work;
        let effect = result.map_err(|error| {
            let kind = if error.kind == CatalogErrorKind::Limit(CatalogLimit::Work)
                && global_ceiling <= self.limits.catalog.max_work
            {
                DtdWriteErrorKind::Limit(DtdWriteLimit::Work)
            } else {
                DtdWriteErrorKind::Catalog(error.kind)
            };
            DtdWriteError {
                kind,
                output_offset,
                unit_offset: None,
                value_location: Some(error.location),
            }
        })?;
        if let Some(dependency) = effect.unavailable {
            return Err(DtdWriteError {
                kind: DtdWriteErrorKind::Unavailable(dependency.kind),
                output_offset,
                unit_offset: None,
                value_location: Some(dependency.location),
            });
        }
        Ok(())
    }
}

fn check_count(
    value: usize,
    ceiling: usize,
    limit: DtdWriteLimit,
    output_offset: usize,
) -> Result<(), DtdWriteError> {
    if value > ceiling {
        Err(simple_error(DtdWriteErrorKind::Limit(limit), output_offset))
    } else {
        Ok(())
    }
}

fn checked_count(
    current: usize,
    additional: usize,
    ceiling: usize,
    limit: DtdWriteLimit,
    output_offset: usize,
) -> Result<usize, DtdWriteError> {
    current
        .checked_add(additional)
        .filter(|next| *next <= ceiling)
        .ok_or_else(|| simple_error(DtdWriteErrorKind::Limit(limit), output_offset))
}

fn unit_error(kind: DtdErrorKind, unit_offset: usize, output_offset: usize) -> DtdWriteError {
    DtdWriteError {
        kind: DtdWriteErrorKind::Unit(kind),
        output_offset,
        unit_offset: Some(unit_offset),
        value_location: None,
    }
}

fn simple_error(kind: DtdWriteErrorKind, output_offset: usize) -> DtdWriteError {
    DtdWriteError {
        kind,
        output_offset,
        unit_offset: None,
        value_location: None,
    }
}

#[cfg(test)]
#[path = "../tests/internal/dtd_writer_author.rs"]
mod tests;
