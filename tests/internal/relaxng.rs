use super::pattern::{EMPTY, NOT_ALLOWED, NameClass, Pat, Store, TEXT, Tables};

#[test]
fn choices_are_canonical_sets() {
    let mut store = Store::new(Default::default());
    let a = store.make(Pat::Value(0));
    let b = store.make(Pat::List(TEXT));
    let ab = store.choice(a, b);
    let cases = [
        (store.choice(b, a), ab),
        (store.choice(ab, a), ab),
        (store.choice(a, NOT_ALLOWED), a),
        (store.choice(NOT_ALLOWED, b), b),
        (store.choice(a, a), a),
    ];
    for (found, expected) in cases {
        assert_eq!(found, expected);
    }
    let three = store.choice(ab, EMPTY);
    assert_ne!(three, ab);
    assert!(store.nullable(three) && !store.nullable(ab));
}

#[test]
fn constructors_simplify_empty_and_not_allowed() {
    let mut store = Store::new(Default::default());
    let cases = [
        (store.group(EMPTY, TEXT), TEXT),
        (store.group(TEXT, NOT_ALLOWED), NOT_ALLOWED),
        (store.interleave(TEXT, EMPTY), TEXT),
        (store.interleave(NOT_ALLOWED, TEXT), NOT_ALLOWED),
        (store.after(NOT_ALLOWED, TEXT), NOT_ALLOWED),
        (store.one_or_more(EMPTY), EMPTY),
        (store.one_or_more(NOT_ALLOWED), NOT_ALLOWED),
        (store.make(Pat::List(NOT_ALLOWED)), NOT_ALLOWED),
        (store.make(Pat::Attribute(0, NOT_ALLOWED)), NOT_ALLOWED),
    ];
    for (index, (found, expected)) in cases.into_iter().enumerate() {
        assert_eq!(found, expected, "case {index}");
    }
    let group = store.group(TEXT, TEXT);
    assert_eq!(store.pat(group), Pat::Group(TEXT, TEXT));
}

#[test]
fn name_classes_contain_and_overlap() {
    let names = vec![
        NameClass::Name("".into(), "a".into()),
        NameClass::Name("urn:x".into(), "a".into()),
        NameClass::NsName("urn:x".into(), Some(1)),
        NameClass::AnyName(Some(0)),
        NameClass::Choice(0, 1),
    ];
    let tables = Tables {
        names,
        ..Tables::default()
    };
    let contains = [
        (0, "", "a", true),
        (0, "urn:x", "a", false),
        (2, "urn:x", "b", true),
        (2, "urn:x", "a", false),
        (3, "urn:y", "z", true),
        (3, "", "a", false),
        (4, "urn:x", "a", true),
        (4, "", "b", false),
    ];
    for (class, ns, local, expected) in contains {
        assert_eq!(
            tables.contains(class, ns, local),
            expected,
            "{class} {ns} {local}"
        );
    }
    let overlaps = [
        (0, 1, false),
        (0, 4, true),
        (1, 2, false),
        (2, 3, true),
        (0, 3, false),
        (3, 4, true),
    ];
    for (a, b, expected) in overlaps {
        assert_eq!(tables.overlap(a, b), expected, "{a} {b}");
        assert_eq!(tables.overlap(b, a), expected, "{b} {a}");
    }
    assert_eq!(tables.describe(0), Some("a"));
    assert_eq!(tables.describe(3), None);
}
