use super::*;
use crate::dtd::{DtdLimits, LiteralSource};
use crate::dtd_catalog::DtdCatalog;
use crate::dtd_tag::prepare_dtd_tag;

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

fn generated_start_with_short_source(source: &str) -> Event<'static> {
    let mut state = ParserState::new(Limits::default(), 0);
    state
        .process_token(TokenKind::Start, source, 0..source.len(), |raw, _, _| {
            Ok(Cow::Borrowed(raw))
        })
        .unwrap();
    let mut catalog = DtdCatalog::new(DtdLimits::STANDALONE, false);
    let generated = String::from("<p:child/>");
    state
        .process_dtd_start_owned(&generated, source.len(), &mut catalog)
        .unwrap()
}

#[test]
fn generated_start_owns_names_and_an_inherited_borrowed_namespace() {
    let physical = String::from("<p:root xmlns:p='urn:physical'>");
    let event = generated_start_with_short_source(&physical);
    drop(physical);
    let Event::Empty(element) = event else {
        panic!("expected generated empty element");
    };
    assert_eq!(element.name.qualified(), "p:child");
    assert_eq!(element.name.namespace(), Some("urn:physical"));

    let mut state = ParserState::new(Limits::default(), 0);
    let raw = state.collect_raw_tag("<p:child/>", 11).unwrap();
    let prepared = prepare_dtd_tag(
        &mut DtdCatalog::new(DtdLimits::STANDALONE, false),
        state.owned_budget(),
        raw,
        LiteralSource::Physical,
        false,
    )
    .unwrap();
    assert_eq!(
        prepared.try_into_static().unwrap_err(),
        Error::new(ErrorKind::InvalidState, 12)
    );
}

#[test]
fn generated_namespace_copy_limit_fails_before_publication() {
    let limits = Limits {
        max_owned_bytes: 3,
        ..Limits::default()
    };
    let source = "<p:r xmlns:p='urn:physical'>";
    let mut state = ParserState::new(limits, 0);
    state
        .process_token(TokenKind::Start, source, 0..source.len(), |raw, _, _| {
            Ok(Cow::Borrowed(raw))
        })
        .unwrap();
    let mut catalog = DtdCatalog::new(DtdLimits::STANDALONE, false);
    let start = source.len();
    let error = state
        .process_dtd_start_owned("<p:c/>", start, &mut catalog)
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::LimitExceeded(Limit::OwnedBytes));
    assert_eq!(error.offset, Some(start + 1));
    assert_eq!(state.element_depth(), 1);
    assert_eq!(state.events, 1);
}

#[test]
fn generated_end_is_owned_and_failures_cannot_pop_the_current_frame() {
    fn close(source: &str) -> Event<'static> {
        let mut state = ParserState::new(Limits::default(), 0);
        state
            .process_token(TokenKind::Start, source, 0..source.len(), |raw, _, _| {
                Ok(Cow::Borrowed(raw))
            })
            .unwrap();
        state.process_dtd_end_owned("</p:r>", source.len()).unwrap()
    }

    let source = String::from("<p:r xmlns:p='urn:physical'>");
    let event = close(&source);
    drop(source);
    assert!(
        matches!(event, Event::End(name) if name.qualified() == "p:r" && name.namespace() == Some("urn:physical"))
    );

    let source = "<p:r xmlns:p='urn:physical'>";
    let mut state = ParserState::new(
        Limits {
            max_owned_bytes: 0,
            ..Limits::default()
        },
        0,
    );
    state
        .process_token(TokenKind::Start, source, 0..source.len(), |raw, _, _| {
            Ok(Cow::Borrowed(raw))
        })
        .unwrap();
    let events = state.events;
    let error = state
        .process_dtd_end_owned("</p:r>", source.len())
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::LimitExceeded(Limit::OwnedBytes));
    assert_eq!(error.offset, source.len() + 2);
    assert_eq!(state.element_depth(), 1);
    assert_eq!(state.events, events);

    let error = state
        .process_dtd_end_owned("</p:x>", source.len())
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::MismatchedEndTag);
    assert_eq!(state.element_depth(), 1);

    let mut state = ParserState::new(
        Limits {
            max_owned_bytes: "urn:physical".len(),
            ..Limits::default()
        },
        0,
    );
    state
        .process_token(TokenKind::Start, source, 0..source.len(), |raw, _, _| {
            Ok(Cow::Borrowed(raw))
        })
        .unwrap();
    let error = state
        .process_dtd_end_owned("</p:r>", source.len())
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::LimitExceeded(Limit::OwnedBytes));
    assert_eq!(error.offset, source.len() + 2);
    assert_eq!(state.element_depth(), 1);
    assert_eq!(state.events, 1);
    assert_eq!(state.budget.remaining_owned(), 0);
}

#[test]
fn generated_leaves_preserve_replacement_eol_and_do_not_rescan_final_data() {
    let mut state = ParserState::new(Limits::default(), 0);
    owned(&mut state, TokenKind::Start, "<r>", 0).unwrap();
    let raw = String::from("a\r\n&amp;T;&#13;");
    let event = state
        .process_dtd_leaf_owned(DtdLeafKind::Text, &raw, 3)
        .unwrap();
    drop(raw);
    assert!(matches!(event, Event::Text(value) if value == "a\r\n&T;\r"));

    let comment = state
        .process_dtd_leaf_owned(DtdLeafKind::Comment, "<!--a\r\nb-->", 20)
        .unwrap();
    assert!(matches!(comment, Event::Comment(value) if value == "a\r\nb"));

    let cdata = state
        .process_dtd_leaf_owned(DtdLeafKind::Cdata, "<![CDATA[π\r]]>", 32)
        .unwrap();
    assert!(matches!(cdata, Event::Cdata(value) if value == "π\r"));

    let instruction = state
        .process_dtd_leaf_owned(DtdLeafKind::ProcessingInstruction, "<?π a\r?>", 50)
        .unwrap();
    assert!(
        matches!(instruction, Event::ProcessingInstruction { target, data } if target == "π" && data == "a\r")
    );

    let before = state.events;
    let error = state
        .process_dtd_leaf_owned(
            DtdLeafKind::ProcessingInstruction,
            "<?XmL version='1.0'?>",
            40,
        )
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::InvalidDeclaration);
    assert_eq!(error.offset, 40);
    assert_eq!(state.events, before);

    let before = state.events;
    let error = state
        .process_dtd_leaf_owned(DtdLeafKind::Text, "π&missing;", 90)
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::UnknownEntity);
    assert_eq!(error.offset, 92);
    assert_eq!(state.events, before);
}

#[test]
fn generated_text_rejects_prolog_and_epilog_before_spending_resources() {
    for (raw, prolog_offset) in [("", 7), (" \n", 7), ("x", 7), (" x", 8)] {
        let mut state = ParserState::new(Limits::default(), 0);
        let before = state.budget.remaining_owned();
        let error = state
            .process_dtd_leaf_owned(DtdLeafKind::Text, raw, 7)
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::TextOutsideRoot);
        assert_eq!(error.offset, prolog_offset);
        assert_eq!(state.budget.remaining_owned(), before);
        assert_eq!(state.events, 0);

        owned(&mut state, TokenKind::Start, "<r/>", 0).unwrap();
        let before = state.budget.remaining_owned();
        let events = state.events;
        let error = state
            .process_dtd_leaf_owned(DtdLeafKind::Text, raw, 11)
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::TextOutsideRoot);
        assert_eq!(error.offset, prolog_offset + 4);
        assert_eq!(state.budget.remaining_owned(), before);
        assert_eq!(state.events, events);
    }
}

#[test]
fn generated_leaf_copy_quota_is_exact_for_unicode_bytes() {
    let mut exact = ParserState::new(
        Limits {
            max_owned_bytes: "π".len(),
            ..Limits::default()
        },
        0,
    );
    exact
        .process_token(TokenKind::Start, "<r>", 0..3, |raw, _, _| {
            Ok(Cow::Borrowed(raw))
        })
        .unwrap();
    assert!(matches!(
        exact
            .process_dtd_leaf_owned(DtdLeafKind::Text, "π", 3)
            .unwrap(),
        Event::Text(value) if value == "π"
    ));
    assert_eq!(exact.budget.remaining_owned(), 0);

    let mut short = ParserState::new(
        Limits {
            max_owned_bytes: "π".len() - 1,
            ..Limits::default()
        },
        0,
    );
    short
        .process_token(TokenKind::Start, "<r>", 0..3, |raw, _, _| {
            Ok(Cow::Borrowed(raw))
        })
        .unwrap();
    let error = short
        .process_dtd_leaf_owned(DtdLeafKind::Text, "π", 3)
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::LimitExceeded(Limit::OwnedBytes));
    assert_eq!(error.offset, 3);
    assert_eq!(short.events, 1);
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
    let mut budget = Budget {
        limits: Limits {
            max_owned_bytes: 3,
            ..Limits::default()
        },
        owned_bytes: 0,
        recovered: None,
        kept: Vec::new(),
    };
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
        (
            SourceEncoding::Utf16 {
                little_endian: false,
                bom: true,
            },
            Some("uTf-16"),
        ),
        (
            SourceEncoding::Utf16 {
                little_endian: true,
                bom: false,
            },
            Some("UTF-16LE"),
        ),
        (
            SourceEncoding::Utf16 {
                little_endian: false,
                bom: false,
            },
            Some("utf-16be"),
        ),
        (
            SourceEncoding::Utf16 {
                little_endian: true,
                bom: true,
            },
            Some("UTF16"),
        ),
        (
            SourceEncoding::Utf16 {
                little_endian: false,
                bom: false,
            },
            Some("unicodeFFFE"),
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
                bom: true,
            },
            Some("UTF-8"),
            ErrorKind::EncodingMismatch,
        ),
        (
            SourceEncoding::Utf16 {
                little_endian: false,
                bom: true,
            },
            Some("UTF-16LE"),
            ErrorKind::EncodingMismatch,
        ),
        (
            SourceEncoding::Utf16 {
                little_endian: true,
                bom: false,
            },
            Some("UTF-16"),
            ErrorKind::EncodingMismatch,
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
            SourceEncoding::Utf16 {
                little_endian: true,
                bom: true,
            },
            Some("UCS-2"),
            ErrorKind::UnsupportedEncoding,
        ),
        (
            SourceEncoding::Utf16 {
                little_endian: true,
                bom: true,
            },
            Some("ISO-8859-1"),
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
