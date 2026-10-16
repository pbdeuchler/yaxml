use super::*;

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

#[test]
fn owned_state_outlives_temporary_tokens_and_events_outlive_scope_removal() {
    let mut state = ParserState::new(Limits::default(), 0);
    let length;
    {
        let token = String::from("<p:r xmlns:p='urn:p'>");
        length = token.len();
        let event = owned(&mut state, TokenKind::Start, &token, 0).unwrap();
        assert!(
            matches!(event, Some(Event::Start(element)) if element.name.namespace.as_deref() == Some("urn:p"))
        );
    }
    let token = String::from("</p:r>");
    let event = owned(&mut state, TokenKind::End, &token, length).unwrap();
    assert!(state.stack.is_empty());
    assert!(state.bindings.is_empty());
    assert!(state.namespace_index.is_empty());
    assert!(state.namespace_ids.is_empty());
    assert!(matches!(event, Some(Event::End(name)) if name.namespace.as_deref() == Some("urn:p")));
    state.validate_finish(length + token.len()).unwrap();
}

#[test]
fn interner_reference_counts_include_aliases_and_shadowed_bindings() {
    let mut state = ParserState::new(Limits::default(), 0);
    let root = "<r xmlns:p='urn:a' xmlns:q='urn:a'>";
    owned(&mut state, TokenKind::Start, root, 0).unwrap();
    assert_eq!(state.namespace_ids.len(), 1);
    assert_eq!(state.namespace_ids.get("urn:a").unwrap().bindings, 2);
    let child = "<p:c xmlns:p='urn:b' xmlns:q='urn:a' p:a='' q:a=''/>";
    let event = owned(&mut state, TokenKind::Start, child, root.len()).unwrap();
    assert_eq!(state.namespace_ids.len(), 1);
    assert_eq!(state.namespace_ids.get("urn:a").unwrap().bindings, 2);
    assert!(!state.namespace_ids.contains_key("urn:b"));
    assert!(
        matches!(event, Some(Event::Empty(element)) if element.name.namespace.as_deref() == Some("urn:b") && element.attributes[2].name.namespace.as_deref() == Some("urn:b") && element.attributes[3].name.namespace.as_deref() == Some("urn:a"))
    );
    let sibling = "<p:c/>";
    assert!(
        matches!(owned(&mut state, TokenKind::Start, sibling, root.len() + child.len()).unwrap(), Some(Event::Empty(element)) if element.name.namespace.as_deref() == Some("urn:a"))
    );
    owned(
        &mut state,
        TokenKind::End,
        "</r>",
        root.len() + child.len() + sibling.len(),
    )
    .unwrap();
    assert!(state.namespace_ids.is_empty());
    assert!(state.namespace_index.is_empty());
    assert_eq!(state.next_namespace, 2);
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
    ] {
        validate_encoding(encoding, label, 11).unwrap();
    }
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
            Some("UTF16"),
            ErrorKind::UnsupportedEncoding,
        ),
    ] {
        assert_eq!(
            validate_encoding(encoding, label, 11),
            Err(Error::new(kind, 11))
        );
    }
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
        state.validate_finish(4),
        Err(Error::new(ErrorKind::EncodingMismatch, 4))
    );
    assert_eq!(
        owned(&mut state, TokenKind::Start, "<r/>", 0)
            .unwrap_err()
            .kind,
        ErrorKind::EncodingMismatch
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
    state.set_encoding(SourceEncoding::Utf8, 0, 3);
    assert_eq!(
        owned(&mut state, TokenKind::Start, "<r/>", 0)
            .unwrap_err()
            .kind,
        ErrorKind::LimitExceeded(Limit::DecodedTokenBytes)
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
        if raw.is_empty() {
            assert!(result.unwrap().is_none());
        } else {
            assert_eq!(result.unwrap_err().kind, ErrorKind::Syntax);
        }
    }
    for (kind, expected) in [
        (TokenKind::Dtd, ErrorKind::UnsupportedDtd),
        (TokenKind::InvalidMarkup, ErrorKind::Syntax),
    ] {
        let mut state = ParserState::new(Limits::default(), 0);
        assert_eq!(owned(&mut state, kind, "<!", 0).unwrap_err().kind, expected);
    }
}
