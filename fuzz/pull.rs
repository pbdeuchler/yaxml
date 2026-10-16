//! Lending pull schedules. Compare live events; never snapshot namespace strings.

use std::io::{self, BufRead, Read};
use std::ops::Range;

use yaxml::{
    BufferedReader, DecodeLimits, Error, ErrorKind, Event, Limit, Limits, ReadError, Reader,
};

use super::encoding::{self, Reference};

const CAPACITIES: [usize; 7] = [1, 2, 3, 7, 64, 8192, 65536];
const IO_MESSAGE: &str = "injected pull source failure";

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
}

impl Read for Source<'_> {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        panic!("pull reader bypassed BufRead");
    }
}

impl BufRead for Source<'_> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.calls += 1;
        if std::mem::take(&mut self.interrupted) {
            return Err(io::Error::from(io::ErrorKind::Interrupted));
        }
        if self.failure == Some(self.position) {
            self.failed = true;
            return Err(io::Error::other(IO_MESSAGE));
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
        self.calls += 1;
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
        if result.is_ok() {
            assert!(self.valid(), "pull accepted invalid XML");
        } else if self.valid() {
            let error = result.unwrap_err();
            assert!(
                self.input.len() > 1024
                    && matches!(error.kind, ErrorKind::LimitExceeded(Limit::OwnedBytes)),
                "pull rejected bounded valid XML: {error}"
            );
        } else if let (Some(reference), Some(Err(expected))) = (&self.reference, self.grammar) {
            reference.failure(expected, result.err(), self.input.len());
        }
    }
}

pub(super) fn exercise(input: &[u8], control: u8) {
    let oracle = Oracle::new(input);
    let capacity =
        CAPACITIES[usize::from(control) % CAPACITIES.len()].max(input.len().div_ceil(4096));
    let result = drive(
        input,
        capacity,
        Limits::default(),
        DecodeLimits::default(),
        Some(&oracle),
        None,
        None,
        true,
    );
    oracle.result(result);
    // A large visible suffix must stay unread when an event suspends parsing.
    let bulk = drive(
        input,
        input.len().max(1),
        Limits::default(),
        DecodeLimits::default(),
        Some(&oracle),
        None,
        None,
        false,
    );
    assert_eq!(
        result, bulk,
        "pull buffer capacity changed terminal XML result"
    );
    oracle.result(bulk);

    let _ = drive(
        input,
        capacity,
        Limits::default(),
        DecodeLimits::default(),
        Some(&oracle),
        Some(usize::from(control & 7)),
        None,
        false,
    );
    let failure = usize::from(control).saturating_mul(input.len()) / 255;
    let _ = drive(
        input,
        capacity,
        Limits::default(),
        DecodeLimits::default(),
        Some(&oracle),
        None,
        Some(failure),
        control & 1 == 0,
    );
    golden_limits(input, &oracle);
}

/// Explicit profiles share the existing independent decoded-limit oracle.
pub(super) fn with_limits(input: &[u8], limits: Limits, decode: DecodeLimits) -> Result<(), Error> {
    drive(
        input,
        3.max(input.len().div_ceil(4096)),
        limits,
        decode,
        None,
        None,
        None,
        false,
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "one bounded driver, with explicit source and parser policies"
)]
fn drive(
    input: &[u8],
    capacity: usize,
    limits: Limits,
    decode: DecodeLimits,
    oracle: Option<&Oracle<'_>>,
    stop: Option<usize>,
    failure: Option<usize>,
    interrupted: bool,
) -> Result<(), Error> {
    let source = Source::new(input, capacity, interrupted, failure);
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
            Ok(Some((event, range))) => {
                assert!(range.start <= range.end && range.end <= input.len());
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
                    oracle
                        .unwrap()
                        .event(event, value, range.clone(), expected.event_range());
                }
                count += 1;
                assert!(
                    count <= input.len().saturating_mul(2).saturating_add(1),
                    "pull made no bounded progress"
                );
                assert!(range.end <= pull.position());
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
