use super::*;
use crate::dtd::{DtdLimits, DtdUnit, DtdUnitView, ElementContentView};

fn compile(model: &str, budget: usize) -> Result<Model, TooComplex> {
    let unit = DtdUnit::parse(format!("<!ELEMENT r {model}>"), DtdLimits::STANDALONE).unwrap();
    let DtdUnitView::Element(element) = unit.view() else {
        unreachable!()
    };
    let ElementContentView::Children(tokens) = element.content() else {
        unreachable!()
    };
    let mut budget = budget;
    Model::compile(tokens, &mut budget)
}

fn accepts(model: &Model, children: &str) -> bool {
    children
        .split_whitespace()
        .try_fold(0, |state, name| model.step(state, name))
        .is_some_and(|state| model.accepts(state))
}

#[test]
fn glushkov_automata_match_children_sequences() {
    // (model, accepted sequences, rejected sequences)
    let table: &[(&str, &[&str], &[&str])] = &[
        ("(a)", &["a"], &["", "a a", "b"]),
        ("(a?)", &["", "a"], &["a a"]),
        ("(a*)", &["", "a", "a a a"], &["b"]),
        ("(a+)", &["a", "a a"], &[""]),
        ("(a,b)", &["a b"], &["a", "b a", "a b b"]),
        ("(a|b)", &["a", "b"], &["", "a b"]),
        (
            "(a,(b|c)*,d?)",
            &["a", "a d", "a b c b d"],
            &["d", "a d d", "a d b"],
        ),
        ("((a,b)+,c)", &["a b c", "a b a b c"], &["a c", "a b", "c"]),
        ("(a*,b*)*", &["", "b a", "a b b a"], &["c"]),
        ("(p:a,a)", &["p:a a"], &["a p:a"]),
        ("(((a)))*", &["", "a a"], &["b"]),
    ];
    for &(source, accepted, rejected) in table {
        let model = compile(source, usize::MAX).unwrap();
        assert!(model.deterministic(), "{source}");
        assert!(model.bytes() > 0);
        for children in accepted {
            assert!(accepts(&model, children), "{source} accepts {children:?}");
        }
        for children in rejected {
            assert!(!accepts(&model, children), "{source} rejects {children:?}");
        }
    }
}

#[test]
fn nondeterministic_models_match_libxml2() {
    // Every row was checked with xmllint --valid (Content model ... is not deterministic).
    for (source, deterministic) in [
        ("(a|a)", false),
        ("(a?,a?)", false),
        ("((a,b)*,a)", false),
        ("((a|b)*,a)", false),
        ("((a,b)?,a)", false),
        ("(a|(a,b))", false),
        ("((a*,b)*,b)", false),
        ("(a*)*", true),
        ("(a+)+", true),
        ("(a,(b|c)*)", true),
        ("(a?,b?)*", true),
        ("(a*|b*)*", true),
    ] {
        let model = compile(source, usize::MAX).unwrap();
        assert_eq!(model.deterministic(), deterministic, "{source}");
        if !deterministic {
            assert_eq!(model.step(0, "a"), None, "{source}");
        }
    }
}

#[test]
fn construction_spends_a_bounded_budget() {
    let chain: Vec<_> = (0..64).map(|index| format!("e{index}?")).collect();
    let chain = format!("({})", chain.join(","));
    assert_eq!(compile(&chain, 100).unwrap_err(), TooComplex);
    let mut budget = 1 << 20;
    let unit = DtdUnit::parse(format!("<!ELEMENT r {chain}>"), DtdLimits::STANDALONE).unwrap();
    let DtdUnitView::Element(element) = unit.view() else {
        unreachable!()
    };
    let ElementContentView::Children(tokens) = element.content() else {
        unreachable!()
    };
    let model = Model::compile(tokens, &mut budget).unwrap();
    assert!(budget < 1 << 20 && budget > 0);
    assert!(accepts(&model, "e0 e63") && !accepts(&model, "e63 e0"));
}
