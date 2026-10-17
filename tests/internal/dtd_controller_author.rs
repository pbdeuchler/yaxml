use super::*;
use crate::Limits;
use crate::parser::ParserState;

#[test]
fn no_subset_retains_the_real_end_delimiter_and_body_suffix() {
    let mut state = ParserState::new(Limits::default(), 0);
    let mut controller =
        DtdController::new(SourceEncoding::Utf8, 10, ControllerLimits::default(), false);
    assert!(controller.current().is_none());
    assert_eq!(
        controller
            .advance(b"<!DOCTYPE r><r/>", state.owned_budget())
            .unwrap(),
        12
    );
    let (event, origin) = controller.current().unwrap();
    assert!(matches!(event,DtdEvent::Begin(header) if header.root_name()=="r"));
    assert_eq!(origin.input_range(), Some(10..21));
    controller.release();
    assert!(matches!(controller.current().unwrap(), (DtdEvent::End, _)));
    assert_eq!(controller.current().unwrap().1.input_range(), Some(21..22));
    controller.release();
    assert!(controller.is_complete());
    assert_eq!(controller.doctype_range(), Some(10..22));
    assert_eq!(controller.physical_position(), 22);
    assert!(controller.counters().work > 12);
    assert_eq!(
        controller.advance(b"<r/>", state.owned_budget()).unwrap(),
        0
    );
}

#[test]
fn parameter_declarations_are_expanded_in_order_and_mutate_catalog_safely() {
    let input = b"<!DOCTYPE r [<!ENTITY % p \"<!ENTITY g 'v'>\">%p;]><r/>";
    let mut state = ParserState::new(Limits::default(), 0);
    let mut controller =
        DtdController::new(SourceEncoding::Utf8, 0, ControllerLimits::default(), false);
    let mut cursor = 0;
    let mut items = Vec::new();
    while !controller.is_complete() {
        cursor += controller
            .advance(&input[cursor..], state.owned_budget())
            .unwrap();
        let (event, origin) = controller.current().expect("event");
        if let DtdEvent::Item { unit, literals } = event {
            items.push((unit.raw().to_owned(), literals, origin.input_range()));
        }
        controller.release();
    }
    assert_eq!(
        items,
        vec![
            (
                "<!ENTITY % p \"<!ENTITY g 'v'>\">".into(),
                LiteralSource::Physical,
                Some(13..44)
            ),
            ("<!ENTITY g 'v'>".into(), LiteralSource::Replacement, None)
        ]
    );
    assert_eq!(&input[cursor..], b"<r/>");
    assert!(matches!(
        controller.catalog().lookup_general("g"),
        EntityLookup::Internal {
            replacement: "v",
            ..
        }
    ));
    assert_eq!(controller.counters().reference_attempts, 1);
    controller.finish_step(state.owned_budget()).unwrap();
}
