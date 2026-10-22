use super::*;

#[test]
fn attribute_values_follow_libxml2_lexical_rules() {
    use AttributeKind::*;
    // (type, valid values, invalid values)
    let table: &[(AttributeKind, &[&str], &[&str])] = &[
        (Cdata, &["", " x  y "], &[]),
        (Id, &["a", "_:a.b-1"], &["", "1a", "a b", " a"]),
        (IdRef, &["a"], &["", "-a"]),
        (Entity, &["e"], &["e f"]),
        (Notation, &["n"], &["n "]),
        (IdRefs, &["a", "a b", "a  b"], &["", " a", "a ", "a 1"]),
        (Entities, &["e f"], &["e\tf"]),
        (NmToken, &["1", "-.a"], &["", "a b", "a\u{0}"]),
        (NmTokens, &["1 2", " 1", "\t1  2 "], &["", " ", "1 +"]),
        (Enumeration, &["x", "x y"], &["", "+"]),
    ];
    for &(kind, valid, invalid) in table {
        for value in valid {
            assert!(value_matches(kind, value), "{kind:?} {value:?}");
        }
        for value in invalid {
            assert!(!value_matches(kind, value), "{kind:?} {value:?}");
        }
    }
}

#[test]
fn diagnostics_without_errors_are_valid() {
    let warning = Diagnostic::<usize> {
        code: Code::AttributeRedefined,
        node: None,
        offset: None,
        message: String::new(),
    };
    let error = Diagnostic {
        code: Code::ContentModel,
        ..warning.clone()
    };
    assert!(is_valid::<usize>(&[]));
    assert!(is_valid(std::slice::from_ref(&warning)));
    assert!(!is_valid(&[warning, error]));
}
