use std::borrow::Cow;
use std::ops::ControlFlow;

use yaxml::{Error, ErrorKind, Event, Limit, Limits, PushParser, Reader};

fn read_with_limits(input: &str, max_owned_bytes: usize) -> Result<Vec<Event<'_>>, Error> {
    let mut reader = Reader::with_limits(
        input.as_bytes(),
        Limits {
            max_owned_bytes,
            ..Limits::default()
        },
    )?;
    let mut events = Vec::new();
    while let Some(event) = reader.read_event()? {
        events.push(event);
    }
    Ok(events)
}

#[derive(Clone, Copy, Debug)]
enum ValueKind {
    Attribute,
    Text,
    Cdata,
    Comment,
    Instruction,
}

fn value<'event>(events: &'event [Event<'event>], kind: ValueKind) -> &'event Cow<'event, str> {
    events
        .iter()
        .find_map(|event| match (kind, event) {
            (ValueKind::Attribute, Event::Start(element) | Event::Empty(element)) => {
                element.attributes.first().map(|attribute| &attribute.value)
            }
            (ValueKind::Text, Event::Text(value))
            | (ValueKind::Cdata, Event::Cdata(value))
            | (ValueKind::Comment, Event::Comment(value)) => Some(value),
            (ValueKind::Instruction, Event::ProcessingInstruction { data, .. }) => Some(data),
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing {kind:?} value"))
}

#[test]
fn unaffected_contexts_borrow_and_changed_contexts_own_literal_values() {
    let borrowed = read_with_limits(
        "<r a='safe'>plain\n\t<![CDATA[x&\n\t]]><!--y&\n\t--><?pi z&\n\t?></r>",
        0,
    )
    .unwrap();
    for (kind, expected) in [
        (ValueKind::Attribute, "safe"),
        (ValueKind::Text, "plain\n\t"),
        (ValueKind::Cdata, "x&\n\t"),
        (ValueKind::Comment, "y&\n\t"),
        (ValueKind::Instruction, "z&\n\t"),
    ] {
        assert!(
            matches!(value(&borrowed, kind), Cow::Borrowed(actual) if *actual == expected),
            "{kind:?}"
        );
    }

    let changed = read_with_limits(
        "<r a='\t\n\r\n&amp;'>x\r\ny&amp;z<![CDATA[a\r\nb]]><!--c\rd--><?pi e\rf?></r>",
        Limits::default().max_owned_bytes,
    )
    .unwrap();
    for (kind, expected) in [
        (ValueKind::Attribute, "   &"),
        (ValueKind::Text, "x\ny&z"),
        (ValueKind::Cdata, "a\nb"),
        (ValueKind::Comment, "c\nd"),
        (ValueKind::Instruction, "e\nf"),
    ] {
        assert!(
            matches!(value(&changed, kind), Cow::Owned(actual) if actual == expected),
            "{kind:?}"
        );
    }
}

#[test]
fn normalization_charges_raw_payload_length_at_exact_boundaries() {
    let cases = [
        ("<r a='x\r\n'/>", ValueKind::Attribute, 3, 6, "x "),
        ("<r>x\r\n</r>", ValueKind::Text, 3, 3, "x\n"),
        ("<r><![CDATA[x\r\n]]></r>", ValueKind::Cdata, 3, 12, "x\n"),
        ("<r><!--x\r\n--></r>", ValueKind::Comment, 3, 7, "x\n"),
        ("<r><?pi x\r\n?></r>", ValueKind::Instruction, 3, 8, "x\n"),
        ("<r a='&amp;'/>", ValueKind::Attribute, 5, 6, "&"),
        ("<r>&amp;</r>", ValueKind::Text, 5, 3, "&"),
    ];
    for (input, kind, raw_length, offset, expected) in cases {
        let expected_error = Error {
            kind: ErrorKind::LimitExceeded(Limit::OwnedBytes),
            offset,
        };
        assert_eq!(
            read_with_limits(input, raw_length - 1).unwrap_err(),
            expected_error,
            "{input:?}"
        );
        let events = read_with_limits(input, raw_length).unwrap();
        assert!(
            matches!(value(&events, kind), Cow::Owned(actual) if actual == expected),
            "{input:?}"
        );
    }
}

#[test]
fn normalization_budget_precedes_reference_semantics() {
    let cases = [
        ("<r>x&bad;</r>", 6, 3, ErrorKind::UnknownEntity, 4),
        ("<r a='x&bad;'/>", 6, 6, ErrorKind::UnknownEntity, 7),
        ("<r>x&#0;</r>", 5, 3, ErrorKind::InvalidReference, 4),
        ("<r a='x&#0;'/>", 5, 6, ErrorKind::InvalidReference, 7),
    ];
    for (input, raw_length, value_offset, semantic_kind, semantic_offset) in cases {
        let budget_error = Error {
            kind: ErrorKind::LimitExceeded(Limit::OwnedBytes),
            offset: value_offset,
        };
        let semantic_error = Error {
            kind: semantic_kind,
            offset: semantic_offset,
        };
        assert_eq!(
            read_with_limits(input, raw_length - 1).unwrap_err(),
            budget_error,
            "{input:?}"
        );
        assert_eq!(
            read_with_limits(input, raw_length).unwrap_err(),
            semantic_error,
            "{input:?}"
        );
    }
}

#[derive(Clone, Copy, Debug)]
enum Encoding {
    Utf8,
    Utf16Little,
    Utf16Big,
}

impl Encoding {
    fn encode(self, input: &str) -> Vec<u8> {
        match self {
            Self::Utf8 => input.as_bytes().to_vec(),
            Self::Utf16Little | Self::Utf16Big => std::iter::once(0xfeff)
                .chain(input.encode_utf16())
                .flat_map(|unit| {
                    if matches!(self, Self::Utf16Little) {
                        unit.to_le_bytes()
                    } else {
                        unit.to_be_bytes()
                    }
                })
                .collect(),
        }
    }
}

fn push_result(input: &[u8], split: usize, max_owned_bytes: usize) -> Result<(), Error> {
    let mut parser = PushParser::with_limits(Limits {
        max_owned_bytes,
        ..Limits::default()
    });
    let mut emit = |_: Event<'_>, _: yaxml::SourceOrigin| ControlFlow::Continue(());
    parser.feed(&input[..split], &mut emit)?;
    parser.feed(&input[split..], &mut emit)?;
    parser.finish(&mut emit)
}

#[test]
fn streaming_offsets_and_validation_precedence_survive_normalization_scans() {
    let reference_input = "<r>é🙂\r\n&bad;</r>";
    let invalid_character_input = "<r>&bad;\u{1}</r>";
    for (encoding, text_offset, reference_offset, invalid_offset) in [
        (Encoding::Utf8, 3, 11, 8),
        (Encoding::Utf16Little, 8, 18, 18),
        (Encoding::Utf16Big, 8, 18, 18),
    ] {
        let reference = encoding.encode(reference_input);
        let invalid_character = encoding.encode(invalid_character_input);
        let mut reference_splits = [0, 1, reference.len() / 2, reference.len()];
        reference_splits.sort_unstable();
        for split in reference_splits
            .into_iter()
            .enumerate()
            .filter_map(|(index, value)| {
                (index == 0 || reference_splits[index - 1] != value).then_some(value)
            })
        {
            assert_eq!(
                push_result(&reference, split, 13).unwrap_err(),
                Error {
                    kind: ErrorKind::LimitExceeded(Limit::OwnedBytes),
                    offset: text_offset,
                },
                "{encoding:?} split {split}"
            );
            assert_eq!(
                push_result(&reference, split, 14).unwrap_err(),
                Error {
                    kind: ErrorKind::UnknownEntity,
                    offset: reference_offset,
                },
                "{encoding:?} split {split}"
            );
        }
        let mut invalid_splits = [0, 1, invalid_character.len() / 2, invalid_character.len()];
        invalid_splits.sort_unstable();
        for split in invalid_splits
            .into_iter()
            .enumerate()
            .filter_map(|(index, value)| {
                (index == 0 || invalid_splits[index - 1] != value).then_some(value)
            })
        {
            assert_eq!(
                push_result(&invalid_character, split, 1).unwrap_err(),
                Error {
                    kind: ErrorKind::InvalidCharacter,
                    offset: invalid_offset,
                },
                "{encoding:?} split {split}"
            );
        }
    }
}
