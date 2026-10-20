use yaxml::{ErrorKind, Limits, Reader};

#[test]
fn slice_failures_preserve_consumed_positions_and_early_structural_checks() {
    for (input, kind, position) in [
        ("<r a=x/>", ErrorKind::Syntax, 0),
        ("<r></x>", ErrorKind::MismatchedEndTag, 3),
        ("<r>&bad;</r>", ErrorKind::UnknownEntity, 8),
        ("<r><!--a--b--></r>", ErrorKind::Syntax, 3),
        ("<?xml?><r/>", ErrorKind::InvalidDeclaration, 7),
        ("<?a:b?><r/>", ErrorKind::InvalidName, 0),
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
    let limits = {
        let mut cfg = Limits::default();
        cfg.max_token_bytes = 0;
        cfg
    };
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
