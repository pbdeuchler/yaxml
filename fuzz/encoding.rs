//! Independent UTF-16 and declared single-byte reference decoding for the
//! auto-decoding input surfaces.
//!
//! Only this bounded test harness materializes decoded input and an offset map.
//! The library must stream with constant decoder carry and token-local mapping.

use std::ops::{ControlFlow, Range};

use yaxml::{
    DecodeLimits, Document, Error, ErrorKind, Event, Limit, Limits, OwnedDocument, PushParser,
    ReadError, Reader,
};

use super::Outcome;

pub(super) fn recognizes(input: &[u8]) -> bool {
    byte_table(input).is_some()
        || input.starts_with(&[0xff, 0xfe])
        || input.starts_with(&[0xfe, 0xff])
        || input.get(..4).is_some_and(|prefix| {
            matches!(prefix, [0, 60, 0, 63] | [60, 0, 63, 0]) || unsupported_signature(prefix)
        })
}

/// UCS-4 and EBCDIC signatures, which have no built-in libxml2 decoder.
fn unsupported_signature(prefix: &[u8]) -> bool {
    matches!(
        prefix,
        [0, 0, 0xfe, 0xff]
            | [0xff, 0xfe, 0, 0]
            | [0, 0, 0xff, 0xfe]
            | [0xfe, 0xff, 0, 0]
            | [0, 0, 0, 60]
            | [60, 0, 0, 0]
            | [0, 0, 60, 0]
            | [0, 60, 0, 0]
            | [0x4c, 0x6f, 0xa7, 0x94]
    )
}

/// libxml2 2.15.4 xmllint decoding of bytes 0x80..=0xFF per built-in charset.
const CHARSETS: &str = include_str!("../tests/fixtures/charsets.tsv");

/// Resolves a BOM-less declaration label through the public owning parser,
/// one probe per high byte, and requires that table to be an xmllint row.
fn byte_table(input: &[u8]) -> Option<Vec<u32>> {
    let ascii = &input[..input
        .iter()
        .position(|b| !b.is_ascii())
        .unwrap_or(input.len())];
    let ascii = std::str::from_utf8(ascii).unwrap();
    let label = &ascii[encoding_value(ascii)?];
    if label.eq_ignore_ascii_case("UTF-8") {
        return None;
    }
    let probed = (0x80..=0xffu8)
        .map(|byte| {
            let head = format!("<?xml version='1.0' encoding='{label}'?><r>");
            match OwnedDocument::parse_bytes(&[head.as_bytes(), &[byte], b"</r>"].concat()) {
                Ok(document) => document.root_element().text_chunks().next()?.chars().next(),
                Err(error) if error.kind == ErrorKind::UnmappedByte => Some('\0'),
                Err(_) => None,
            }
            .map(u32::from)
        })
        .collect::<Option<Vec<_>>>()?;
    let row = CHARSETS
        .lines()
        .filter(|line| !line.starts_with('#'))
        .map(|line| {
            let values = line.split_once('\t').unwrap().1.split(' ');
            values.map(|value| u32::from_str_radix(value, 16).unwrap())
        })
        .find(|row| row.clone().eq(probed.iter().copied()));
    assert!(
        row.is_some(),
        "{label} decoded with a table no xmllint row has"
    );
    Some(probed)
}

pub(super) struct Reference {
    pub(super) canonical: String,
    original_encoding: Option<String>,
    /// The recorded declaration encoding: the label, or the detected
    /// encoding's name when the label is outside its allowed set.
    declared: Option<String>,
    label_start: Option<usize>,
    raw_offsets: Vec<usize>,
}

impl Reference {
    pub(super) fn new(input: &[u8]) -> Option<Self> {
        if let Some(table) = byte_table(input) {
            let decoded = input
                .iter()
                .map(|&byte| match byte {
                    0..0x80 => Some(char::from(byte)),
                    _ => char::from_u32(table[usize::from(byte - 0x80)]).filter(|c| *c != '\0'),
                })
                .collect::<Option<String>>()?;
            return Some(Self::decoded(decoded, 0, |_| 1));
        }
        if input.get(..4).is_some_and(unsupported_signature) {
            return None;
        }
        let (little, bom) = if input.starts_with(&[0xff, 0xfe]) {
            (true, 2)
        } else if input.starts_with(&[0xfe, 0xff]) {
            (false, 2)
        } else {
            (input.starts_with(&[60, 0, 63, 0]), 0)
        };
        let bytes = &input[bom..];
        if !bytes.len().is_multiple_of(2) {
            return None;
        }
        let units = bytes.as_chunks::<2>().0.iter().map(|&pair| {
            if little {
                u16::from_le_bytes(pair)
            } else {
                u16::from_be_bytes(pair)
            }
        });
        let decoded = char::decode_utf16(units)
            .collect::<Result<String, _>>()
            .ok()?;
        // A second U+FEFF is content, not another transport BOM. The UTF-8
        // reference reader would otherwise strip it and conceal a defect.
        if decoded.starts_with('\u{feff}') {
            return None;
        }
        let span = encoding_value(&decoded);
        let original_encoding = span.as_ref().map(|span| decoded[span.clone()].to_owned());
        let matching_label = if little { "UTF-16LE" } else { "UTF-16BE" };
        let allowed = ["UTF-16", "UTF16", matching_label];
        // Any well-formed EncName is accepted; a BOM-less source needs one.
        let enc_name = |label: &str| {
            label.starts_with(|c: char| c.is_ascii_alphabetic())
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        };
        if original_encoding
            .as_deref()
            .map_or(bom == 0, |label| !enc_name(label))
        {
            return None;
        }
        let mut reference = Self::decoded(decoded, bom, |character| character.len_utf16() * 2);
        // libxml2 keeps the detected encoding and records its name for any
        // other label.
        if let Some(label) = &mut reference.declared
            && !allowed.iter().any(|a| a.eq_ignore_ascii_case(label))
        {
            *label = matching_label.to_owned();
        }
        Some(reference)
    }

    /// Maps each decoded scalar to its source width after a `bom`-byte prefix.
    fn decoded(decoded: String, bom: usize, width: impl Fn(char) -> usize) -> Self {
        let span = encoding_value(&decoded);
        let original_encoding = span.as_ref().map(|span| decoded[span.clone()].to_owned());
        let mut canonical = decoded.clone();
        if let Some(span) = &span {
            canonical.replace_range(span.clone(), "UTF-8");
        }
        let mut original_offsets = vec![usize::MAX; decoded.len() + 1];
        let mut raw = bom;
        for (offset, character) in decoded.char_indices() {
            original_offsets[offset] = raw;
            raw += width(character);
        }
        original_offsets[decoded.len()] = raw;
        let raw_offsets = (0..=canonical.len())
            .map(|offset| {
                let original = match &span {
                    Some(span) if offset >= span.start + 5 => offset + span.len() - 5,
                    Some(span) if offset > span.start => return usize::MAX,
                    _ => offset,
                };
                original_offsets[original]
            })
            .collect();
        Self {
            canonical,
            declared: original_encoding.clone(),
            original_encoding,
            label_start: span.map(|span| span.start),
            raw_offsets,
        }
    }

    pub(super) fn range(&self, range: Range<usize>) -> Range<usize> {
        let start = self.raw_offsets[range.start];
        let end = self.raw_offsets[range.end];
        assert_ne!(
            start,
            usize::MAX,
            "reference span split a scalar or encoding label"
        );
        assert_ne!(
            end,
            usize::MAX,
            "reference span split a scalar or encoding label"
        );
        start..end
    }

    pub(super) fn event(&self, actual: &Event<'_>, expected: Event<'_>) {
        match (actual, expected) {
            (Event::Declaration(actual), Event::Declaration(expected)) => {
                assert_eq!(actual.version, expected.version);
                assert_eq!(actual.standalone, expected.standalone);
                assert_eq!(actual.encoding, self.declared.as_deref());
            }
            (actual, expected) => assert!(*actual == expected, "decoded typed event changed"),
        }
    }

    pub(super) fn failure(&self, expected: Error, actual: Option<Error>, input_len: usize) {
        // The slice reader validates all characters before grammar, whereas
        // streaming may encounter an earlier semantic error. Resource and
        // allocation failures can likewise precede later malformed markup.
        if matches!(
            expected.kind,
            ErrorKind::InvalidUtf8
                | ErrorKind::InvalidCharacter
                | ErrorKind::Allocation
                | ErrorKind::LimitExceeded(_)
        ) {
            return;
        }
        let actual = actual.expect("invalid decoded grammar accepted");
        if input_len > 1024 && resource_error(actual.kind) {
            return;
        }
        assert!(
            !matches!(
                actual.kind,
                ErrorKind::LimitExceeded(_) | ErrorKind::Allocation
            ),
            "decoded grammar failure concealed by unrelated resource rejection"
        );
        // A slice can reject a second root or top-level CDATA before a
        // streaming token is complete. Both still identify its raw start.
        let incomplete_markup = actual.kind == ErrorKind::UnexpectedEof
            && matches!(
                expected.kind,
                ErrorKind::MultipleRoots | ErrorKind::TextOutsideRoot
            );
        assert!(
            actual.kind == expected.kind || incomplete_markup,
            "decoded semantic error changed kind: actual={actual}, expected={expected}"
        );
        assert!(expected.offset <= self.canonical.len());
        let boundary = self.canonical.floor_char_boundary(expected.offset);
        let mut offset = self.raw_offsets[boundary];
        if offset == usize::MAX {
            // A location inside the synthetic ASCII label has no individual
            // source-byte counterpart. Anchor it at the original value start.
            offset = self.raw_offsets[self.label_start.expect("unmapped label location")];
        }
        assert_eq!(
            actual.offset, offset,
            "decoded semantic error changed original source offset"
        );
    }
}

// Rewriting a single declaration value lets the independent UTF-8 Reader check
// all XML grammar, including ordering, duplicates and EncName syntax. The label
// is separately required to agree above; malformed labels cannot gain acceptance.
fn encoding_value(input: &str) -> Option<Range<usize>> {
    let declaration = input.strip_prefix("<?xml")?;
    if !declaration.starts_with([' ', '\t', '\r', '\n']) {
        return None;
    }
    let end = input.find("?>")?;
    let offset = input[..end].find("encoding")? + "encoding".len();
    let suffix = input[offset..end].trim_start_matches([' ', '\t', '\r', '\n']);
    let suffix = suffix
        .strip_prefix('=')?
        .trim_start_matches([' ', '\t', '\r', '\n']);
    let quote = suffix.chars().next()?;
    if !matches!(quote, '\'' | '"') {
        return None;
    }
    let start = end - suffix.len() + 1;
    let length = input[start..end].find(quote)?;
    Some(start..start + length)
}

pub(super) fn exercise(input: &[u8]) -> Outcome {
    required_declaration_rejections(input);
    let reference = Reference::new(input);
    let document = reference.as_ref().map(|r| Document::parse(&r.canonical));
    let valid = matches!(&document, Some(Ok(_)));
    // These borrowing APIs keep their explicit UTF-8 contract.
    assert!(Document::parse_bytes(input).is_err());
    let mut count = 0;
    let result = compare_stream(input, &[], reference.as_ref(), valid, &mut count);
    if let (Some(reference), Some(Err(expected))) = (&reference, &document) {
        reference.failure(*expected, result.err(), input.len());
    }
    let mut schedule = Vec::new();
    let mut left = input.len();
    while left != 0 {
        let width = (1 + input[schedule.len() % input.len()] as usize % 17)
            .max(input.len().div_ceil(4096))
            .min(left);
        schedule.push(width);
        left -= width;
    }
    let mut chunked_count = 0;
    let chunked = compare_stream(
        input,
        &schedule,
        reference.as_ref(),
        valid,
        &mut chunked_count,
    );
    if let (Some(reference), Some(Err(expected))) = (&reference, &document) {
        reference.failure(*expected, chunked.err(), input.len());
    }
    assert_eq!(
        result.is_ok(),
        chunked.is_ok(),
        "chunking changed UTF-16 acceptance"
    );
    if result.is_ok() {
        assert_eq!(count, chunked_count);
    }
    let mut reader =
        valid.then(|| Reader::new(reference.as_ref().unwrap().canonical.as_bytes()).unwrap());
    let io = PushParser::new().read_from(std::io::Cursor::new(input), |event, origin| {
        let range = super::span(origin);
        if let Some(reader) = &mut reader {
            let expected = reader
                .read_event()
                .unwrap()
                .expect("extra decoded I/O event");
            let reference = reference.as_ref().unwrap();
            reference.event(event, expected);
            assert_eq!(
                range,
                reference.range(super::span(reader.event_origin().unwrap()))
            );
        }
        ControlFlow::Continue(())
    });
    match io {
        Ok(()) => {
            assert!(valid, "I/O accepted invalid UTF-16 XML");
            assert!(reader.unwrap().read_event().unwrap().is_none());
        }
        Err(ReadError::Xml(error)) => {
            super::check_error(error, input);
            assert!(!valid || resource_error(error.kind));
            if let (Some(reference), Some(Err(expected))) = (&reference, &document) {
                reference.failure(*expected, Some(error), input.len());
            }
        }
        Err(error) => panic!("cursor produced an I/O failure: {error}"),
    }
    let mut temporary = input.to_vec();
    let owned = OwnedDocument::parse_bytes(&temporary);
    let owned_error = owned.as_ref().err().copied();
    if let (Some(reference), Some(Err(expected))) = (&reference, &document) {
        reference.failure(*expected, owned_error, input.len());
    }
    temporary.fill(0);
    drop(temporary);
    if valid && input.len() <= 1024 {
        assert!(result.is_ok(), "small valid UTF-16 rejected by stream");
        assert!(owned.is_ok(), "small valid UTF-16 rejected by owned tree");
    }
    match owned {
        Ok(owned) => {
            assert!(valid, "owned tree accepted invalid UTF-16 XML");
            let reference = reference.as_ref().unwrap();
            let borrowed = document.as_ref().unwrap().as_ref().unwrap();
            super::serialize::owned(&owned, borrowed, reference.canonical.as_bytes());
            let expected = borrowed.declaration();
            match (owned.declaration(), expected) {
                (Some(actual), Some(expected)) => {
                    assert_eq!(actual.version, expected.version);
                    assert_eq!(actual.standalone, expected.standalone);
                    assert_eq!(actual.encoding, reference.declared.as_deref());
                }
                (None, None) => {}
                _ => panic!("owned declaration disappeared"),
            }
            let mut root = true;
            super::owned::check_nodes(&owned, borrowed, |range| {
                if std::mem::take(&mut root) {
                    0..input.len()
                } else {
                    reference.range(range)
                }
            });
        }
        Err(error) => {
            super::check_error(error, input);
            assert!(
                !valid || resource_error(error.kind),
                "valid UTF-16 rejected: {error}"
            );
        }
    }
    if let Ok(text) = std::str::from_utf8(input) {
        assert_eq!(OwnedDocument::parse(text).err(), owned_error);
    }
    super::check_io_schedule(input);
    super::writer::exercise(input, false);
    let byte = |index| input.get(index).copied().unwrap_or(0) as usize;
    exercise_with_limits(
        input,
        Limits {
            max_input_bytes: input.len(),
            max_token_bytes: byte(0),
            max_depth: byte(1) & 15,
            max_attributes: byte(2) & 15,
            max_events: byte(3),
            max_owned_bytes: byte(4),
            ..Limits::default()
        },
    );
    exercise_decode_limits(
        input,
        DecodeLimits {
            max_decoded_bytes: byte(5),
            max_decoded_token_bytes: byte(6),
        },
    );
    if let Some(minimum) = reference.as_ref().and_then(owned_minimum) {
        for maximum in [minimum - 1, minimum] {
            owned_limit_profile(
                input,
                Limits {
                    max_owned_bytes: maximum,
                    ..Limits::default()
                },
                Some(minimum),
            );
        }
    }
    Outcome {
        error: result.err(),
        events: count,
    }
}

/// Exercise the public point where BOM-less UTF-16 must prove its required
/// declaration has an encoding pseudo-attribute.
fn required_declaration_rejections(input: &[u8]) {
    let little_endian = input.starts_with(&[0xff, 0xfe]) || input.starts_with(&[b'<', 0, b'?', 0]);
    let encoded = encode_utf16("<?xml version='1.0'?><r/>", little_endian);
    let expected = Error {
        kind: ErrorKind::EncodingMismatch,
        offset: 0,
    };
    let streamed = super::drive_push(
        &encoded,
        &[1, 2, 3, encoded.len()],
        Limits::default(),
        |_, _| panic!("required-declaration rejection emitted an event"),
    );
    check_required_declaration(streamed, expected);
    check_required_declaration(
        super::pull::with_limits(&encoded, Limits::default(), DecodeLimits::default()),
        expected,
    );
    check_required_declaration(OwnedDocument::parse_bytes(&encoded).map(|_| ()), expected);
}

fn encode_utf16(text: &str, little_endian: bool) -> Vec<u8> {
    text.encode_utf16()
        .flat_map(|unit| {
            if little_endian {
                unit.to_le_bytes()
            } else {
                unit.to_be_bytes()
            }
        })
        .collect()
}

fn check_required_declaration(result: Result<(), Error>, expected: Error) {
    assert_eq!(
        result,
        Err(expected),
        "BOM-less UTF-16 declaration rejection changed"
    );
}

fn resource_error(kind: ErrorKind) -> bool {
    matches!(
        kind,
        ErrorKind::LimitExceeded(Limit::OwnedBytes | Limit::DecodedTokenBytes)
    )
}

fn compare_stream(
    input: &[u8],
    schedule: &[usize],
    reference: Option<&Reference>,
    valid: bool,
    count: &mut usize,
) -> Result<(), yaxml::Error> {
    let mut reader = valid.then(|| Reader::new(reference.unwrap().canonical.as_bytes()).unwrap());
    let result = super::drive_push(input, schedule, Limits::default(), |event, range| {
        *count += 1;
        if let Some(reader) = &mut reader {
            let expected = reader.read_event().unwrap().expect("extra decoded event");
            let reference = reference.unwrap();
            reference.event(event, expected);
            assert_eq!(
                range,
                reference.range(super::span(reader.event_origin().unwrap()))
            );
        }
    });
    match result {
        Ok(()) => {
            assert!(valid, "stream accepted invalid UTF-16 XML");
            assert!(
                reader.unwrap().read_event().unwrap().is_none(),
                "missing decoded event"
            );
        }
        Err(error) => assert!(
            !valid || resource_error(error.kind),
            "valid UTF-16 rejected: {error}"
        ),
    }
    result
}

pub(super) fn exercise_with_limits(input: &[u8], limits: Limits) -> Outcome {
    let minimum = if input.len() <= 80 && recognizes(input) && aggregate_only(limits) {
        Reference::new(input).as_ref().and_then(owned_minimum)
    } else {
        None
    };
    owned_limit_profile(input, limits, minimum)
}

fn aggregate_only(limits: Limits) -> bool {
    Limits {
        max_owned_bytes: Limits::default().max_owned_bytes,
        ..limits
    } == Limits::default()
}

fn owned_minimum(reference: &Reference) -> Option<usize> {
    // Independent allocation-work goldens: an open root retains one name,
    // normalization charges decoded source length, and pool growth charges
    // each full replacement capacity. Empty roots retain no open name.
    match reference.canonical.as_str() {
        "<r/>" => Some(1),
        "<r>a</r>" => Some(4),
        "<r>&amp;</r>" => Some(9),
        "<r a='&#9;'/>" => Some(10),
        _ => None,
    }
}

fn owned_limit_profile(input: &[u8], limits: Limits, minimum: Option<usize>) -> Outcome {
    let mut events = 0;
    let result = super::drive_push(input, &[input.len()], limits, |_, _| events += 1);
    let owned = OwnedDocument::with_limits(input, limits);
    if aggregate_only(limits)
        && let Some(minimum) = minimum
    {
        assert_eq!(
            owned.is_ok(),
            limits.max_owned_bytes >= minimum,
            "UTF-16 owned aggregate budget boundary changed"
        );
        if let Err(error) = &owned {
            assert_eq!(error.kind, ErrorKind::LimitExceeded(Limit::OwnedBytes));
        }
    }
    if let Err(error) = owned {
        super::check_error(error, input);
    } else {
        assert!(result.is_ok(), "owned accepted a stream policy failure");
    }
    Outcome {
        error: result.err(),
        events,
    }
}

pub(super) fn exercise_decode_limits(input: &[u8], limits: DecodeLimits) {
    let reference = recognizes(input).then(|| Reference::new(input)).flatten();
    let canonical = if recognizes(input) {
        reference
            .as_ref()
            .map(|reference| reference.canonical.as_str())
    } else {
        std::str::from_utf8(input).ok()
    };
    // The reference's token ceiling describes decoded UTF-8. Raise it for this
    // grammar check so a >1 MiB decoded token can exercise an explicit increase
    // of DecodeLimits even though its UTF-16 source fits the 1 MiB driver cap.
    let document = canonical
        .and_then(|text| Document::with_limits(text.as_bytes(), reference_limits(text)).ok());
    let oracle = document
        .as_ref()
        .map(|document| DecodeOracle::new(canonical.unwrap(), reference.as_ref(), document));
    let mut profiles = vec![limits];
    if let Some(oracle) = &oracle {
        let total = oracle.total;
        let token = oracle.token;
        for (total, token) in [
            (total, token),
            (total - 1, token),
            (total + 1, token),
            (total, token - 1),
            (total, token + 1),
        ] {
            if !profiles.iter().any(|profile| {
                profile.max_decoded_bytes == total && profile.max_decoded_token_bytes == token
            }) {
                profiles.push(DecodeLimits {
                    max_decoded_bytes: total,
                    max_decoded_token_bytes: token,
                });
            }
        }
    }
    // A fixed maximum of six profiles covers arbitrary policy plus the exact,
    // below and above boundaries independently for each new ceiling.
    for limits in profiles {
        decode_profile(input, limits, oracle.as_ref());
    }
}

fn reference_limits(input: &str) -> Limits {
    Limits {
        max_token_bytes: input.len(),
        ..Limits::default()
    }
}

struct DecodeOracle<'a> {
    canonical: &'a str,
    reference: Option<&'a Reference>,
    document: &'a Document<'a>,
    total: usize,
    token: usize,
    /// Entity expansion produced tokens the source spans cannot measure.
    generated: bool,
}

impl<'a> DecodeOracle<'a> {
    fn new(
        canonical: &'a str,
        reference: Option<&'a Reference>,
        document: &'a Document<'a>,
    ) -> Self {
        // The canonical text spells the original label as "UTF-8"; aliases can
        // be shorter, so add the original length before removing that one.
        let label = reference
            .and_then(|reference| reference.original_encoding.as_ref())
            .map_or("UTF-8".len(), String::len);
        let mut reader =
            Reader::with_limits(canonical.as_bytes(), reference_limits(canonical)).unwrap();
        let bom = reader.position();
        let mut previous = bom;
        let mut token = 0;
        let mut generated = false;
        while let Some(event) = reader.read_event().unwrap() {
            generated |= reader.event_range().is_none();
            let range = super::span(reader.event_origin().unwrap());
            // Events expanded from one reference share its span.
            token = token.max(range.start.saturating_sub(previous));
            // A DOCTYPE is one `<!DOCTYPE` token, then separately bounded units.
            if matches!(event, Event::Doctype(_)) {
                token = token.max("<!DOCTYPE".len());
                previous = range.end;
                continue;
            }
            let length = if matches!(event, Event::Declaration(_)) {
                range.len() + label - "UTF-8".len()
            } else {
                range.len()
            };
            token = token.max(length);
            previous = previous.max(range.end);
        }
        // Gaps are lexical whitespace outside the root and count as tokens too.
        token = token.max(canonical.len() - previous);
        Self {
            canonical,
            reference,
            document,
            total: canonical.len() - bom + label - "UTF-8".len(),
            token,
            generated,
        }
    }

    fn range(&self, range: Range<usize>) -> Range<usize> {
        if let Some(reference) = self.reference {
            reference.range(range)
        } else {
            range
        }
    }

    fn event(&self, actual: &Event<'_>, expected: Event<'_>) {
        if let Some(reference) = self.reference {
            reference.event(actual, expected);
        } else {
            assert!(
                *actual == expected,
                "explicit decode policy changed typed event"
            );
        }
    }

    fn check_result(
        &self,
        result: Result<(), yaxml::Error>,
        limits: DecodeLimits,
        input_len: usize,
    ) {
        let total_exceeded = limits.max_decoded_bytes < self.total;
        let token_exceeded = limits.max_decoded_token_bytes < self.token;
        match result {
            Ok(()) => assert!(
                !total_exceeded && !token_exceeded,
                "explicit decoded ceiling ignored"
            ),
            Err(error) => {
                // Above 1024 bytes, cumulative namespace/string copying may
                // exhaust the default owned budget before the selected quota.
                let legitimate = match error.kind {
                    ErrorKind::LimitExceeded(Limit::DecodedBytes) => total_exceeded,
                    ErrorKind::LimitExceeded(Limit::DecodedTokenBytes) => {
                        token_exceeded || self.generated
                    }
                    ErrorKind::LimitExceeded(Limit::OwnedBytes) => input_len > 1024,
                    _ => false,
                };
                assert!(
                    legitimate,
                    "valid input rejected under explicit decode policy: {error}"
                );
            }
        }
    }

    fn owned(&self, owned: &OwnedDocument, input_len: usize) {
        match (owned.declaration(), self.document.declaration()) {
            (Some(actual), Some(expected)) => {
                assert_eq!(actual.version, expected.version);
                assert_eq!(actual.standalone, expected.standalone);
                let encoding = self
                    .reference
                    .map_or(expected.encoding, |reference| reference.declared.as_deref());
                assert_eq!(actual.encoding, encoding);
            }
            (None, None) => {}
            _ => panic!("explicit decode policy changed declaration"),
        }
        let mut root = true;
        super::owned::check_nodes(owned, self.document, |range| {
            if std::mem::take(&mut root) {
                0..input_len
            } else {
                self.range(range)
            }
        });
    }
}

fn decode_stream(
    input: &[u8],
    limits: DecodeLimits,
    oracle: Option<&DecodeOracle<'_>>,
    chunk_size: usize,
) -> Result<(), yaxml::Error> {
    let mut parser = PushParser::with_decode_limits(Limits::default(), limits);
    let mut reader = oracle.map(|oracle| {
        Reader::with_limits(
            oracle.canonical.as_bytes(),
            reference_limits(oracle.canonical),
        )
        .unwrap()
    });
    let mut latest = None;
    let mut emit = |event: &Event<'_>, origin: yaxml::SourceOrigin| {
        let range = super::span(origin);
        if let Some(reader) = &mut reader {
            let expected = reader
                .read_event()
                .unwrap()
                .expect("extra explicit-policy event");
            let oracle = oracle.unwrap();
            oracle.event(event, expected);
            assert_eq!(
                range,
                oracle.range(super::span(reader.event_origin().unwrap()))
            );
        }
        latest = Some(range);
        ControlFlow::Continue(())
    };
    let mut result = Ok(());
    let mut previous = 0;
    for (index, chunk) in input.chunks(chunk_size).enumerate() {
        result = parser.feed(chunk, &mut emit);
        assert!((previous..=(index * chunk_size + chunk.len())).contains(&parser.position()));
        previous = parser.position();
        if result.is_err() {
            break;
        }
    }
    if result.is_ok() {
        result = parser.finish(&mut emit);
    }
    assert!(parser.position() <= input.len());
    if let Some(range) = latest {
        assert_eq!(super::span(parser.event_origin().unwrap()), range);
    }
    match result {
        Ok(()) => {
            assert!(
                oracle.is_some(),
                "explicit decode policy accepted invalid XML"
            );
            if let Some(mut reader) = reader {
                assert!(
                    reader.read_event().unwrap().is_none(),
                    "missing explicit-policy event"
                );
            }
            assert_eq!(parser.position(), input.len());
            parser.finish(|_, _| panic!("event after EOF")).unwrap();
            let error = parser
                .feed(&[], |_, _| panic!("event after EOF"))
                .unwrap_err();
            assert_eq!(error.kind, ErrorKind::InvalidState);
            assert_eq!(parser.finish(|_, _| panic!()).unwrap_err(), error);
        }
        Err(error) => {
            super::check_error(error, input);
            assert_eq!(parser.finish(|_, _| panic!()).unwrap_err(), error);
            assert_eq!(parser.feed(&[], |_, _| panic!()).unwrap_err(), error);
        }
    }
    result
}

fn decode_profile(input: &[u8], limits: DecodeLimits, oracle: Option<&DecodeOracle<'_>>) {
    let pull = super::pull::with_limits(input, Limits::default(), limits);
    if let Some(oracle) = oracle {
        oracle.check_result(pull, limits, input.len());
    }
    let result = decode_stream(input, limits, oracle, input.len().max(1));
    let split = decode_stream(input, limits, oracle, 3.max(input.len().div_ceil(4096)));
    assert_eq!(
        result.is_ok(),
        split.is_ok(),
        "decoded ceilings depend on chunk boundaries"
    );
    if let Some(oracle) = oracle {
        oracle.check_result(result, limits, input.len());
        oracle.check_result(split, limits, input.len());
    }
    match OwnedDocument::with_decode_limits(input, Limits::default(), limits) {
        Ok(owned) => {
            assert!(result.is_ok(), "owned ignored a decoded ceiling");
            if let Some(oracle) = oracle {
                oracle.check_result(Ok(()), limits, input.len());
                oracle.owned(&owned, input.len());
            }
        }
        Err(error) => {
            super::check_error(error, input);
            if let Some(oracle) = oracle {
                oracle.check_result(Err(error), limits, input.len());
            }
        }
    }
}

#[cfg(test)]
mod required_declaration_tests {
    use super::check_required_declaration;
    use yaxml::{Error, ErrorKind};

    #[test]
    fn rejection_oracle_accepts_the_exact_kind_and_source_offset() {
        let expected = Error {
            kind: ErrorKind::EncodingMismatch,
            offset: 0,
        };
        check_required_declaration(Err(expected), expected);
    }

    #[test]
    #[should_panic(expected = "BOM-less UTF-16 declaration rejection changed")]
    fn rejection_oracle_rejects_a_shifted_source_offset() {
        check_required_declaration(
            Err(Error {
                kind: ErrorKind::EncodingMismatch,
                offset: 2,
            }),
            Error {
                kind: ErrorKind::EncodingMismatch,
                offset: 0,
            },
        );
    }
}
