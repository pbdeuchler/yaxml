use std::borrow::Cow;

use super::*;

fn unit(raw: &str) -> DtdUnit<'_> {
    DtdUnit::parse(Cow::Borrowed(raw), DtdLimits::default()).unwrap()
}

#[test]
fn state_applies_one_catalog_from_begin_through_complete() {
    let header = unit("<!DOCTYPE r [");
    let declaration = unit("<!ATTLIST r a CDATA 'x'>");
    let mut state = DtdWriterState::begin(
        &header,
        header.raw().len(),
        7,
        false,
        DtdWriteLimits::default(),
    )
    .unwrap();
    assert!(state.is_open());
    state
        .item(
            &declaration,
            LiteralSource::Physical,
            declaration.raw().len(),
            20,
        )
        .unwrap();
    state.end(2, 50).unwrap();
    assert!(state.is_complete());
    assert_eq!(state.catalog_mut().unwrap().attributes("r").count(), 1);
    assert_eq!(
        state.counters(),
        DtdWriteCounters {
            encoded_bytes: header.raw().len() + declaration.raw().len() + 2,
            events: 3,
            items: 1,
            // One header token + four ATTLIST tokens, 37 raw declaration
            // bytes, and the one-byte default-value normalization.
            work: 43,
        }
    );
}

#[test]
fn family_sequence_and_native_limits_are_typed_before_catalog_use() {
    let item = unit("<!ELEMENT r EMPTY>");
    assert_eq!(
        DtdWriterState::begin(
            &item,
            item.raw().len(),
            11,
            false,
            DtdWriteLimits::default()
        )
        .err()
        .unwrap(),
        DtdWriteError {
            kind: DtdWriteErrorKind::InvalidUnitFamily,
            output_offset: 11,
            unit_offset: None,
            value_location: None,
        }
    );

    let header = unit("<!DOCTYPE r [");
    let mut limits = DtdWriteLimits::default();
    limits.unit.max_tokens = 0;
    let error = DtdWriterState::begin(&header, header.raw().len(), 13, false, limits)
        .err()
        .unwrap();
    assert_eq!(
        error.kind,
        DtdWriteErrorKind::Unit(DtdErrorKind::LimitExceeded(DtdLimit::Tokens))
    );
    assert_eq!(error.output_offset, 13);
    assert_eq!(error.unit_offset, Some(10));
}

#[test]
fn aggregate_work_is_distinct_from_native_catalog_work() {
    let header = unit("<!DOCTYPE r [");
    let mut aggregate = DtdWriteLimits::default();
    aggregate.max_work = header.limit_inspection_work(aggregate.unit).unwrap();
    assert_eq!(
        DtdWriterState::begin(&header, header.raw().len(), 17, false, aggregate)
            .err()
            .unwrap()
            .kind,
        DtdWriteErrorKind::Limit(DtdWriteLimit::Work)
    );

    let mut native = DtdWriteLimits::default();
    native.catalog.max_work = 0;
    assert_eq!(
        DtdWriterState::begin(&header, header.raw().len(), 19, false, native)
            .err()
            .unwrap()
            .kind,
        DtdWriteErrorKind::Catalog(CatalogErrorKind::Limit(CatalogLimit::Work))
    );
}
