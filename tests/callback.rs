use std::ops::ControlFlow;
use std::panic::{AssertUnwindSafe, catch_unwind};

use yaxml::{ErrorKind, PushParser};

#[test]
fn recovered_callback_panics_poison_the_parser() {
    for during_finish in [false, true] {
        let mut parser = PushParser::new();
        if during_finish {
            parser
                .feed(b"<r>text", |_, _| ControlFlow::Continue(()))
                .unwrap();
        }
        let result = catch_unwind(AssertUnwindSafe(|| {
            if during_finish {
                parser.finish(|_, _| panic!("injected callback panic"))
            } else {
                parser.feed(b"<r/>", |_, _| panic!("injected callback panic"))
            }
        }));
        assert!(result.is_err());
        let error = parser
            .feed(b"", |_, _| panic!("callback after panic"))
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::InvalidState);
        assert_eq!(
            parser
                .finish(|_, _| panic!("callback after panic"))
                .unwrap_err(),
            error
        );
    }
}

#[test]
fn successful_feeds_clear_the_in_flight_panic_guard() {
    let mut parser = PushParser::new();
    for chunk in [b"<r>".as_slice(), b"text", b"</r>"] {
        parser
            .feed(chunk, |_, _| ControlFlow::Continue(()))
            .unwrap();
    }
    parser.finish(|_, _| ControlFlow::Continue(())).unwrap();
    parser.finish(|_, _| panic!("events after finish")).unwrap();
}
