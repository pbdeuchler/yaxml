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

#[test]
fn physical_units_and_cr_free_replacements_retain_every_grammar_family() {
    for raw in [
        "<!DOCTYPE π>",
        "<!DOCTYPE π [",
        "<!DOCTYPE π SYSTEM 'relative.dtd'>",
        "<!DOCTYPE π PUBLIC 'a b' 'relative.dtd' [",
        "<!ELEMENT π EMPTY>",
        "<!ELEMENT π ANY>",
        "<!ELEMENT π (#PCDATA|a)*>",
        "<!ELEMENT π ((a,b?)|c)+>",
        "<!ATTLIST π a CDATA #REQUIRED b (x|y) #FIXED 'x'>",
        "<!ENTITY e 'a&amp;&#13;&#38;lt;'>",
        "<!ENTITY % p '<!ELEMENT a EMPTY>'>",
        "<!ENTITY e SYSTEM 'relative.ent'>",
        "<!ENTITY % p PUBLIC 'public' 'relative.ent'>",
        "<!ENTITY e PUBLIC 'public' 'image.bin' NDATA n>",
        "<!NOTATION n SYSTEM 'image'>",
        "<!NOTATION n PUBLIC 'public'>",
        "<!NOTATION n PUBLIC 'public' 'image'>",
        "<!--comment-->",
        "<?target?>",
        "<?target data?>",
    ] {
        for source in [LiteralSource::Physical, LiteralSource::Replacement] {
            let unit = DtdUnit::parse(raw, DtdLimits::default()).expect(raw);
            let mut fragments = unit.physical_fragments(source);
            let piece = fragments.next().unwrap().unwrap();
            assert_eq!(piece, raw);
            assert_eq!(piece.as_ptr(), unit.raw().as_ptr());
            assert_eq!(fragments.next(), None);
            assert_eq!(fragments.next(), None);
            assert_eq!(unit.raw(), raw);
        }
    }
}

#[test]
fn literal_ranges_do_not_change_reference_spelling_or_misc_whitespace() {
    for (raw, replacement) in [
        ("<!ENTITY e '\r\r\r'>", "<!ENTITY e '&#13;&#13;&#13;'>"),
        (
            "<!ATTLIST r a CDATA '\r&#13;\r' b CDATA #FIXED \"π\r\n\r\">",
            "<!ATTLIST r a CDATA ' &#13; ' b CDATA #FIXED \"π \n \">",
        ),
        ("<!ELEMENT\rπ\rEMPTY>", "<!ELEMENT\rπ\rEMPTY>"),
        ("<!--a\r\nb-->", "<!--a\r\nb-->"),
        ("<?p a\rb?>", "<?p a\rb?>"),
    ] {
        assert_eq!(encode(raw, LiteralSource::Physical), Ok(raw.into()));
        assert_eq!(
            encode(raw, LiteralSource::Replacement),
            Ok(replacement.into())
        );
    }
}

#[test]
fn every_external_identifier_context_rejects_only_replacement_cr() {
    for marked in [
        "<!DOCTYPE π SYSTEM 'a^\rb'>",
        "<!DOCTYPE π PUBLIC 'a^\rb' 'system' [",
        "<!ENTITY e SYSTEM 'a^\rb'>",
        "<!ENTITY % p PUBLIC 'public' 'a^\rb'>",
        "<!ENTITY e PUBLIC 'a^\rb' 'system' NDATA n>",
        "<!NOTATION n SYSTEM 'a^\rb'>",
        "<!NOTATION n PUBLIC 'a^\rb'>",
        "<!NOTATION n PUBLIC 'public' 'a^\rb'>",
    ] {
        let offset = marked.find('^').unwrap();
        let raw = marked.replacen('^', "", 1);
        assert_eq!(encode(&raw, LiteralSource::Physical), Ok(raw.clone()));
        assert_eq!(
            encode(&raw, LiteralSource::Replacement),
            Err(UnrepresentableIdentifier { offset })
        );
    }
    // Reference-like spelling remains literal identifier data. A numeric
    // spelling additionally contains a forbidden URI fragment delimiter.
    assert_eq!(
        encode("<!ENTITY e SYSTEM 'a&amp;b'>", LiteralSource::Replacement),
        Ok("<!ENTITY e SYSTEM 'a&amp;b'>".into())
    );
    assert_eq!(
        DtdUnit::parse("<!ENTITY e SYSTEM 'a&#13;b'>", DtdLimits::default()).unwrap_err(),
        DtdError::new(DtdErrorKind::SystemIdentifierFragment, 21)
    );
}

#[test]
fn physical_entity_output_preserves_constructed_replacement_and_attribute_use() {
    use crate::dtd_catalog::{
        CatalogLimits, DeclarationContext, DtdCatalog, EntityLookup, NormalizedValue,
    };

    let raw = "<!ENTITY e \"π\r\n&amp;&#13;&#38;lt;\">";
    let physical = encode(raw, LiteralSource::Replacement).unwrap();
    assert_eq!(physical, "<!ENTITY e \"π&#13;\n&amp;&#13;&#38;lt;\">");
    for (declaration, source) in [
        (raw, LiteralSource::Replacement),
        (physical.as_str(), LiteralSource::Physical),
    ] {
        let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
        let unit = DtdUnit::parse(declaration, DtdLimits::default()).unwrap();
        catalog
            .apply(
                &unit,
                DeclarationContext {
                    internal_subset: true,
                    literals: source,
                },
            )
            .unwrap();
        let EntityLookup::Internal { replacement, .. } = catalog.lookup_general("e") else {
            panic!("internal entity")
        };
        assert_eq!(replacement, "π\r\n&amp;\r&lt;");
        assert_eq!(
            catalog
                .normalize_attribute_value("r", "x", "&e;", LiteralSource::Physical)
                .unwrap(),
            NormalizedValue::Value("π  & <".into())
        );
    }
}

#[test]
fn physical_defaults_preserve_numeric_whitespace_and_separate_literal_crlf() {
    use crate::dtd_catalog::{CatalogLimits, DeclarationContext, DefaultValue, DtdCatalog};

    let raw = "<!ATTLIST r x CDATA \"a\r\n&#13;&q;\" y NMTOKENS \" \r\nx&#9; \r y \">";
    let physical = encode(raw, LiteralSource::Replacement).unwrap();
    assert_eq!(
        physical,
        "<!ATTLIST r x CDATA \"a \n&#13;&q;\" y NMTOKENS \"  \nx&#9;   y \">"
    );
    for (declaration, source) in [
        (raw, LiteralSource::Replacement),
        (physical.as_str(), LiteralSource::Physical),
    ] {
        let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
        let entity = DtdUnit::parse("<!ENTITY q '&#13;'>", DtdLimits::default()).unwrap();
        catalog
            .apply(
                &entity,
                DeclarationContext {
                    internal_subset: true,
                    literals: LiteralSource::Physical,
                },
            )
            .unwrap();
        let unit = DtdUnit::parse(declaration, DtdLimits::default()).unwrap();
        catalog
            .apply(
                &unit,
                DeclarationContext {
                    internal_subset: true,
                    literals: source,
                },
            )
            .unwrap();
        let defaults: Vec<_> = catalog
            .attributes("r")
            .map(|binding| (binding.name(), binding.default()))
            .collect();
        assert_eq!(
            defaults,
            [
                (
                    "x",
                    DefaultValue::Value {
                        fixed: false,
                        value: "a  \r "
                    }
                ),
                (
                    "y",
                    DefaultValue::Value {
                        fixed: false,
                        value: "x\t y"
                    }
                ),
            ]
        );
    }
}
