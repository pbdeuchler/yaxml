//! Complete-document serialization must preserve the original logical tree.
//! Compare live views; copying all namespace values can amplify a small input.

use std::borrow::Cow;
use std::cell::Cell;
use std::io::{self, Write};

use yaxml::options::NOENT;
use yaxml::{
    Document, ErrorKind, Event, Limit, Limits, NodeKind, OwnedDocument, ParseOptions, WriteError,
    Writer, WriterLimits,
};

#[derive(Clone, Copy)]
enum Tree<'a, 'input> {
    Borrowed(&'a Document<'input>),
    Owned(&'a OwnedDocument),
}

impl Tree<'_, '_> {
    fn write<W: Write>(self, writer: &mut Writer<W>) -> Result<(), WriteError> {
        match self {
            Self::Borrowed(document) => writer.write_document(document),
            Self::Owned(document) => writer.write_owned_document(document),
        }
    }
}

pub(super) fn borrowed(document: &Document<'_>, input: &[u8]) {
    exercise(Tree::Borrowed(document), document, input);
}

pub(super) fn owned(document: &OwnedDocument, expected: &Document<'_>, canonical: &[u8]) {
    exercise(Tree::Owned(document), expected, canonical);
}

fn limits(input: &[u8]) -> WriterLimits {
    // A literal attribute quote can expand sixfold. Childless-element folding,
    // removed transport BOM/whitespace and declaration rewriting cannot exceed
    // this bound for an accepted UTF-8 reference (including decoded UTF-16).
    // Entity expansion and copied defaults add up to the permitted
    // amplification, so the tree, not the input, can be the larger term.
    let ceiling = super::expanded_bound(input.len())
        .saturating_mul(6)
        .saturating_add(1024);
    WriterLimits {
        max_output_bytes: ceiling,
        max_token_bytes: ceiling,
        ..WriterLimits::default()
    }
}

fn exercise(tree: Tree<'_, '_>, expected: &Document<'_>, input: &[u8]) {
    let limits = limits(input);
    let mut writer = Writer::with_limits(Vec::new(), limits);
    match tree.write(&mut writer) {
        Ok(()) => {}
        Err(error) => {
            // At <=256 reference bytes the sixfold encoding bound and the
            // quadratic name/URI bound are safely below default string quotas.
            assert!(
                input.len() > 256
                    && matches!(error, WriteError::Xml(error) if error.kind == ErrorKind::LimitExceeded(Limit::OwnedBytes)),
                "serializer rejected bounded valid tree: {error}"
            );
            terminal(tree, &mut writer);
            return;
        }
    }
    writer.finish().unwrap();
    let position = writer.position();
    // A document method owns completion, including the final flush.
    terminal(tree, &mut writer);
    let output = writer.into_inner();
    assert_eq!(position, output.len());
    assert!(output.len() <= limits.max_output_bytes);
    let reparsed = Document::with_limits(
        &output,
        Limits {
            max_input_bytes: limits.max_output_bytes,
            max_token_bytes: limits.max_token_bytes,
            ..Limits::default()
        },
    )
    .unwrap();
    // The writer expands kept entity references: compare with the substituted tree.
    let substituted = expected
        .nodes()
        .any(|node| node.entity_name().is_some())
        .then(|| Document::parse_with(input, ParseOptions::new(NOENT)).unwrap());
    tree_equal(substituted.as_ref().unwrap_or(expected), &reparsed);
    misuse(tree);
    limited(tree, &output, super::selector(input));
    if output.len() <= 4096 {
        let control = input.get(input.len() / 2).copied().unwrap_or(0);
        scheduled(tree, &output, limits, control);
    }
    if let Some(golden) = golden(input) {
        assert_eq!(
            output, golden.output,
            "document serializer changed byte policy"
        );
        quotas(tree, &golden);
        // Independent sink selectors are deterministic even when XML grammar
        // bytes cannot reach every control value through mutation.
        for control in [0, 1, 2, 3, 4, 5, 6, 7, 124, 252] {
            scheduled(tree, &output, limits, control);
        }
    }
}

/// One input-selected ceiling, often below what the output needs: the same
/// bytes or a limit failure that leaves the writer terminal.
fn limited(tree: Tree<'_, '_>, expected: &[u8], control: u8) {
    let scale = usize::from(control >> 3);
    let generous = WriterLimits::default();
    let limits = match control & 7 {
        0 => WriterLimits {
            max_output_bytes: expected.len() * scale / 32,
            ..generous
        },
        1 => WriterLimits {
            max_token_bytes: expected.len() * scale / 32,
            ..generous
        },
        2 => WriterLimits {
            max_depth: scale & 7,
            ..generous
        },
        3 => WriterLimits {
            max_attributes: scale & 3,
            ..generous
        },
        4 => WriterLimits {
            max_events: scale,
            ..generous
        },
        5 => WriterLimits {
            max_owned_bytes: expected.len() * scale / 8,
            ..generous
        },
        _ => return,
    };
    let mut writer = Writer::with_limits(Vec::new(), limits);
    match tree.write(&mut writer) {
        Ok(()) => assert_eq!(writer.into_inner(), expected),
        Err(error) => {
            assert!(
                matches!(error, WriteError::Xml(error) if matches!(error.kind, ErrorKind::LimitExceeded(_))),
                "limited serializer failed with {error}"
            );
            terminal(tree, &mut writer);
        }
    }
}

fn terminal<W: Write>(tree: Tree<'_, '_>, writer: &mut Writer<W>) {
    let position = writer.position();
    for result in [
        tree.write(writer),
        writer.write_event(&Event::Text(Cow::Borrowed(""))),
        writer.finish(),
    ] {
        assert!(
            matches!(result, Err(WriteError::Xml(error)) if error.kind == ErrorKind::InvalidState)
        );
    }
    assert_eq!(writer.position(), position);
}

fn misuse(tree: Tree<'_, '_>) {
    let mut writer = Writer::new(Vec::new());
    writer.write_event(&Event::Text(Cow::Borrowed(""))).unwrap();
    assert!(
        matches!(tree.write(&mut writer), Err(WriteError::Xml(error)) if error.kind == ErrorKind::InvalidState),
        "empty text failed to consume writer freshness"
    );
    terminal(tree, &mut writer);
    assert!(writer.into_inner().is_empty());
}

fn tree_equal(expected: &Document<'_>, actual: &Document<'_>) {
    match (expected.declaration(), actual.declaration()) {
        (Some(expected), Some(actual)) => {
            assert_eq!(expected.version, actual.version);
            assert_eq!(expected.standalone, actual.standalone);
            assert_eq!(actual.encoding, expected.encoding.map(|_| "UTF-8"));
        }
        (None, None) => {}
        _ => panic!("serializer changed declaration presence"),
    }
    assert_eq!(
        actual.nodes().len(),
        expected.nodes().len(),
        "serializer lost or added tree nodes"
    );
    for (expected, actual) in expected.nodes().zip(actual.nodes()) {
        assert!(
            expected.name() == actual.name(),
            "serializer changed an expanded or qualified name"
        );
        assert!(
            expected.attributes() == actual.attributes(),
            "serializer changed ordered attributes or XMLNS declarations"
        );
        assert!(
            expected.text() == actual.text(),
            "serializer changed a logical value"
        );
        match (expected.kind(), actual.kind()) {
            (NodeKind::Document, NodeKind::Document)
            | (NodeKind::Element(_), NodeKind::Element(_))
            | (NodeKind::Text(_), NodeKind::Text(_))
            | (NodeKind::Cdata(_), NodeKind::Cdata(_))
            | (NodeKind::Comment(_), NodeKind::Comment(_)) => {}
            (
                NodeKind::ProcessingInstruction {
                    target: expected, ..
                },
                NodeKind::ProcessingInstruction { target: actual, .. },
            ) => assert_eq!(actual, expected),
            (NodeKind::Doctype(expected), NodeKind::Doctype(actual)) => {
                assert_eq!(actual, expected)
            }
            _ => panic!("serializer changed node kind"),
        }
        assert_eq!(
            actual.parent().map(|n| n.id()),
            expected.parent().map(|n| n.id())
        );
        assert_eq!(
            actual.first_child().map(|n| n.id()),
            expected.first_child().map(|n| n.id())
        );
        assert_eq!(
            actual.last_child().map(|n| n.id()),
            expected.last_child().map(|n| n.id())
        );
        assert_eq!(
            actual.next_sibling().map(|n| n.id()),
            expected.next_sibling().map(|n| n.id())
        );
        assert_eq!(actual.descendants().len(), expected.descendants().len());
    }
}

struct Golden {
    output: &'static [u8],
    token: usize,
    events: usize,
    attributes: usize,
    depth: usize,
    owned: usize,
}

fn golden(input: &[u8]) -> Option<Golden> {
    let input = input.strip_prefix(b"\xef\xbb\xbf").unwrap_or(input);
    let (output, token, events, attributes, depth, owned) = match input {
        b"<r/>" | b"<r></r>" => (b"<r/>".as_slice(), 4, 1, 0, 1, 0),
        b"<r>a</r>" => (b"<r>a</r>".as_slice(), 4, 3, 0, 1, 1),
        b"<r>&amp;</r>" => (b"<r>&amp;</r>".as_slice(), 5, 3, 0, 1, 6),
        b"<r a='&#9;'/>" => (b"<r a=\"&#9;\"/>".as_slice(), 13, 1, 1, 1, 4),
        b"<r><![CDATA[]]></r>" => (b"<r><![CDATA[]]></r>".as_slice(), 12, 3, 0, 1, 1),
        b"<r><a/><b/></r>" => (b"<r><a/><b/></r>".as_slice(), 4, 4, 0, 2, 1),
        b"<r><a>x</a><a>x</a></r>" => (b"<r><a>x</a><a>x</a></r>".as_slice(), 4, 8, 0, 2, 3),
        _ => return None,
    };
    Some(Golden {
        output,
        token,
        events,
        attributes,
        depth,
        owned,
    })
}

fn quotas(tree: Tree<'_, '_>, golden: &Golden) {
    for (field, minimum) in [
        (Limit::OutputBytes, golden.output.len()),
        (Limit::TokenBytes, golden.token),
        (Limit::Events, golden.events),
        (Limit::Attributes, golden.attributes),
        (Limit::Depth, golden.depth),
        (Limit::OwnedBytes, golden.owned),
    ] {
        for maximum in [minimum.saturating_sub(1), minimum, minimum + 1] {
            let mut limits = WriterLimits::default();
            match field {
                Limit::OutputBytes => limits.max_output_bytes = maximum,
                Limit::TokenBytes => limits.max_token_bytes = maximum,
                Limit::Events => limits.max_events = maximum,
                Limit::Attributes => limits.max_attributes = maximum,
                Limit::Depth => limits.max_depth = maximum,
                Limit::OwnedBytes => limits.max_owned_bytes = maximum,
                _ => unreachable!(),
            }
            let mut writer = Writer::with_limits(Vec::new(), limits);
            let result = tree.write(&mut writer);
            if maximum < minimum {
                assert!(
                    matches!(result, Err(WriteError::Xml(error)) if error.kind == ErrorKind::LimitExceeded(field)),
                    "serializer ignored independent cumulative quota minimum"
                );
                terminal(tree, &mut writer);
            } else {
                assert!(
                    result.is_ok(),
                    "serializer rejected independent exact quota minimum: {result:?}"
                );
                writer.finish().unwrap();
            }
            let position = writer.position();
            let bytes = writer.into_inner();
            assert_eq!(bytes.len(), position);
            assert!(
                golden.output.starts_with(&bytes),
                "quota failure changed confirmed output prefix"
            );
            if maximum >= minimum {
                assert_eq!(bytes, golden.output);
            }
        }
    }
}

#[derive(Debug)]
struct Injected;
impl std::fmt::Display for Injected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("injected document sink failure")
    }
}
impl std::error::Error for Injected {}

struct Sink<'a> {
    expected: &'a [u8],
    position: usize,
    calls: &'a Cell<usize>,
    flushes: &'a Cell<usize>,
    mode: u8,
    failure: usize,
}

impl Write for Sink<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.calls.set(self.calls.get() + 1);
        assert!(
            self.calls.get() <= self.expected.len() * 4 + 16,
            "serializer sink made no bounded progress"
        );
        if matches!(self.mode, 1 | 7) && self.calls.get() == 1 {
            return Err(io::ErrorKind::Interrupted.into());
        }
        if self.mode == 2 {
            return Ok(0);
        }
        if self.mode == 3 {
            return Ok(bytes.len() + 1);
        }
        if self.mode == 4 && self.position == self.failure {
            return Err(io::Error::other(Injected));
        }
        let length = if self.mode == 4 {
            bytes.len().min(self.failure - self.position)
        } else {
            bytes.len().min(3)
        };
        assert_eq!(
            bytes.get(..length),
            self.expected.get(self.position..self.position + length),
            "serializer sink bytes differ from complete output"
        );
        self.position += length;
        Ok(length)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.flushes.set(self.flushes.get() + 1);
        assert!(
            self.flushes.get() <= 2,
            "serializer repeated flush indefinitely"
        );
        if self.mode == 5 {
            return Err(io::Error::other(Injected));
        }
        if matches!(self.mode, 6 | 7) && self.flushes.get() == 1 {
            return Err(io::ErrorKind::Interrupted.into());
        }
        Ok(())
    }
}

fn scheduled(tree: Tree<'_, '_>, expected: &[u8], limits: WriterLimits, control: u8) {
    let mode = control & 7;
    let failure = usize::from(control >> 3) * expected.len() / 31;
    let calls = Cell::new(0);
    let flushes = Cell::new(0);
    let sink = Sink {
        expected,
        position: 0,
        calls: &calls,
        flushes: &flushes,
        mode,
        failure,
    };
    let mut writer = Writer::with_limits(sink, limits);
    let result = tree.write(&mut writer);
    if matches!(mode, 2 | 3 | 5) || (mode == 4 && failure < expected.len()) {
        let error = result.expect_err("serializer swallowed sink failure");
        assert!(std::error::Error::source(&error).is_some());
        let WriteError::Io(cause) = error else {
            panic!("valid tree invented XML failure");
        };
        let kind = match mode {
            2 => io::ErrorKind::WriteZero,
            3 => io::ErrorKind::InvalidData,
            _ => io::ErrorKind::Other,
        };
        assert_eq!(cause.kind(), kind);
        if matches!(mode, 4 | 5) {
            assert!(
                cause
                    .get_ref()
                    .unwrap()
                    .downcast_ref::<Injected>()
                    .is_some(),
                "serializer replaced original I/O cause"
            );
        }
    } else {
        result.unwrap();
        assert_eq!(writer.position(), expected.len());
        assert_eq!(flushes.get(), if matches!(mode, 6 | 7) { 2 } else { 1 });
        let before = (calls.get(), flushes.get());
        writer.finish().unwrap();
        assert_eq!((calls.get(), flushes.get()), before);
    }
    let before = (calls.get(), flushes.get());
    terminal(tree, &mut writer);
    assert_eq!(
        (calls.get(), flushes.get()),
        before,
        "serializer touched sink after terminal state"
    );
    let position = writer.position();
    assert_eq!(writer.into_inner().position, position);
}
