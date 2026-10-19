//! Lending pull schedules. Compare live events; never snapshot namespace strings.

use std::io::{self, BufRead, Read};
use std::ops::Range;

use yaxml::{
    BufferedReader, DecodeLimits, Error, ErrorKind, Event, Limit, Limits, ReadError, Reader,
};

use super::encoding::{self, Reference};

const CAPACITIES: [usize; 7] = [1, 2, 3, 7, 64, 8192, 65536];
const IO_MESSAGE: &str = "injected pull source failure";

#[derive(Debug)]
struct Injected;

impl std::fmt::Display for Injected {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output.write_str(IO_MESSAGE)
    }
}

impl std::error::Error for Injected {}

/// A source whose exposed slice and consume ledger are independently bounded.
/// It owns no input copy and exposes no bytes beyond an injected failure.
struct Source<'a> {
    input: &'a [u8],
    position: usize,
    capacity: usize,
    visible: usize,
    calls: usize,
    interrupted: bool,
    failure: Option<usize>,
    failed: bool,
}

impl<'a> Source<'a> {
    fn new(input: &'a [u8], capacity: usize, interrupted: bool, failure: Option<usize>) -> Self {
        Self {
            input,
            position: 0,
            capacity: capacity.max(1),
            visible: 0,
            calls: 0,
            interrupted,
            failure,
            failed: false,
        }
    }

    fn operation(&mut self) {
        self.calls += 1;
        assert!(
            self.calls <= self.input.len().saturating_mul(8).saturating_add(32),
            "pull source made no bounded progress"
        );
    }
}

impl Read for Source<'_> {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        panic!("pull reader bypassed BufRead");
    }
}

impl BufRead for Source<'_> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.operation();
        if std::mem::take(&mut self.interrupted) {
            return Err(io::Error::from(io::ErrorKind::Interrupted));
        }
        if self.failure == Some(self.position) {
            self.failed = true;
            return Err(io::Error::other(Injected));
        }
        let end = self
            .position
            .saturating_add(self.capacity)
            .min(self.input.len())
            .min(self.failure.unwrap_or(self.input.len()));
        self.visible = end - self.position;
        Ok(&self.input[self.position..end])
    }

    fn consume(&mut self, amount: usize) {
        self.operation();
        assert!(amount <= self.visible, "pull consumed beyond exposed bytes");
        self.visible -= amount;
        self.position += amount;
    }
}

struct Oracle<'a> {
    input: &'a [u8],
    reference: Option<Reference>,
    encoded: bool,
    grammar: Option<Result<(), Error>>,
}

impl<'a> Oracle<'a> {
    fn new(input: &'a [u8]) -> Self {
        let encoded = encoding::recognizes(input);
        let reference = encoded.then(|| Reference::new(input)).flatten();
        let canonical = if encoded {
            reference.as_ref().map(|value| value.canonical.as_bytes())
        } else {
            Some(input)
        };
        let grammar = canonical.map(|bytes| {
            let mut reader = Reader::new(bytes)?;
            let mut count = 0;
            while reader.read_event()?.is_some() {
                count += 1;
                assert!(count <= bytes.len().saturating_mul(2).saturating_add(1));
            }
            Ok(())
        });
        Self {
            input,
            reference,
            encoded,
            grammar,
        }
    }

    fn canonical(&self) -> Option<&[u8]> {
        if self.encoded {
            self.reference
                .as_ref()
                .map(|value| value.canonical.as_bytes())
        } else {
            Some(self.input)
        }
    }

    fn valid(&self) -> bool {
        self.grammar == Some(Ok(()))
    }

    fn event(
        &self,
        actual: Event<'_>,
        expected: Event<'_>,
        range: Range<usize>,
        expected_range: Range<usize>,
    ) {
        if let Some(reference) = &self.reference {
            reference.event(actual, expected);
            assert_eq!(range, reference.range(expected_range));
        } else {
            assert!(actual == expected, "pull typed value changed");
            assert_eq!(range, expected_range);
        }
    }

    fn result(&self, result: Result<(), Error>) {
        match result {
            Ok(()) => assert!(self.valid(), "pull accepted invalid XML"),
            Err(error) if self.valid() => assert!(
                self.input.len() > 1024
                    && matches!(error.kind, ErrorKind::LimitExceeded(Limit::OwnedBytes)),
                "pull rejected bounded valid XML: {error}"
            ),
            Err(error) => {
                if let (Some(reference), Some(Err(expected))) = (&self.reference, self.grammar) {
                    reference.failure(expected, Some(error), self.input.len());
                }
            }
        }
    }
}

pub(super) fn exercise(input: &[u8], control: u8) {
    let oracle = Oracle::new(input);
    let capacity =
        CAPACITIES[usize::from(control) % CAPACITIES.len()].max(input.len().div_ceil(4096));
    let result = drive(
        Source::new(input, capacity, true, None),
        Limits::default(),
        DecodeLimits::default(),
        Some(&oracle),
        None,
    );
    oracle.result(result);
    // A large visible suffix must stay unread when an event suspends parsing.
    let bulk = drive(
        Source::new(input, input.len().max(1), false, None),
        Limits::default(),
        DecodeLimits::default(),
        Some(&oracle),
        None,
    );
    assert_eq!(
        result, bulk,
        "pull buffer capacity changed terminal XML result"
    );
    oracle.result(bulk);

    let _ = drive(
        Source::new(input, capacity, false, None),
        Limits::default(),
        DecodeLimits::default(),
        Some(&oracle),
        Some(usize::from(control & 7)),
    );
    let failure = usize::from(control).saturating_mul(input.len()) / 255;
    let _ = drive(
        Source::new(input, capacity, control & 1 == 0, Some(failure)),
        Limits::default(),
        DecodeLimits::default(),
        Some(&oracle),
        None,
    );
    golden_limits(input, &oracle);
}

/// Explicit profiles share the existing independent decoded-limit oracle.
pub(super) fn with_limits(input: &[u8], limits: Limits, decode: DecodeLimits) -> Result<(), Error> {
    drive(
        Source::new(input, 3.max(input.len().div_ceil(4096)), false, None),
        limits,
        decode,
        None,
        None,
    )
}

fn drive(
    source: Source<'_>,
    limits: Limits,
    decode: DecodeLimits,
    oracle: Option<&Oracle<'_>>,
    stop: Option<usize>,
) -> Result<(), Error> {
    let input = source.input;
    let capacity = source.capacity;
    let interrupted = source.interrupted;
    let default_decode = decode.max_decoded_bytes == DecodeLimits::default().max_decoded_bytes
        && decode.max_decoded_token_bytes == DecodeLimits::default().max_decoded_token_bytes;
    let mut pull = if limits == Limits::default() && default_decode && interrupted {
        BufferedReader::new(source)
    } else if default_decode {
        BufferedReader::with_limits(source, limits)
    } else {
        BufferedReader::with_decode_limits(source, limits, decode)
    };
    assert_eq!(pull.position(), 0);
    assert_eq!(pull.get_ref().calls, 0, "pull constructor performed I/O");
    let mut expected = oracle
        .filter(|oracle| oracle.valid())
        .map(|oracle| Reader::new(oracle.canonical().unwrap()).unwrap());
    let mut count = 0;
    let mut previous = 0;
    let mut retained = None;
    let result = loop {
        if stop == Some(count) {
            break Ok(());
        }
        let result = pull.read_event();
        match result {
            Ok(Some((event, origin))) => {
                let range = origin
                    .input_range()
                    .unwrap_or_else(|| origin.physical_context());
                assert!(range.start <= range.end && range.end <= input.len());
                let lookahead = if matches!(&event, Event::Text(_)) {
                    if encoding::recognizes(input) { 2 } else { 1 }
                } else {
                    0
                };
                // DTD text ends only after classifying its terminating general
                // entity reference, so that whole `&name;` is consumed lookahead.
                let terminator = &input[range.end..];
                let reference = terminator.iter().take(lookahead).any(|&byte| byte == b'&');
                let lookahead = terminator
                    .iter()
                    .position(|&byte| byte == b';')
                    .filter(|_| reference)
                    .map_or(lookahead, |end| end + lookahead);
                let latest_position = range.end.saturating_add(lookahead).max(4);
                if count == capacity % 7 {
                    let payload = match &event {
                        Event::Text(value) | Event::Cdata(value) | Event::Comment(value) => {
                            Some(value.as_ref())
                        }
                        Event::ProcessingInstruction { data, .. } => Some(data.as_ref()),
                        _ => None,
                    };
                    // Exercise caller retention without namespace-copy amplification.
                    retained = payload.map(|value| value.chars().take(32).collect::<String>());
                }
                if let Some(expected) = &mut expected {
                    let value = expected.read_event().unwrap().expect("extra pull event");
                    oracle.unwrap().event(
                        event,
                        value,
                        range.clone(),
                        super::span(expected.event_origin().unwrap()),
                    );
                }
                count += 1;
                assert!(
                    count <= input.len().saturating_mul(2).saturating_add(1),
                    "pull made no bounded progress"
                );
                assert!(range.end <= pull.position());
                assert!(
                    pull.position() <= latest_position,
                    "pull read beyond an event suspension"
                );
            }
            Ok(None) => {
                if let Some(expected) = &mut expected {
                    assert!(
                        expected.read_event().unwrap().is_none(),
                        "pull lost events before EOF"
                    );
                }
                assert_eq!(pull.position(), input.len());
                let calls = pull.get_ref().calls;
                assert!(pull.read_event().unwrap().is_none());
                assert_eq!(pull.get_ref().calls, calls, "pull touched source after EOF");
                break Ok(());
            }
            Err(ReadError::Xml(error)) => {
                super::check_error(error, input.len());
                let calls = pull.get_ref().calls;
                assert!(
                    matches!(pull.read_event(), Err(ReadError::Xml(actual)) if actual == error)
                );
                assert_eq!(
                    pull.get_ref().calls,
                    calls,
                    "pull touched source after XML failure"
                );
                break Err(error);
            }
            Err(ReadError::Io(error)) => {
                assert!(pull.get_ref().failed, "pull invented an I/O error");
                assert_eq!(error.kind(), io::ErrorKind::Other);
                assert_eq!(error.to_string(), IO_MESSAGE);
                assert!(
                    error
                        .get_ref()
                        .unwrap()
                        .downcast_ref::<Injected>()
                        .is_some(),
                    "pull replaced the original I/O cause"
                );
                let calls = pull.get_ref().calls;
                assert!(
                    matches!(pull.read_event(), Err(ReadError::Xml(error)) if error.kind == ErrorKind::InvalidState)
                );
                assert_eq!(
                    pull.get_ref().calls,
                    calls,
                    "pull touched source after I/O failure"
                );
                break Ok(());
            }
            Err(error) => panic!("unexpected pull error variant: {error}"),
        }
        let position = pull.position();
        assert!((previous..=input.len()).contains(&position));
        assert_eq!(
            position,
            pull.get_ref().position,
            "pull source consume ledger changed"
        );
        previous = position;
    };
    let position = pull.position();
    std::hint::black_box(retained);
    assert!((previous..=input.len()).contains(&position));
    let source = pull.into_inner();
    assert_eq!(
        source.position, position,
        "pull extraction lost consumed bytes"
    );
    assert_eq!(&source.input[source.position..], &input[position..]);
    result
}

fn golden_limits(input: &[u8], oracle: &Oracle<'_>) {
    let Some(canonical) = oracle
        .canonical()
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
    else {
        return;
    };
    let canonical = canonical.strip_prefix('\u{feff}').unwrap_or(canonical);
    // Independent tiny-document minima: retained open name, normalization work,
    // semantic events and longest lexical token. No pool belongs to pull input.
    let (owned, events, token, attributes) = match canonical {
        "<r/>" => (0usize, 1, 4, 0),
        "<r>a</r>" => (1, 3, 4, 0),
        "<r>&amp;</r>" => (6, 3, 5, 0),
        "<r a='&#9;'/>" => (4, 1, canonical.len(), 1),
        _ => return,
    };
    let stride = if oracle.encoded { 2 } else { 1 };
    for (field, minimum) in [
        (Limit::InputBytes, input.len()),
        (Limit::TokenBytes, token * stride),
        (Limit::OwnedBytes, owned),
        (Limit::Events, events),
        (Limit::Attributes, attributes),
        (Limit::Depth, 1),
    ] {
        for maximum in [minimum.saturating_sub(1), minimum, minimum + 1] {
            let mut limits = Limits::default();
            match field {
                Limit::InputBytes => limits.max_input_bytes = maximum,
                Limit::TokenBytes => limits.max_token_bytes = maximum,
                Limit::OwnedBytes => limits.max_owned_bytes = maximum,
                Limit::Events => limits.max_events = maximum,
                Limit::Attributes => limits.max_attributes = maximum,
                Limit::Depth => limits.max_depth = maximum,
                _ => unreachable!(),
            }
            let result = with_limits(input, limits, DecodeLimits::default());
            if maximum < minimum {
                assert!(
                    matches!(result, Err(error) if error.kind == ErrorKind::LimitExceeded(field)),
                    "pull ignored independent policy minimum"
                );
            } else {
                assert!(
                    result.is_ok(),
                    "pull rejected independent exact policy minimum"
                );
            }
        }
    }
}
