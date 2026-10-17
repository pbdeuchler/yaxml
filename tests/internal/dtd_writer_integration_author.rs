use std::borrow::Cow;

use super::*;
use crate::dtd::DtdLimits;

fn unit(raw: &'static str) -> DtdUnit<'static> {
    DtdUnit::parse(Cow::Borrowed(raw), DtdLimits::default()).unwrap()
}

#[test]
fn one_catalog_serializes_subset_and_validates_sparse_body_tag() {
    let header = unit("<!DOCTYPE r [");
    let declaration = unit("<!ATTLIST r a CDATA 'x'>");
    let mut writer = Writer::new(Vec::new());
    assert_eq!(writer.dtd.capacity(), 0);

    writer
        .write_dtd_event(&TakenDtdEvent::Begin(header))
        .unwrap();
    writer
        .write_dtd_event(&TakenDtdEvent::Item {
            unit: declaration,
            literals: LiteralSource::Physical,
        })
        .unwrap();
    writer.write_dtd_event(&TakenDtdEvent::End).unwrap();
    assert_eq!(writer.dtd.len(), 1);
    assert!(writer.dtd[0].is_complete());
    assert_eq!(writer.dtd[0].catalog().unwrap().attributes("r").count(), 1);

    writer
        .write_dtd_body_event(&Event::Empty(Element {
            name: Name::new("r", None).unwrap(),
            attributes: Vec::new(),
        }))
        .unwrap();
    writer.finish().unwrap();
    assert_eq!(
        writer.into_inner(),
        b"<!DOCTYPE r [<!ATTLIST r a CDATA 'x'>]><r/>"
    );
}

#[test]
fn no_subset_delimiter_is_held_until_end() {
    let mut writer = Writer::new(Vec::new());
    writer
        .write_dtd_event(&TakenDtdEvent::Begin(unit("<!DOCTYPE r>")))
        .unwrap();
    assert_eq!(writer.sink, b"<!DOCTYPE r");
    writer.write_dtd_event(&TakenDtdEvent::End).unwrap();
    assert_eq!(writer.sink, b"<!DOCTYPE r>");
}

#[test]
fn protocol_and_outer_writer_failures_remain_distinct() {
    let mut protocol = Writer::new(Vec::new());
    let DtdEventWriteError::Dtd(error) = protocol.write_dtd_event(&TakenDtdEvent::End).unwrap_err()
    else {
        panic!("missing DTD protocol error");
    };
    assert_eq!(error.kind, DtdWriteErrorKind::InvalidSequence);

    let mut lifecycle = Writer::new(Vec::new());
    assert!(lifecycle.finish().is_err());
    let DtdEventWriteError::Writer(WriteError::Xml(error)) =
        lifecycle.write_dtd_event(&TakenDtdEvent::End).unwrap_err()
    else {
        panic!("missing outer Writer error");
    };
    assert_eq!(error.kind, ErrorKind::InvalidState);
}
