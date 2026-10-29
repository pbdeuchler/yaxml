//! `RECOVER` on the streams, and the namespace errors strict parsing keeps.
//! For every chunking, push and pull deliver the same events, ranges and
//! recovered errors; on decodable input the slice reader's over its
//! transcoded text, which project onto the recovered DOM.

use std::io::BufReader;
use std::ops::{ControlFlow, Range};

use yaxml::options::{NOBLANKS, RECOVER};
use yaxml::{
    BufferedReader, Document, Error, ErrorKind, Event, ParseOptions, PushParser, Reader, Transcoded,
};

/// Events with their ranges, then the recovered errors; or the fatal error.
type Stream = Result<(Vec<String>, Vec<Error>), ErrorKind>;

/// An event as text, its attribute list however it is lent.
pub(super) fn render(event: &Event<'_>, range: Range<usize>) -> String {
    match event {
        Event::Start(element) | Event::Empty(element) => format!(
            "{} {:?} {:?} {range:?}",
            matches!(event, Event::Empty(_)),
            element.name,
            &element.attributes[..]
        ),
        event => format!("{event:?} {range:?}"),
    }
}

fn options(flags: u32) -> ParseOptions {
    ParseOptions::new(flags)
}

/// The push parser fed `input` in chunks of `widths` (then the rest).
fn pushed(input: &[u8], flags: u32, widths: &[usize]) -> Stream {
    let mut parser = PushParser::with_options(options(flags)).unwrap();
    let mut events = Vec::new();
    let mut emit = |event: &Event<'_>, origin| {
        events.push(render(event, super::span(origin)));
        ControlFlow::Continue(())
    };
    let mut rest = input;
    for &width in widths.iter().chain(std::iter::once(&input.len())) {
        let (chunk, after) = rest.split_at(width.min(rest.len()));
        parser.feed(chunk, &mut emit).map_err(|e| e.kind)?;
        rest = after;
    }
    parser.finish(&mut emit).map_err(|e| e.kind)?;
    Ok((events, parser.errors().to_vec()))
}

/// The buffered reader over a source of `capacity` bytes at a time.
fn pulled(input: &[u8], flags: u32, capacity: usize) -> Stream {
    let source = BufReader::with_capacity(capacity, input);
    let mut reader = BufferedReader::with_options(source, options(flags)).unwrap();
    let mut events = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Some((event, origin))) => events.push(render(&event, super::span(origin))),
            Ok(None) => break,
            Err(yaxml::ReadError::Xml(error)) => return Err(error.kind),
            Err(error) => panic!("in-memory source failed: {error}"),
        }
    }
    assert!(matches!(reader.read_event(), Ok(None)), "EOF is terminal");
    Ok((events, reader.errors().to_vec()))
}

/// The slice reader's stream over transcoded `input`, offsets in the input,
/// and its events for the DOM projection; `None` when it rejects the input
/// up front.
fn sliced<'t>(input: &'t Transcoded<'_>, flags: u32) -> Option<(Stream, Vec<Event<'t>>)> {
    let mut reader = Reader::transcoded(input, options(flags)).ok()?;
    let (mut lines, mut events) = (Vec::new(), Vec::new());
    loop {
        match reader.read_event() {
            Ok(Some(event)) => {
                let range = super::span(reader.event_origin().unwrap());
                lines.push(render(
                    &event,
                    input.source_offset(range.start)..input.source_offset(range.end),
                ));
                events.push(event);
            }
            Ok(None) => return Some((Ok((lines, sourced(input, reader.errors()))), events)),
            Err(error) => return Some((Err(error.kind), events)),
        }
    }
}

/// `errors` at their input offsets.
pub(super) fn sourced(input: &Transcoded<'_>, errors: &[Error]) -> Vec<Error> {
    let source = |e: &Error| Error {
        offset: input.source_offset(e.offset),
        ..*e
    };
    errors.iter().map(source).collect()
}

/// `input` transcoded, when the slice reader reads it as the streams do:
/// decodable (the streams drop text pending at a decoding error recovery
/// stops at), and without characters a DTD stack meets (the slice reader's
/// stops at the first of the whole input; a stream's at the token holding it).
pub(super) fn comparable(input: &[u8], flags: u32) -> Option<Transcoded<'_>> {
    Transcoded::new(input, &options(flags & !RECOVER)).ok()?;
    let text = Transcoded::new(input, &options(flags)).ok()?;
    let characters = std::str::from_utf8(text.as_bytes()).map_or(true, |text| {
        text.chars().any(|c| !yaxml::chars::is_char(u32::from(c)))
    });
    let doctype = text.as_bytes().windows(9).any(|w| w == b"<!DOCTYPE");
    (!(doctype && characters)).then_some(text)
}

/// The streams on `input` under `flags` (`RECOVER` or strict), chunked as
/// `control` selects.
pub(super) fn exercise(input: &[u8], flags: u32, control: u8) {
    let width = 1 + usize::from(control % 13);
    let widths: Vec<usize> = (0..input.len().div_ceil(width)).map(|_| width).collect();
    let stream = pushed(input, flags, &widths);
    // Owned names can exceed the owned-bytes budget where the slice borrows.
    let owned = matches!(
        stream,
        Err(ErrorKind::LimitExceeded(yaxml::Limit::OwnedBytes))
    );
    assert_eq!(
        stream,
        pushed(input, flags, &[]),
        "chunking changed recovery"
    );
    let capacity = [1, 3, 7, 64, 8192][usize::from(control >> 4) % 5];
    assert_eq!(
        stream,
        pulled(input, flags, capacity),
        "push/pull recovery differ"
    );
    if let Ok((events, errors)) = &stream {
        assert!(events.len() <= super::event_bound(input.len()));
        assert!(
            errors.len() <= 103,
            "100 + the first error clearing each property bit"
        );
        for error in errors {
            assert!(error.offset <= input.len(), "{error:?} in {input:?}");
            assert!(!matches!(
                error.kind,
                ErrorKind::LimitExceeded(_) | ErrorKind::Allocation | ErrorKind::EntityLoop
            ));
            assert!(
                flags & RECOVER != 0
                    || matches!(error.kind, ErrorKind::Namespace | ErrorKind::Warning(_))
            );
        }
    }
    let Some(text) = comparable(input, flags).filter(|_| !owned) else {
        return;
    };
    let Some((slice, events)) = sliced(&text, flags) else {
        return;
    };
    assert_eq!(stream, slice, "stream/slice recovery differ on {input:?}");
    if slice.is_ok()
        && flags & NOBLANKS == 0
        && let Ok(document) = Document::parse_transcoded(&text, options(flags))
    {
        assert_eq!(
            sourced(&text, document.errors()),
            slice.as_ref().unwrap().1,
            "DOM errors"
        );
        super::check_projection(&document, &events);
    }
}
