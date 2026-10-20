use super::*;
use crate::dtd::DtdLimits;
use crate::dtd_catalog::DtdCatalog;
use crate::dtd_controller::content::publish_replacement;

impl ParserState<'_> {
    pub(crate) fn retained_snapshot(&self) -> [(usize, usize); 4] {
        [
            (self.stack.len(), self.stack.capacity()),
            (self.bindings.len(), self.bindings.capacity()),
            (self.namespace_index.len(), self.namespace_index.capacity()),
            (self.namespace_ids.len(), self.namespace_ids.capacity()),
        ]
    }
}

fn owned<'event>(
    state: &mut ParserState<'static>,
    kind: TokenKind,
    raw: &'event str,
    offset: usize,
) -> Result<Option<Event<'event>>, Error> {
    state.process_token(
        kind,
        raw,
        offset..offset + raw.len(),
        |raw, offset, budget| budget.copy(raw, offset),
    )
}

/// A replacement-text token published as the content controller publishes
/// it for the slice reader: owned, failures at `offset`.
fn replacement(
    state: &mut ParserState<'static>,
    kind: TokenKind,
    raw: &str,
    offset: usize,
) -> Result<Option<Event<'static>>, Error> {
    let mut catalog = DtdCatalog::new(DtdLimits::STANDALONE, false);
    let origin = crate::dtd_frames::FragmentOrigin::input(offset..offset).unwrap();
    let copy = &mut |raw: &str, offset, budget: &mut Budget| budget.copy(raw, offset);
    let event = publish_replacement(state, &mut catalog, (kind, raw, offset, origin), copy)?;
    event
        .map(|event| owned_event(event, state.owned_budget(), offset))
        .transpose()
}

fn physical(source: &'static str, limits: Limits) -> ParserState<'static> {
    let mut state = ParserState::new(limits, 0);
    state
        .process_token(TokenKind::Start, source, 0..source.len(), |raw, _, _| {
            Ok(Cow::Borrowed(raw))
        })
        .unwrap();
    state
}

#[test]
fn replacement_tags_are_owned_and_inherit_physical_namespaces() {
    let source = "<p:r xmlns:p='urn:physical'>";
    let mut state = physical(source, Limits::default());
    let start = String::from("<p:child/>");
    let event = replacement(&mut state, TokenKind::Start, &start, source.len()).unwrap();
    drop(start);
    let Some(Event::Empty(element)) = event else {
        panic!("expected generated empty element");
    };
    assert_eq!(element.name.qualified(), "p:child");
    assert_eq!(element.name.namespace(), Some("urn:physical"));

    let end = String::from("</p:r>");
    let event = replacement(&mut state, TokenKind::End, &end, source.len()).unwrap();
    drop(end);
    assert!(
        matches!(event, Some(Event::End(name)) if name.qualified() == "p:r" && name.namespace() == Some("urn:physical"))
    );
}

#[test]
fn replacement_copies_are_charged_and_stop_at_markup_errors() {
    let source = "<p:r xmlns:p='urn:physical'>";
    for (kind, raw) in [(TokenKind::Start, "<p:c/>"), (TokenKind::End, "</p:r>")] {
        let mut state = physical(source, Limits::default().max_owned_bytes(3));
        let error = replacement(&mut state, kind, raw, 40).unwrap_err();
        assert_eq!(
            error,
            Error::new(ErrorKind::LimitExceeded(Limit::OwnedBytes), 40)
        );
    }
    // Recovery does not repair replacement text, which libxml2 stops at.
    let mut state = physical(source, Limits::default())
        .with_flags(crate::options::Flags(crate::options::RECOVER));
    for raw in ["</p:x>", "<!--a--b-->", "<?xml version='1.0'?>", "a]]>b"] {
        let kind = match raw.as_bytes()[1] {
            b'/' => TokenKind::End,
            b'!' => TokenKind::Comment,
            b'?' => TokenKind::Instruction,
            _ => TokenKind::Text,
        };
        assert!(replacement(&mut state, kind, raw, 40).is_err(), "{raw}");
        assert_eq!(state.element_depth(), 1);
    }
}

#[test]
fn replacement_leaves_keep_declared_line_ends_and_exact_copies() {
    let mut state = ParserState::new(Limits::default(), 0);
    owned(&mut state, TokenKind::Start, "<r>", 0).unwrap();
    let raw = String::from("a\r\n&amp;T;&#13;");
    let event = replacement(&mut state, TokenKind::Text, &raw, 3).unwrap();
    drop(raw);
    assert!(matches!(event, Some(Event::Text(value)) if value == "a\r\n&T;\r"));
    let comment = replacement(&mut state, TokenKind::Comment, "<!--a\r\nb-->", 20).unwrap();
    assert!(matches!(comment, Some(Event::Comment(value)) if value == "a\r\nb"));
    let cdata = replacement(&mut state, TokenKind::Cdata, "<![CDATA[π\r]]>", 32).unwrap();
    assert!(matches!(cdata, Some(Event::Cdata(value)) if value == "π\r"));
    let instruction = replacement(&mut state, TokenKind::Instruction, "<?π a\r?>", 50).unwrap();
    assert!(
        matches!(instruction, Some(Event::ProcessingInstruction { target, data }) if target == "π" && data == "a\r")
    );
    let tag = replacement(&mut state, TokenKind::Start, "<e a='x\r\ny'/>", 60).unwrap();
    assert!(matches!(tag, Some(Event::Empty(element)) if element.attributes[0].value == "x  y"));
    for (kind, raw, error) in [
        (
            TokenKind::Instruction,
            "<?XmL version='1.0'?>",
            ErrorKind::InvalidDeclaration,
        ),
        (TokenKind::Text, "π&missing;", ErrorKind::UnknownEntity),
    ] {
        let error_offset = Error::new(error, 40);
        assert_eq!(
            replacement(&mut state, kind, raw, 40).unwrap_err(),
            error_offset
        );
    }

    for (limit, accepted) in [("π".len(), true), ("π".len() - 1, false)] {
        let mut state = physical("<r>", Limits::default().max_owned_bytes(limit));
        let result = replacement(&mut state, TokenKind::Text, "π", 3);
        assert_eq!(result.is_ok(), accepted);
        assert_eq!(
            state.budget.remaining_owned(),
            limit - "π".len().min(limit) * usize::from(accepted)
        );
    }
}

#[test]
fn owned_state_outlives_temporary_tokens_and_events_outlive_scope_removal() {
    let mut state = ParserState::new(Limits::default(), 0);
    let length;
    {
        let token = String::from("<p:r xmlns:p='urn:p'>");
        length = token.len();
        let event = owned(&mut state, TokenKind::Start, &token, 0).unwrap();
        assert!(
            matches!(event, Some(Event::Start(element)) if element.name.namespace() == Some("urn:p"))
        );
    }
    let token = String::from("</p:r>");
    let event = owned(&mut state, TokenKind::End, &token, length).unwrap();
    assert!(state.stack.is_empty());
    assert!(state.bindings.is_empty());
    assert!(state.namespace_index.is_empty());
    assert!(state.namespace_ids.is_empty());
    assert!(matches!(event, Some(Event::End(name)) if name.namespace() == Some("urn:p")));
    state.validate_finish(length + token.len()).unwrap();
}

#[test]
fn interner_reference_counts_include_aliases_and_shadowed_bindings() {
    // Without padding the scope is searched linearly; with it, indexed.
    for pad in [0, 20] {
        let mut state = ParserState::new(Limits::default(), 0);
        let wrapper: String = (0..pad)
            .map(|n| format!(" xmlns:z{n}='urn:z{n}'"))
            .collect();
        let wrapper = format!("<w{wrapper}>");
        owned(&mut state, TokenKind::Start, &wrapper, 0).unwrap();
        let mut offset = wrapper.len();
        let count = |state: &ParserState<'_>, uri| {
            state
                .interned(uri)
                .map(|(_, count)| if state.indexed { count } else { 2 })
        };
        let root = "<r xmlns:p='urn:a' xmlns:q='urn:a'>";
        owned(&mut state, TokenKind::Start, root, offset).unwrap();
        offset += root.len();
        assert_eq!(state.indexed, pad > 0);
        assert_eq!(count(&state, "urn:a"), Some(2));
        let aliases = &state.bindings[state.bindings.len() - 2..];
        assert_eq!(aliases[0].namespace, aliases[1].namespace);
        let child = "<p:c xmlns:p='urn:b' xmlns:q='urn:a' p:a='' q:a=''/>";
        let event = owned(&mut state, TokenKind::Start, child, offset).unwrap();
        offset += child.len();
        assert_eq!(count(&state, "urn:a"), Some(2));
        assert_eq!(state.interned("urn:b"), None);
        assert!(
            matches!(event, Some(Event::Empty(element)) if element.name.namespace() == Some("urn:b") && element.attributes[2].name.namespace() == Some("urn:b") && element.attributes[3].name.namespace() == Some("urn:a"))
        );
        let sibling = "<p:c/>";
        assert!(
            matches!(owned(&mut state, TokenKind::Start, sibling, offset).unwrap(), Some(Event::Empty(element)) if element.name.namespace() == Some("urn:a"))
        );
        offset += sibling.len();
        owned(&mut state, TokenKind::End, "</r>", offset).unwrap();
        assert_eq!(state.interned("urn:a"), None);
        assert_eq!(state.next_namespace, pad + 2);
        if pad == 0 {
            assert!(state.namespace_ids.is_empty());
            assert!(state.namespace_index.is_empty());
        }
    }
}

#[test]
fn completed_sibling_namespaces_do_not_accumulate() {
    let mut state = ParserState::new(Limits::default(), 0);
    owned(&mut state, TokenKind::Start, "<r>", 0).unwrap();
    let mut offset = 3;
    let mut capacity = None;
    for index in 0..1000 {
        let token = format!("<c xmlns:p='urn:{index}'/>");
        owned(&mut state, TokenKind::Start, &token, offset).unwrap();
        offset += token.len();
        assert_eq!(state.stack.len(), 1);
        assert!(state.bindings.is_empty());
        assert!(state.namespace_ids.is_empty());
        assert!(state.namespace_index.is_empty());
        let retained_capacity = (
            state.bindings.capacity(),
            state.namespace_ids.capacity(),
            state.namespace_index.capacity(),
        );
        assert_eq!(
            *capacity.get_or_insert(retained_capacity),
            retained_capacity
        );
    }
    assert_eq!(state.next_namespace, 1000);
    owned(&mut state, TokenKind::End, "</r>", offset).unwrap();
    state.validate_finish(offset + 4).unwrap();
}

#[test]
fn retention_budget_differs_from_borrowing_without_changing_xml_semantics() {
    let limits = Limits {
        max_owned_bytes: 0,
        ..Limits::default()
    };
    let mut owned_state = ParserState::new(limits, 0);
    assert_eq!(
        owned(&mut owned_state, TokenKind::Start, "<r>", 0).unwrap_err(),
        Error::new(ErrorKind::LimitExceeded(Limit::OwnedBytes), 1)
    );
    let mut empty_state = ParserState::new(limits, 0);
    assert!(
        owned(&mut empty_state, TokenKind::Start, "<r/>", 0)
            .unwrap()
            .is_some()
    );
    empty_state.validate_finish(4).unwrap();
    let mut borrowed = ParserState::new(limits, 0);
    for (kind, token, offset) in [(TokenKind::Start, "<r>", 0), (TokenKind::End, "</r>", 3)] {
        assert!(
            borrowed
                .process_token(kind, token, offset..offset + token.len(), |raw, _, _| Ok(
                    Cow::Borrowed(raw)
                ))
                .unwrap()
                .is_some()
        );
    }
    borrowed.validate_finish(7).unwrap();
}

#[test]
fn copy_budget_is_cumulative_and_checks_overflow() {
    let mut budget = Budget::new(Limits {
        max_owned_bytes: 3,
        ..Limits::default()
    });
    assert_eq!(budget.copy("abc", 0).unwrap(), "abc");
    assert_eq!(
        budget.copy("d", 7).unwrap_err(),
        Error::new(ErrorKind::LimitExceeded(Limit::OwnedBytes), 7)
    );
    assert_eq!(budget.copy("", 9).unwrap(), "");
    budget.owned_bytes = usize::MAX;
    assert_eq!(
        budget.copy("d", 11).unwrap_err(),
        Error::new(ErrorKind::LimitExceeded(Limit::OwnedBytes), 11)
    );
}

#[test]
fn bom_whitespace_and_finish_states() {
    let mut state = ParserState::new(
        Limits {
            max_events: 0,
            ..Limits::default()
        },
        0,
    );
    state.set_initial_offset(3);
    assert_eq!(state.initial, 3);
    assert!(
        owned(&mut state, TokenKind::Text, " \r\n", 3)
            .unwrap()
            .is_none()
    );
    assert!(owned(&mut state, TokenKind::Text, "", 6).unwrap().is_none());
    assert_eq!(
        state.validate_finish(6).unwrap_err().kind,
        ErrorKind::MissingRoot
    );
    assert_eq!(
        owned(&mut state, TokenKind::Start, "<r/>", 6)
            .unwrap_err()
            .kind,
        ErrorKind::LimitExceeded(Limit::Events)
    );
    let mut state = ParserState::new(Limits::default(), 0);
    state.set_initial_offset(3);
    assert!(matches!(
        owned(
            &mut state,
            TokenKind::Instruction,
            "<?xml version='1.0'?>",
            3
        )
        .unwrap(),
        Some(Event::Declaration(_))
    ));
    owned(&mut state, TokenKind::Start, "<r>", 24).unwrap();
    assert_eq!(
        state.validate_finish(27).unwrap_err().kind,
        ErrorKind::UnexpectedEof
    );
}

#[test]
fn encoding_agreement_follows_complete_declaration_grammar() {
    use crate::encoding::SourceEncoding;
    for (encoding, label) in [
        (SourceEncoding::Utf8, Some("UTF-8")),
        (
            SourceEncoding::Utf16 {
                little_endian: true,
                bom: true,
            },
            None,
        ),
        (SourceEncoding::Utf8, Some("utf8")),
    ] {
        assert_eq!(validate_encoding(encoding, false, label, 11), Ok(encoding));
    }
    let Some(crate::encoding::Charset::Byte(table)) = crate::encoding::charset("ISO-8859-1") else {
        panic!("built-in Latin-1")
    };
    let latin1 = SourceEncoding::Byte(table);
    assert_eq!(
        validate_encoding(SourceEncoding::Utf8, true, Some("Latin1"), 11),
        Ok(latin1)
    );
    assert_eq!(
        validate_encoding(SourceEncoding::Utf8, true, None, 11),
        Ok(SourceEncoding::Utf8)
    );
    for (encoding, label, kind) in [
        (
            SourceEncoding::Utf8,
            Some("UTF-16"),
            ErrorKind::UnsupportedEncoding,
        ),
        (
            SourceEncoding::Utf16 {
                little_endian: true,
                bom: false,
            },
            None,
            ErrorKind::EncodingMismatch,
        ),
        (
            SourceEncoding::Utf8,
            Some("ISO-8859-1"),
            ErrorKind::UnsupportedEncoding,
        ),
        (
            SourceEncoding::Utf8,
            Some("Shift_JIS"),
            ErrorKind::UnsupportedEncoding,
        ),
    ] {
        assert_eq!(
            validate_encoding(encoding, false, label, 11),
            Err(Error::new(kind, 11))
        );
    }
    assert_eq!(
        validate_encoding(SourceEncoding::Utf8, true, Some("EUC-JP"), 11),
        Err(Error::new(ErrorKind::UnsupportedEncoding, 11))
    );
    let mut state = ParserState::new(Limits::default(), 0);
    state.set_encoding(
        SourceEncoding::Utf16 {
            little_endian: true,
            bom: false,
        },
        0,
        1024,
    );
    assert_eq!(
        owned(&mut state, TokenKind::Instruction, "<?pi?>", 0)
            .unwrap_err()
            .kind,
        ErrorKind::EncodingMismatch
    );
    let malformed = "<?xml version='1.0' encoding='UTF-8' standalone='maybe'?>";
    assert_eq!(
        owned(&mut state, TokenKind::Instruction, malformed, 0)
            .unwrap_err()
            .kind,
        ErrorKind::InvalidDeclaration
    );
}

#[test]
fn malformed_private_tokens_fail_without_slicing_panics() {
    for (kind, raw) in [
        (TokenKind::Start, ""),
        (TokenKind::Start, "x"),
        (TokenKind::Start, "r>"),
        (TokenKind::Start, "<r"),
        (TokenKind::End, "x>"),
        (TokenKind::End, "</"),
        (TokenKind::Comment, "<!--"),
        (TokenKind::Cdata, "<![CDATA["),
        (TokenKind::Instruction, "<?"),
    ] {
        let mut state = ParserState::new(Limits::default(), 0);
        owned(&mut state, TokenKind::Start, "<r>", 0).unwrap();
        let result = owned(&mut state, kind, raw, 3);
        match raw {
            "" => assert!(result.unwrap().is_none()),
            "<r" => assert_eq!(result.unwrap_err().kind, ErrorKind::UnexpectedEof),
            _ => assert_eq!(result.unwrap_err().kind, ErrorKind::Syntax),
        }
    }
    for (kind, expected) in [
        (TokenKind::Dtd, ErrorKind::Syntax),
        (TokenKind::InvalidMarkup, ErrorKind::Syntax),
    ] {
        let mut state = ParserState::new(Limits::default(), 0);
        assert_eq!(owned(&mut state, kind, "<!", 0).unwrap_err().kind, expected);
    }
}

#[test]
fn normalization_predicate_matches_all_flag_contexts_and_scan_boundaries() {
    use crate::escape::needs_normalization;

    let contexts = [
        (false, false, b"\r".as_slice()),
        (false, true, b"\r&".as_slice()),
        (true, true, b"\r\n\t&".as_slice()),
    ];
    let alphabet = ["a", "\r", "\n", "\t", "&", "é", "🙂"];
    let mut values = vec![String::new()];
    let mut frontier = vec![String::new()];
    for _ in 0..4 {
        let mut next = Vec::new();
        for prefix in &frontier {
            for suffix in alphabet {
                next.push(format!("{prefix}{suffix}"));
            }
        }
        values.extend(next.iter().cloned());
        frontier = next;
    }

    for (attribute, references, triggers) in contexts {
        for raw in &values {
            let expected = raw.bytes().any(|byte| triggers.contains(&byte));
            assert_eq!(
                needs_normalization(raw, attribute, references),
                expected,
                "attribute={attribute} references={references} raw={raw:?}"
            );
        }
    }

    let lengths = [0, 1, 7, 15, 16, 31, 32, 63, 64, 65, 127, 128, 129, 257];
    for (attribute, references, triggers) in contexts {
        for length in lengths {
            let safe = "x".repeat(length);
            assert!(!needs_normalization(&safe, attribute, references));
            assert!(!needs_normalization(
                &format!("é{safe}🙂"),
                attribute,
                references
            ));
            if length == 0 {
                continue;
            }
            let mut positions = [0, length / 2, length - 1];
            positions.sort_unstable();
            for position in positions
                .into_iter()
                .enumerate()
                .filter_map(|(index, value)| {
                    (index == 0 || positions[index - 1] != value).then_some(value)
                })
            {
                for sentinel in *b"\r\n\t&" {
                    let mut bytes = vec![b'x'; length];
                    bytes[position] = sentinel;
                    let raw = std::str::from_utf8(&bytes).unwrap();
                    assert_eq!(
                        needs_normalization(raw, attribute, references),
                        triggers.contains(&sentinel),
                        "attribute={attribute} references={references} length={length} position={position} sentinel={sentinel:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn delivered_attribute_lists_return_their_capacity() {
    let mut state = ParserState::new(Limits::default(), 0);
    assert_eq!(state.take_attributes().capacity(), 0);
    let mut list: Vec<Attribute<'_>> = Vec::with_capacity(8);
    list.push(Attribute {
        name: Name::new("a", None).unwrap(),
        value: Cow::Borrowed("v"),
    });
    state.recycle_attributes(list);
    // A smaller list never replaces a larger stored allocation.
    state.recycle_attributes(Vec::with_capacity(2));
    let reused = state.take_attributes();
    assert!(reused.is_empty());
    assert_eq!(reused.capacity(), 8);
    assert_eq!(state.take_attributes().capacity(), 0);
}

#[test]
fn scanned_qname_splits_agree_with_qname() {
    let alphabet: Vec<char> = (0x20u8..0x7f)
        .map(char::from)
        .chain(['\u{b7}', '\u{c0}', '\u{d7}', '\u{300}', '\u{10000}'])
        .collect();
    let mut names = Vec::new();
    for &a in &alphabet {
        for &b in &alphabet {
            names.push(format!("{a}{b}"));
            names.push(format!("{a}:{b}"));
            names.push(format!("a{a}:{b}c"));
        }
    }
    names.extend(["p:q:r".into(), ":".into(), "é:é".into(), "a:·".into()]);
    for name in names {
        let raw = format!("{name} ");
        let mut scan = TagScanner {
            raw: &raw,
            cursor: 0,
            offset: 5,
        };
        let mut plain = TagScanner {
            raw: &raw,
            cursor: 0,
            offset: 5,
        };
        let scanned = scan.scan_name();
        let named = plain.name();
        assert_eq!(
            scanned.as_ref().map(|(name, _)| *name),
            named.as_ref().copied(),
            "{name:?}"
        );
        assert_eq!(scan.cursor, plain.cursor);
        if let (Ok((scanned, split)), Ok(named)) = (scanned, named) {
            let expected = qname(named, 5);
            let actual = split_qname(scanned, split, 5);
            assert_eq!(actual, expected, "{name:?}");
            if let Some(split) = split {
                let prefix = expected.unwrap().0.map(|prefix| prefix.len());
                assert_eq!(split.map(NonZeroUsize::get), prefix, "{name:?}");
            }
        }
    }
}

#[test]
fn frame_reservation_starts_large_then_doubles_only_when_full() {
    let mut empty: Vec<u8> = Vec::new();
    reserve_frame(&mut empty, 0).unwrap();
    assert!(empty.capacity() >= FRAMES);

    let mut full = vec![0u8; FRAMES + 8];
    full.shrink_to_fit();
    let length = full.len();
    reserve_frame(&mut full, 0).unwrap();
    assert!(full.capacity() >= 2 * length);

    let mut spare: Vec<u8> = Vec::with_capacity(4);
    spare.push(1);
    reserve_frame(&mut spare, 0).unwrap();
    assert_eq!(spare.capacity(), 4, "spare room needs no reservation");
}

#[test]
fn recovery_records_well_formedness_errors_and_keeps_fatal_ones() {
    let syntax = Error::new(ErrorKind::Syntax, 1);
    let fatal = [
        ErrorKind::LimitExceeded(Limit::Depth),
        ErrorKind::Allocation,
        ErrorKind::EntityLoop,
        ErrorKind::ExternalResource,
        ErrorKind::InvalidState,
    ];
    // (recovering, error, recorded?)
    let mut cases = vec![(false, syntax, false), (true, syntax, true)];
    cases.extend(fatal.map(|kind| (true, Error::new(kind, 2), false)));
    for (recovering, error, recorded) in cases {
        let mut budget = Budget::new(Limits::default());
        budget.recovered = recovering.then(Vec::new);
        assert_eq!(budget.recovers(), recovering);
        assert_eq!(budget.recover(error).is_ok(), recorded, "{error:?}");
        let errors = Budget::conclude(Some(&mut budget), Ok(()));
        assert_eq!(errors, Ok(if recorded { vec![error] } else { vec![] }));
    }
}

#[test]
fn conclude_records_the_terminal_error_or_returns_it() {
    let end = Error::new(ErrorKind::UnexpectedEof, 9);
    let limit = Error::new(ErrorKind::LimitExceeded(Limit::Events), 9);
    let mut budget = Budget::new(Limits::default());
    // (budget, recovering, end, result)
    type Case = (bool, bool, Result<(), Error>, Result<Vec<Error>, Error>);
    let cases: [Case; 6] = [
        (true, false, Ok(()), Ok(vec![])),
        (true, false, Err(end), Err(end)),
        (true, true, Ok(()), Ok(vec![])),
        (true, true, Err(end), Ok(vec![end])),
        (true, true, Err(limit), Err(limit)),
        (false, true, Err(end), Err(end)),
    ];
    for (present, recovering, ending, expected) in cases {
        budget.recovered = recovering.then(Vec::new);
        let budget = present.then_some(&mut budget);
        assert_eq!(Budget::conclude(budget, ending), expected, "{ending:?}");
    }
}

#[test]
fn recovery_records_at_most_libxml2s_error_count() {
    let mut budget = Budget::new(Limits::default());
    budget.recovered = Some(Vec::new());
    for offset in 0..MAX_RECOVERED + 5 {
        budget
            .recover(Error::new(ErrorKind::Syntax, offset))
            .unwrap();
    }
    let errors = Budget::conclude(Some(&mut budget), Ok(())).unwrap();
    assert_eq!(errors.len(), MAX_RECOVERED);
    assert_eq!(errors.last().unwrap().offset, MAX_RECOVERED - 1);
}
