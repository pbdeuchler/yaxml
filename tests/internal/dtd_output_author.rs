use super::*;

fn encode(raw: &str, source: LiteralSource) -> Result<String, UnrepresentableIdentifier> {
    DtdUnit::parse(raw, DtdLimits::default())
        .expect(raw)
        .physical_fragments(source)
        .collect()
}

#[test]
fn constructed_entity_and_default_cr_have_distinct_physical_encodings() {
    assert_eq!(
        encode("<!ENTITY e 'a\r\nb&#13;'>", LiteralSource::Replacement),
        Ok("<!ENTITY e 'a&#13;\nb&#13;'>".into())
    );
    assert_eq!(
        encode(
            "<!ATTLIST r a CDATA 'a\r\nb&#13;'>",
            LiteralSource::Replacement
        ),
        Ok("<!ATTLIST r a CDATA 'a \nb&#13;'>".into())
    );
}

#[test]
fn unrepresentable_identifier_is_typed_and_iterator_is_fused() {
    let raw = "<!ENTITY e SYSTEM 'a\rb'>";
    let unit = DtdUnit::parse(raw, DtdLimits::default()).unwrap();
    let mut fragments = unit.physical_fragments(LiteralSource::Replacement);
    assert_eq!(fragments.next(), Some(Ok("<!ENTITY e SYSTEM 'a")));
    assert_eq!(
        fragments.next(),
        Some(Err(UnrepresentableIdentifier { offset: 20 }))
    );
    assert_eq!(fragments.next(), None);
    assert_eq!(fragments.next(), None);
}
