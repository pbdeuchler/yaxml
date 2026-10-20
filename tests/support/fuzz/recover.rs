//! `RECOVER` on the streams. For every chunking, push and pull deliver the
//! same events, ranges and recovered errors; on UTF-8 input the slice
//! reader's, which project onto the recovered DOM.

use std::io::BufReader;
use std::ops::{ControlFlow, Range};

use yaxml::options::{NOBLANKS, RECOVER};
use yaxml::{BufferedReader, Document, Error, ErrorKind, Event, ParseOptions, PushParser, Reader};

/// Events with their ranges, then the recovered errors; or the fatal error.
type Stream = Result<(Vec<String>, Vec<Error>), ErrorKind>;

/// An event as text, its attribute list however it is lent.
fn render(event: &Event<'_>, range: Range<usize>) -> String {
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
    ParseOptions::new(RECOVER | flags)
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

/// The slice reader's stream, and its events for the DOM projection;
/// `None` when it rejects the input up front.
fn sliced(input: &[u8], flags: u32) -> Option<(Stream, Vec<Event<'_>>)> {
    let mut reader = Reader::with_options(input, options(flags)).ok()?;
    let (mut lines, mut events) = (Vec::new(), Vec::new());
    loop {
        match reader.read_event() {
            Ok(Some(event)) => {
                lines.push(render(&event, super::span(reader.event_origin().unwrap())));
                events.push(event);
            }
            Ok(None) => return Some((Ok((lines, reader.errors().to_vec())), events)),
            Err(error) => return Some((Err(error.kind), events)),
        }
    }
}

/// Whether the slice reader reads `input` as the streams do: UTF-8 without
/// a declared charset (which only streams decode), and without characters
/// a DTD stack meets (the slice reader's stops at the first of the whole
/// input; a stream's at the token holding it).
pub(super) fn comparable(input: &[u8]) -> bool {
    let declared = input
        .windows(8)
        .any(|w| w.eq_ignore_ascii_case(b"encoding"));
    let characters = std::str::from_utf8(input).map_or(true, |text| {
        text.chars().any(|c| !yaxml::chars::is_char(u32::from(c)))
    });
    let doctype = input.windows(9).any(|w| w == b"<!DOCTYPE");
    !super::encoding::recognizes(input) && !declared && !(doctype && characters)
}

/// The recovering streams on `input` under `flags`, chunked as `control`
/// selects.
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
        assert!(errors.len() <= 100);
        for error in errors {
            assert!(error.offset <= input.len(), "{error:?} in {input:?}");
            assert!(!matches!(
                error.kind,
                ErrorKind::LimitExceeded(_) | ErrorKind::Allocation | ErrorKind::EntityLoop
            ));
        }
    }
    let Some((slice, events)) = sliced(input, flags).filter(|_| comparable(input) && !owned) else {
        return;
    };
    assert_eq!(stream, slice, "stream/slice recovery differ on {input:?}");
    if slice.is_ok()
        && flags & NOBLANKS == 0
        && let Ok(document) = Document::parse_with(input, options(flags))
    {
        assert_eq!(document.errors(), slice.as_ref().unwrap().1, "DOM errors");
        super::check_projection(&document, &events);
    }
}
