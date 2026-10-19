use std::borrow::Cow;
use std::io;

use super::*;
use crate::Attribute;
use crate::dtd::DtdLimits;
use crate::dtd_catalog::ValueLocation;
use crate::dtd_writer::DtdWriteLimit;

fn unit(raw: &'static str) -> DtdUnit<'static> {
    DtdUnit::parse(Cow::Borrowed(raw), DtdLimits::default()).unwrap()
}

fn empty<'a>(namespace: Option<&'a str>, attributes: Vec<Attribute<'a>>) -> Event<'a> {
    Event::Empty(Element {
        name: Name::new("r", namespace.map(Cow::Borrowed)).unwrap(),
        attributes,
    })
}

fn write_subset(writer: &mut Writer<Vec<u8>>, declaration: &'static str) {
    writer
        .write_dtd_event(&TakenDtdEvent::Begin(unit("<!DOCTYPE r [")))
        .unwrap();
    writer
        .write_dtd_event(&TakenDtdEvent::Item {
            unit: unit(declaration),
            literals: LiteralSource::Physical,
        })
        .unwrap();
    writer.write_dtd_event(&TakenDtdEvent::End).unwrap();
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
        .write_event(&Event::Empty(Element {
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

#[test]
fn late_identifier_failure_writes_no_part_of_the_item_and_poison_is_sticky() {
    let header = unit("<!DOCTYPE r [");
    let item = unit("<!ENTITY e SYSTEM 'a\rb'>");
    let mut writer = Writer::new(Vec::new());
    writer
        .write_dtd_event(&TakenDtdEvent::Begin(header))
        .unwrap();
    let before = writer.sink.clone();
    let start = writer.position;
    let prefix = item.raw().find('\r').unwrap();
    let DtdEventWriteError::Dtd(error) = writer
        .write_dtd_event(&TakenDtdEvent::Item {
            unit: item,
            literals: LiteralSource::Replacement,
        })
        .unwrap_err()
    else {
        panic!("expected identifier error");
    };
    assert_eq!(error.kind, DtdWriteErrorKind::UnrepresentableIdentifier);
    assert_eq!(error.output_offset, start + prefix);
    assert_eq!(error.unit_offset, Some(prefix));
    assert_eq!(writer.sink, before);
    assert!(matches!(
        writer.write_dtd_event(&TakenDtdEvent::End),
        Err(DtdEventWriteError::Writer(WriteError::Xml(Error {
            kind: ErrorKind::InvalidState,
            ..
        })))
    ));
}

#[test]
fn open_subset_rejects_ordinary_events_and_finish_without_writing_more() {
    let mut event_writer = Writer::new(Vec::new());
    event_writer
        .write_dtd_event(&TakenDtdEvent::Begin(unit("<!DOCTYPE r [")))
        .unwrap();
    let before = event_writer.sink.clone();
    assert!(matches!(
        event_writer.write_event(&empty(None, Vec::new())),
        Err(WriteError::Xml(Error {
            kind: ErrorKind::Syntax,
            offset,
        })) if offset == before.len()
    ));
    assert_eq!(event_writer.sink, before);

    let mut finish_writer = Writer::new(Vec::new());
    finish_writer
        .write_dtd_event(&TakenDtdEvent::Begin(unit("<!DOCTYPE r [")))
        .unwrap();
    let before = finish_writer.sink.clone();
    assert!(matches!(
        finish_writer.finish(),
        Err(WriteError::Xml(Error {
            kind: ErrorKind::UnexpectedEof,
            offset,
        })) if offset == before.len()
    ));
    assert_eq!(finish_writer.sink, before);
}

#[test]
fn sparse_and_expanded_defaults_share_namespace_semantics_but_output_policy_differs() {
    const XMLNS: &str = "http://www.w3.org/2000/xmlns/";
    const DECLARATION: &str = "<!ATTLIST r xmlns CDATA 'urn:u' a CDATA 'x'>";

    let mut sparse = Writer::new(Vec::new());
    write_subset(&mut sparse, DECLARATION);
    sparse
        .write_event(&empty(Some("urn:u"), Vec::new()))
        .unwrap();
    sparse.finish().unwrap();
    assert_eq!(
        sparse.into_inner(),
        b"<!DOCTYPE r [<!ATTLIST r xmlns CDATA 'urn:u' a CDATA 'x'>]><r/>"
    );

    let mut expanded = Writer::new(Vec::new());
    write_subset(&mut expanded, DECLARATION);
    let work_before_body = expanded.dtd[0].counters().work;
    expanded
        .write_event(&empty(
            Some("urn:u"),
            vec![
                Attribute {
                    name: Name::new("xmlns", Some(Cow::Borrowed(XMLNS))).unwrap(),
                    value: Cow::Borrowed("urn:u"),
                },
                Attribute {
                    name: Name::new("a", None).unwrap(),
                    value: Cow::Borrowed("x"),
                },
            ],
        ))
        .unwrap();
    assert!(expanded.dtd[0].counters().work > work_before_body);
    expanded.finish().unwrap();
    assert_eq!(
        expanded.into_inner(),
        b"<!DOCTYPE r [<!ATTLIST r xmlns CDATA 'urn:u' a CDATA 'x'>]><r xmlns=\"urn:u\" a=\"x\"/>"
    );

    let mut inconsistent = Writer::new(Vec::new());
    write_subset(&mut inconsistent, DECLARATION);
    let start = inconsistent.position;
    let WriteError::Xml(public_error) = inconsistent
        .write_event(&empty(None, Vec::new()))
        .unwrap_err()
    else {
        panic!("expected XML error");
    };
    assert_eq!(public_error.kind, ErrorKind::Namespace);
    let error = inconsistent.dtd[0].failure().unwrap();
    assert_eq!(error.kind, DtdWriteErrorKind::Parser(ErrorKind::Namespace));
    assert_eq!(error.output_offset, start);
    assert_eq!(inconsistent.position, start);
}

#[test]
fn tokenized_value_mismatch_reports_syntax_after_dtd_normalization() {
    let mut writer = Writer::new(Vec::new());
    write_subset(&mut writer, "<!ATTLIST r a NMTOKENS #IMPLIED>");
    let start = writer.position;
    let event = empty(
        None,
        vec![Attribute {
            name: Name::new("a", None).unwrap(),
            value: Cow::Borrowed(" x "),
        }],
    );
    assert!(matches!(
        writer.write_event(&event),
        Err(WriteError::Xml(Error {
            kind: ErrorKind::Syntax,
            offset,
        })) if offset == start
    ));
    assert_eq!(
        writer.dtd[0].failure().unwrap().kind,
        DtdWriteErrorKind::Parser(ErrorKind::Syntax)
    );
    assert_eq!(writer.position, start);
}

#[test]
fn applied_unicode_default_qname_keeps_its_raw_declaration_offset() {
    let mut writer = Writer::new(Vec::new());
    write_subset(&mut writer, "<!ATTLIST 根 π:a:b CDATA 'v'>");
    let start = writer.position;
    let event = Event::Empty(Element {
        name: Name::new("根", None).unwrap(),
        attributes: Vec::new(),
    });
    assert!(matches!(
        writer.write_event(&event),
        Err(WriteError::Xml(Error {
            kind: ErrorKind::InvalidName,
            offset,
        })) if offset == start
    ));
    let error = writer.dtd[0].failure().unwrap();
    assert_eq!(
        error.kind,
        DtdWriteErrorKind::Parser(ErrorKind::InvalidName)
    );
    assert_eq!(error.output_offset, start);
    assert_eq!(
        error.value_location,
        Some(ValueLocation::Declaration {
            declaration: 1,
            offset: 18,
        })
    );
}

#[test]
fn cheap_unit_bounds_reject_before_the_item_replaces_the_staging_buffer() {
    let header = unit("<!DOCTYPE r [");
    let item = unit("<!ELEMENT r (a,b,c,d,e)>");
    let mut limits = DtdWriteLimits::default();
    limits.unit.max_unit_bytes = header.raw().len();
    let mut writer = Writer::new(Vec::new());
    writer
        .write_dtd_event_with_limits(&TakenDtdEvent::Begin(header), limits)
        .unwrap();
    let before_output = writer.sink.clone();
    let before_buffer = writer.buffer.text.clone();
    let DtdEventWriteError::Dtd(error) = writer
        .write_dtd_event(&TakenDtdEvent::Item {
            unit: item,
            literals: LiteralSource::Physical,
        })
        .unwrap_err()
    else {
        panic!("expected unit byte limit");
    };
    assert_eq!(
        error.kind,
        DtdWriteErrorKind::Unit(crate::dtd::DtdErrorKind::LimitExceeded(
            crate::dtd::DtdLimit::UnitBytes
        ))
    );
    assert_eq!(writer.sink, before_output);
    assert_eq!(writer.buffer.text, before_buffer);
}

#[test]
fn body_normalization_consumes_the_same_global_work_ceiling() {
    let header = unit("<!DOCTYPE r>");
    let mut limits = DtdWriteLimits::default();
    limits.max_work = header.limit_inspection_work(limits.unit).unwrap() + header.raw().len();
    let mut writer = Writer::new(Vec::new());
    writer
        .write_dtd_event_with_limits(&TakenDtdEvent::Begin(header), limits)
        .unwrap();
    writer.write_dtd_event(&TakenDtdEvent::End).unwrap();
    let start = writer.position;
    let event = empty(
        None,
        vec![Attribute {
            name: Name::new("a", None).unwrap(),
            value: Cow::Borrowed("x"),
        }],
    );
    assert!(matches!(
        writer.write_event(&event),
        Err(WriteError::Xml(Error {
            kind: ErrorKind::Syntax,
            ..
        }))
    ));
    let error = writer.dtd[0].failure().unwrap();
    assert_eq!(error.kind, DtdWriteErrorKind::Limit(DtdWriteLimit::Work));
    assert_eq!(writer.dtd[0].failure(), Some(error));
    assert_eq!(writer.position, start);
}

#[derive(Default)]
struct ShortThenError {
    bytes: Vec<u8>,
    calls: usize,
}

impl io::Write for ShortThenError {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.calls += 1;
        if self.calls == 1 {
            let accepted = bytes.len().min(4);
            self.bytes.extend_from_slice(&bytes[..accepted]);
            Ok(accepted)
        } else {
            Err(io::Error::other("injected"))
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn dtd_events_use_the_existing_partial_sink_and_poison_contract() {
    let mut writer = Writer::new(ShortThenError::default());
    let error = writer
        .write_dtd_event(&TakenDtdEvent::Begin(unit("<!DOCTYPE r>")))
        .unwrap_err();
    assert!(matches!(
        error,
        DtdEventWriteError::Writer(WriteError::Io(ref error))
            if error.kind() == io::ErrorKind::Other
    ));
    assert_eq!(writer.position(), 4);
    assert_eq!(writer.sink.bytes, b"<!DO");
    assert!(matches!(
        writer.write_dtd_event(&TakenDtdEvent::End),
        Err(DtdEventWriteError::Writer(WriteError::Xml(Error {
            kind: ErrorKind::InvalidState,
            ..
        })))
    ));
}
