use yaxml::{ErrorKind, Limits, Reader};

#[test]
fn slice_failures_preserve_consumed_positions_and_early_structural_checks() {
    for (input, kind, position) in [
        ("<r a=x/>", ErrorKind::Syntax, 0),
        ("<r></x>", ErrorKind::MismatchedEndTag, 3),
        ("<r>&bad;</r>", ErrorKind::UnknownEntity, 8),
        ("<r><!--a--b--></r>", ErrorKind::Syntax, 3),
        ("<?xml?><r/>", ErrorKind::InvalidDeclaration, 7),
        ("<?1?><r/>", ErrorKind::InvalidName, 0),
        ("<r/><s", ErrorKind::MultipleRoots, 4),
        ("<![CDATA[", ErrorKind::TextOutsideRoot, 0),
    ] {
        let mut reader = Reader::new(input.as_bytes()).unwrap();
        loop {
            match reader.read_event() {
                Ok(Some(_)) => {}
                Ok(None) => panic!("invalid input accepted: {input:?}"),
                Err(error) => {
                    assert_eq!(error.kind, kind, "{input:?}");
                    assert_eq!(reader.position(), position, "{input:?}");
                    assert_eq!(reader.read_event().unwrap_err(), error);
                    break;
                }
            }
        }
    }
    let limits = Limits::default().max_token_bytes(0);
    let mut reader = Reader::with_limits(b"<!DOCTYPE", limits).unwrap();
    assert_eq!(
        reader.read_event().unwrap_err().kind,
        ErrorKind::LimitExceeded(yaxml::Limit::TokenBytes)
    );
}

/// A token cut off by EOF fails its start check in every reader, as the
/// slice reader checks it before scanning to the token's end.
#[test]
fn streams_check_a_cut_off_token_start_like_the_slice_reader() {
    use std::ops::ControlFlow;
    for (input, kind) in [
        ("<r/><", ErrorKind::MultipleRoots),
        ("<r/><s a='", ErrorKind::MultipleRoots),
        ("<r/><![CDATA[", ErrorKind::TextOutsideRoot),
        ("<r/></", ErrorKind::UnexpectedEof),
        ("<r/><?", ErrorKind::UnexpectedEof),
        ("<r><s", ErrorKind::UnexpectedEof),
    ] {
        let mut reader = Reader::new(input.as_bytes()).unwrap();
        let slice = loop {
            match reader.read_event() {
                Ok(Some(_)) => {}
                Ok(None) => panic!("invalid input accepted: {input:?}"),
                Err(error) => break error,
            }
        };
        let mut parser = yaxml::PushParser::new();
        let pushed = parser
            .feed(input.as_bytes(), |_, _| ControlFlow::Continue(()))
            .and_then(|()| parser.finish(|_, _| ControlFlow::Continue(())))
            .unwrap_err();
        assert_eq!((slice.kind, pushed.kind), (kind, kind), "{input:?}");
        assert_eq!(slice.offset, pushed.offset, "{input:?}");
    }
}

/// A character outside `Char` in the XML declaration comes after the
/// declaration's own error, as in libxml2, on every surface and in every
/// encoding path (UTF-8 in place, invalid UTF-8 later, a declared encoding).
#[test]
fn declaration_errors_precede_its_invalid_characters() {
    use yaxml::{Document, ErrorKind::*, Event, OwnedDocument, PushParser};
    let cases: &[(&[u8], (yaxml::ErrorKind, usize))] = &[
        (
            b"<?xml version='x' a\x0b='no'?><r/>",
            (InvalidDeclaration, 6),
        ),
        (
            b"<?xml version='x' a\x0b='no'?><r/>\xff",
            (InvalidDeclaration, 6),
        ),
        (
            b"<?xml version='1.0' a\x0b='no'?><r/>",
            (InvalidDeclaration, 21),
        ),
        (
            b"<?xml version='x' encoding='ISO-8859-5' a\x0b='no'?><r/>\xff",
            (InvalidDeclaration, 6),
        ),
        // Characters past a valid declaration keep their order.
        (b"<?xml version='1.0'?><r>\x0b</r>", (InvalidCharacter, 24)),
        (b"<r>\x0b</r>", (InvalidCharacter, 3)),
        (
            b"<?xml version='1.0' encoding='bogus'?><r>\x0b</r>",
            (UnsupportedEncoding, 20),
        ),
    ];
    for &(input, expected) in cases {
        let mut parser = PushParser::new();
        let mut emit = |_: &Event, _| std::ops::ControlFlow::Continue(());
        let pushed = parser
            .feed(input, &mut emit)
            .and_then(|_| parser.finish(&mut emit));
        let errors = [
            pushed.err(),
            Document::parse_bytes(input).err(),
            OwnedDocument::parse_bytes(input).err(),
        ];
        for error in errors {
            assert_eq!(
                error.map(|e| (e.kind, e.offset)),
                Some(expected),
                "{input:?}"
            );
        }
    }
}
