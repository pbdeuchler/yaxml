//! External subsets and entities served from fuzz-provided memory: every
//! surface agrees, requests stay within the load and byte budgets, and
//! repeated parses issue the same requests.

use std::io::{BufReader, Cursor};
use std::ops::ControlFlow;
use std::sync::{Arc, Mutex};

use yaxml::{
    BufferedReader, Document, Error, Limits, OwnedDocument, PushParser, ReadError, Reader, Request,
    Resolver,
};

const LIMITS: Limits = Limits {
    max_input_bytes: 1 << 20,
    max_token_bytes: 1 << 16,
    max_depth: 64,
    max_attributes: 64,
    max_events: 1 << 16,
    max_owned_bytes: 1 << 22,
    max_dtd_bytes: 1 << 16,
    max_entity_expansion_bytes: 1 << 16,
    max_entity_amplification: 5,
    max_entity_references: 1 << 12,
    max_entity_depth: 8,
    max_external_loads: 8,
    max_external_bytes: 1 << 14,
};

/// Resources are named `s`, `e`, `p` and `0`..`9` relative to the base.
const NAMES: [&str; 13] = [
    "s", "e", "p", "0", "1", "2", "3", "4", "5", "6", "7", "8", "9",
];

type Log = Arc<Mutex<Vec<(String, usize)>>>;

fn resolver(resources: &[Vec<u8>], log: &Log) -> Box<dyn Resolver> {
    let resources = resources.to_vec();
    let log = log.clone();
    Box::new(move |request: Request<'_>| {
        log.lock()
            .unwrap()
            .push((request.uri.to_owned(), request.max_bytes));
        let name = request.uri.strip_prefix("d/").unwrap_or("");
        Ok(NAMES
            .iter()
            .position(|candidate| *candidate == name)
            .and_then(|index| resources.get(index).cloned()))
    })
}

fn surface(index: usize, input: &[u8], resolver: Box<dyn Resolver>) -> Result<usize, Error> {
    let base = "d/doc.xml";
    let mut events = 0;
    match index {
        0 => {
            let mut reader = Reader::with_limits(input, LIMITS)?.with_resolver(base, resolver);
            while reader.read_event()?.is_some() {
                events += 1;
            }
        }
        1 => {
            events = Document::with_resolver(input, LIMITS, base, resolver)?
                .nodes()
                .len()
        }
        2 => {
            events = OwnedDocument::with_resolver(input, LIMITS, base, resolver)?
                .nodes()
                .len()
        }
        3 => {
            let mut parser = PushParser::with_limits(LIMITS).with_resolver(base, resolver);
            let mut count = |_: yaxml::Event<'_>, _| {
                events += 1;
                ControlFlow::Continue(())
            };
            for chunk in input.chunks(7) {
                parser.feed(chunk, &mut count)?;
            }
            parser.finish(&mut count)?;
        }
        _ => {
            let source = BufReader::with_capacity(11, Cursor::new(input));
            let mut reader =
                BufferedReader::with_limits(source, LIMITS).with_resolver(base, resolver);
            loop {
                match reader.read_event() {
                    Ok(Some(_)) => events += 1,
                    Ok(None) => break,
                    Err(ReadError::Xml(error)) => return Err(error),
                    Err(error) => panic!("in-memory buffered input failed: {error}"),
                }
            }
        }
    }
    Ok(events)
}

/// Returns the reader's error after checking every other surface against it.
pub(super) fn exercise(document: &[u8], resources: &[Vec<u8>]) -> Option<Error> {
    let run = |index| {
        let log = Log::default();
        let result = surface(index, document, resolver(resources, &log));
        let requests = log.lock().unwrap().clone();
        assert!(
            requests.len() <= LIMITS.max_external_loads,
            "load budget ignored"
        );
        assert!(
            requests.windows(2).all(|pair| pair[1].1 <= pair[0].1)
                && requests
                    .iter()
                    .all(|(_, left)| *left <= LIMITS.max_external_bytes),
            "byte budget is not cumulative: {requests:?}"
        );
        (result, requests)
    };
    let (expected, requests) = run(0);
    // The DOMs count nodes rather than events; acceptance, error kind and
    // the request sequence must agree.
    for index in 1..5 {
        let (result, made) = run(index);
        assert_eq!(
            (result.err().map(|error| error.kind), &made),
            (expected.err().map(|error| error.kind), &requests),
            "surface {index} disagrees for {document:?}"
        );
    }
    expected.err()
}
