use super::*;

/// `e` and `f` are declared; other names are not.
fn declared(name: &str) -> bool {
    matches!(name, "e" | "f")
}

fn form(literal: &str, tokenized: bool, physical: bool) -> Option<String> {
    kept_form(literal, tokenized, physical, declared, |_| Ok(())).unwrap()
}

#[test]
fn kept_form_keeps_declared_references_and_resolves_the_rest() {
    // (literal, tokenized, physical, libxml2's xmlParseAttValueInternal result)
    let table: &[(&str, bool, bool, Option<&str>)] = &[
        // Nothing kept: no reference, or only character, predefined or
        // undeclared ones. No allocation.
        ("plain", false, true, None),
        ("a&amp;b&#38;c&lt;&#x26;", false, true, None),
        ("&undeclared;x", false, true, None),
        ("&#101;", false, true, None),
        // Kept references; a literal `&` from any reference is `&#38;`.
        ("&e;", false, true, Some("&e;")),
        (
            "1&e;2&amp;3&#38;4&#x26;5",
            false,
            true,
            Some("1&e;2&#38;3&#38;4&#38;5"),
        ),
        ("&lt;&gt;&quot;&apos;&e;", false, true, Some("<>\"'&e;")),
        ("&e;&f;&undeclared;", false, true, Some("&e;&f;")),
        // Whitespace becomes a space; character references stay as written.
        ("a\tb\nc&e;&#9;&#10;", false, true, Some("a b c&e;\t\n")),
        ("a\r\nb&e;\r", false, true, Some("a b&e; ")),
        ("a\r\nb&e;", false, false, Some("a  b&e;")),
        // Tokenized values collapse around kept references, not inside them.
        ("  a  &e;  b  ", true, true, Some("a &e; b")),
        (" &e; ", true, true, Some("&e;")),
        ("&#32;a&#32;&#32;&e;&#32;", true, true, Some("a &e;")),
        ("x&undeclared; &e;", true, true, Some("x &e;")),
        ("&e;&#9;", true, true, Some("&e;\t")),
        ("é&e;\u{10000}", false, true, Some("é&e;\u{10000}")),
    ];
    for &(literal, tokenized, physical, expected) in table {
        assert_eq!(
            form(literal, tokenized, physical).as_deref(),
            expected,
            "{literal:?} tokenized={tokenized} physical={physical}"
        );
    }
}

#[test]
fn kept_form_charges_the_literal_before_reserving() {
    let mut charged = None;
    let kept = kept_form("ab&e;", false, true, declared, |bytes| {
        charged = Some(bytes);
        Ok(())
    });
    assert_eq!(kept.unwrap().as_deref(), Some("ab&e;"));
    assert_eq!(charged, Some(5));
    // Nothing is charged when nothing is kept.
    let refused = |_| {
        Err(DtdError::new(
            ErrorKind::LimitExceeded(crate::Limit::OwnedBytes),
            None,
        ))
    };
    assert_eq!(
        kept_form("a&amp;", false, true, declared, refused).unwrap(),
        None
    );
    // A refused charge fails before allocating.
    let error = kept_form("&e;", false, true, declared, refused).unwrap_err();
    assert_eq!(
        error.kind,
        ErrorKind::LimitExceeded(crate::Limit::OwnedBytes)
    );
}
