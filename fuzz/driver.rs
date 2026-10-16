//! Shared, safe fuzz entry point. Panic on an invariant violation so external
//! runners preserve the failing input.

use std::collections::HashSet;
use yaxml::{Document, Error, Event, Limits, NodeKind, Reader};

/// Maximum testcase size accepted by the process driver.
pub const MAX_INPUT_BYTES: usize = 1 << 20;

/// Acceptance and event count for differential and regression reporting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Outcome {
    pub error: Option<Error>,
    pub events: usize,
}

/// Exercise all currently implemented byte-input parser entry points.
pub fn exercise(input: &[u8]) -> Outcome {
    assert!(
        input.len() <= MAX_INPUT_BYTES,
        "fuzz testcase exceeds the driver budget"
    );
    let mut stream = Vec::new();
    let outcome = read(input, None, |event| stream.push(event));
    let document = Document::parse_bytes(input);
    assert_eq!(
        outcome.error.is_none(),
        document.is_ok(),
        "reader/DOM acceptance mismatch for {input:?}"
    );
    if let Err(error) = &document {
        check_error(*error, input.len());
    }
    if let Ok(document) = &document {
        check_document(document, input.len(), outcome.events);
        check_projection(document, &stream);
    }
    if let Ok(text) = std::str::from_utf8(input) {
        assert_eq!(document.is_ok(), Document::parse(text).is_ok());
    }
    // Vary policy independently from grammar bytes without needing a runtime.
    let byte = |index| input.get(index).copied().unwrap_or(0) as usize;
    exercise_with_limits(
        input,
        Limits {
            max_input_bytes: if byte(5) & 1 == 0 {
                MAX_INPUT_BYTES
            } else {
                input.len().saturating_sub(1)
            },
            max_token_bytes: byte(0),
            max_depth: byte(1) & 15,
            max_attributes: byte(2) & 15,
            max_events: byte(3),
            max_owned_bytes: byte(4),
        },
    );
    outcome
}

/// Exercise the explicit policy boundary with the same budget in both APIs.
pub fn exercise_with_limits(input: &[u8], limits: Limits) -> Outcome {
    let mut stream = Vec::new();
    let outcome = read(input, Some(limits), |event| stream.push(event));
    let document = Document::with_limits(input, limits);
    assert_eq!(
        outcome.error.is_none(),
        document.is_ok(),
        "reader/DOM policy acceptance mismatch for {input:?}"
    );
    match document {
        Ok(document) => {
            check_document(&document, input.len(), outcome.events);
            check_projection(&document, &stream);
        }
        Err(error) => check_error(error, input.len()),
    }
    outcome
}

fn read<'input>(
    input: &'input [u8],
    limits: Option<Limits>,
    mut emit: impl FnMut(Event<'input>),
) -> Outcome {
    let mut reader = match limits.map_or_else(
        || Reader::new(input),
        |limits| Reader::with_limits(input, limits),
    ) {
        Ok(reader) => reader,
        Err(error) => {
            check_error(error, input.len());
            return Outcome {
                error: Some(error),
                events: 0,
            };
        }
    };
    let mut events = 0;
    let mut previous = reader.position();
    loop {
        let result = reader.read_event();
        let position = reader.position();
        assert!(
            (previous..=input.len()).contains(&position),
            "reader offset moved backwards or outside input"
        );
        previous = position;
        match result {
            Ok(Some(event)) => {
                let range = reader.event_range();
                assert!(range.start <= range.end && range.end <= position);
                events += 1;
                assert!(
                    events <= input.len().saturating_mul(2).saturating_add(1),
                    "reader made no bounded progress"
                );
                emit(event);
            }
            Ok(None) => {
                assert!(
                    reader
                        .read_event()
                        .expect("EOF must remain terminal")
                        .is_none()
                );
                return Outcome {
                    error: None,
                    events,
                };
            }
            Err(error) => {
                check_error(error, input.len());
                assert_eq!(
                    reader.read_event().expect_err("error must remain terminal"),
                    error
                );
                return Outcome {
                    error: Some(error),
                    events,
                };
            }
        }
    }
}

/// Compare the complete logical stream with the DOM, including values and names.
/// This detects conversion loss even though both APIs share lexical parsing.
pub fn check_projection(document: &Document<'_>, stream: &[Event<'_>]) {
    let mut nodes = document.nodes().skip(1);
    let mut parents = Vec::new();
    let mut declaration = None;
    for event in stream {
        match event {
            Event::Declaration(value) => {
                assert!(
                    declaration.replace(value).is_none(),
                    "duplicate declaration"
                );
                continue;
            }
            Event::End(name) => {
                let parent = parents.pop().expect("end event without open element");
                let node = document.node(parent).unwrap();
                assert_eq!(node.name(), Some(name), "end name differs from DOM");
                continue;
            }
            _ => {}
        }
        let node = nodes.next().expect("DOM lost an event node");
        let expected_parent = parents.last().copied().unwrap_or(document.root().id());
        assert_eq!(
            node.parent().unwrap().id(),
            expected_parent,
            "DOM changed event nesting"
        );
        match (event, node.kind()) {
            (Event::Start(expected) | Event::Empty(expected), NodeKind::Element(actual)) => {
                assert_eq!(
                    actual, expected,
                    "element names or attributes differ from stream"
                );
                if matches!(event, Event::Start(_)) {
                    parents.push(node.id());
                }
            }
            (Event::Text(expected), NodeKind::Text(actual))
            | (Event::Cdata(expected), NodeKind::Cdata(actual))
            | (Event::Comment(expected), NodeKind::Comment(actual)) => {
                assert_eq!(actual, expected, "DOM payload differs from stream");
            }
            (
                Event::ProcessingInstruction {
                    target: expected_target,
                    data: expected_data,
                },
                NodeKind::ProcessingInstruction {
                    target: actual_target,
                    data: actual_data,
                },
            ) => {
                assert_eq!(
                    actual_target, expected_target,
                    "PI target differs from stream"
                );
                assert_eq!(actual_data, expected_data, "PI data differs from stream");
            }
            _ => panic!(
                "DOM node kind differs from stream: {event:?} versus {:?}",
                node.kind()
            ),
        }
    }
    assert!(parents.is_empty(), "stream did not close every element");
    assert!(nodes.next().is_none(), "DOM added nodes absent from stream");
    assert_eq!(
        document.declaration(),
        declaration,
        "DOM declaration differs from stream"
    );
}

fn check_error(error: Error, len: usize) {
    assert!(
        error.offset <= len,
        "error offset points outside the input: {error}"
    );
    assert!(!error.to_string().is_empty());
}

fn check_document(document: &Document<'_>, len: usize, events: usize) {
    let nodes: Vec<_> = document.nodes().collect();
    assert!(nodes.len() <= events + 1);
    assert!(matches!(document.root().kind(), NodeKind::Document));
    assert!(matches!(
        document.root_element().kind(),
        NodeKind::Element(_)
    ));
    let _ = document.declaration();
    assert_eq!(document.root().range(), 0..len);
    assert!(document.root().parent().is_none());
    assert!(document.root().next_sibling().is_none());
    assert_eq!(
        document.root_element().parent().unwrap().id(),
        document.root().id()
    );

    let mut seen = HashSet::new();
    let mut pending = vec![document.root()];
    let mut visited = Vec::new();
    while let Some(node) = pending.pop() {
        assert!(seen.insert(node.id()), "DOM cycle or duplicate child");
        assert!(
            visited.len() < nodes.len(),
            "DOM traversal exceeded stored node count"
        );
        visited.push(node.id());
        assert_eq!(document.node(node.id()).unwrap().id(), node.id());
        let range = node.range();
        assert!(range.start <= range.end && range.end <= len);
        let children: Vec<_> = node.children().take(nodes.len() + 1).collect();
        assert!(children.len() <= nodes.len(), "sibling cycle");
        assert_eq!(
            node.first_child().map(|n| n.id()),
            children.first().map(|n| n.id())
        );
        assert_eq!(
            node.last_child().map(|n| n.id()),
            children.last().map(|n| n.id())
        );
        let mut previous_end = range.start;
        let mut subtree_size = 1;
        for (index, child) in children.iter().copied().enumerate() {
            assert_eq!(child.parent().unwrap().id(), node.id());
            assert_eq!(
                child.next_sibling().map(|n| n.id()),
                children.get(index + 1).map(|n| n.id())
            );
            let child_range = child.range();
            assert!(child_range.start >= previous_end && child_range.end <= range.end);
            previous_end = child_range.end;
            subtree_size += child.descendants().len();
        }
        assert_eq!(node.descendants().len(), subtree_size);
        match node.kind() {
            NodeKind::Element(_) => {
                assert!(node.name().is_some());
                assert!(node.text().is_none());
            }
            NodeKind::Document => {
                assert!(node.name().is_none());
                assert!(node.text().is_none());
                assert!(node.attributes().is_empty());
            }
            _ => {
                assert!(node.name().is_none());
                assert!(node.text().is_some());
                assert!(node.attributes().is_empty());
                assert!(children.is_empty());
            }
        }
        pending.extend(children.into_iter().rev());
    }
    assert_eq!(visited, nodes.iter().map(|n| n.id()).collect::<Vec<_>>());
}
