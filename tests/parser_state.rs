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
    let limits = Limits {
        max_token_bytes: 0,
        ..Limits::default()
    };
    let mut reader = Reader::with_limits(b"<!DOCTYPE", limits).unwrap();
    assert_eq!(
        reader.read_event().unwrap_err().kind,
        ErrorKind::LimitExceeded(yaxml::Limit::TokenBytes)
    );
}
