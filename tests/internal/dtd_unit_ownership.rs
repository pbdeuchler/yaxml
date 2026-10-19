use crate::parser::ParserState;
use crate::{Error, ErrorKind, Limit, Limits};

use super::*;

#[test]
fn borrowed_units_outlive_their_source_and_move_validated_metadata() {
    let samples = [
        "<!DOCTYPE π SYSTEM 'a>b' [",
        "<!ELEMENT π (a,(b|c)*)>",
        "<!ATTLIST π x (1|:x) #FIXED '&amp;'>",
        "<!ENTITY % p '&#60;a/>tail'>",
        "<!NOTATION n PUBLIC 'public-only'>",
        "<!-- π -->",
        "<?π data?>",
    ];
    for raw in samples {
        let mut state = ParserState::new(
            Limits {
                max_owned_bytes: raw.len(),
                ..Limits::default()
            },
            0,
        );
        let owned = {
            let temporary = raw.to_owned();
            let unit = DtdUnit::parse(temporary.as_str(), DtdLimits::default()).unwrap();
            let tokens = unit.tokens.as_ptr();
            let owned = unit.try_into_owned(state.owned_budget(), 37).unwrap();
            assert_eq!(owned.tokens.as_ptr(), tokens);
            owned
        };
        assert!(matches!(owned.raw, Cow::Owned(_)));
        let expected = DtdUnit::parse(raw, DtdLimits::default()).unwrap();
        assert_eq!(owned.raw(), raw);
        assert_eq!(owned.view(), expected.view());
        assert_eq!(
            owned.reference_positions().collect::<Vec<_>>(),
            expected.reference_positions().collect::<Vec<_>>()
        );
        assert_eq!(state.owned_budget().remaining_owned(), 0);
    }
}

#[test]
fn already_owned_units_move_without_spending_another_string_budget() {
    let raw = String::from("<!ENTITY e '&amp;&#65;'>");
    let pointer = raw.as_ptr();
    let unit = DtdUnit::parse(raw, DtdLimits::default()).unwrap();
    let tokens = unit.tokens.as_ptr();
    let mut state = ParserState::new(
        Limits {
            max_owned_bytes: 0,
            ..Limits::default()
        },
        0,
    );
    let owned = unit.try_into_owned(state.owned_budget(), 71).unwrap();
    assert_eq!(owned.raw().as_ptr(), pointer);
    assert_eq!(owned.tokens.as_ptr(), tokens);
    assert_eq!(owned.raw(), "<!ENTITY e '&amp;&#65;'>");
    assert_eq!(state.owned_budget().remaining_owned(), 0);
}

#[test]
fn borrowed_promotion_checks_the_shared_budget_before_publication() {
    let raw = "<!ENTITY e 'π'>";
    for available in [0, raw.len() - 1] {
        let mut state = ParserState::new(
            Limits {
                max_owned_bytes: available,
                ..Limits::default()
            },
            0,
        );
        let unit = DtdUnit::parse(raw, DtdLimits::default()).unwrap();
        assert_eq!(
            unit.try_into_owned(state.owned_budget(), 43),
            Err(Error {
                kind: ErrorKind::LimitExceeded(Limit::OwnedBytes),
                offset: 43,
            })
        );
        assert_eq!(state.owned_budget().remaining_owned(), available);
    }
}
