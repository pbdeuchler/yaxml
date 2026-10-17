use super::*;
use crate::dtd::DtdLimits;

fn context() -> DeclarationContext {
    DeclarationContext {
        internal_subset: true,
        literals: LiteralSource::Physical,
    }
}

fn declare(catalog: &mut DtdCatalog, raw: &str) -> Result<DeclarationEffect, CatalogError> {
    catalog.apply(
        &DtdUnit::parse(raw, DtdLimits::default()).unwrap(),
        context(),
    )
}

#[test]
fn initial_replacement_and_default_phases_are_distinct() {
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    declare(&mut catalog, "<!ENTITY e 'AT&amp;T; &#60;a/> &#13;'>").unwrap();
    let EntityLookup::Internal { id, replacement } = catalog.lookup_general("e") else {
        panic!("internal binding")
    };
    assert_eq!(replacement, "AT&amp;T; <a/> \r");
    assert_eq!(catalog.replacement(id), Some(replacement));
    assert_eq!(id.index(), 0);
    assert_eq!(catalog.declaration(id), Some(0));
    declare(&mut catalog, "<!ENTITY e 'ignored'>").unwrap();
    assert!(matches!(
        catalog.lookup_parameter("e"),
        EntityLookup::Undeclared
    ));
    assert_eq!(
        catalog
            .normalize_attribute_value("r", "a", "AT&amp;T;", LiteralSource::Physical)
            .unwrap(),
        NormalizedValue::Value("AT&T;".into())
    );
    declare(&mut catalog, "<!ATTLIST r a NMTOKENS '  a  &#9; b  '>").unwrap();
    let attribute = catalog.attributes("r").next().unwrap();
    assert_eq!(attribute.name(), "a");
    assert_eq!(attribute.kind(), AttributeKind::NmTokens);
    assert_eq!(
        attribute.default(),
        DefaultValue::Value {
            fixed: false,
            value: "a \t b"
        }
    );
    assert_eq!(attribute.declaration(), 2);
    assert_eq!(catalog.usage().bindings, 2);
    assert_eq!(catalog.attributes("missing").count(), 0);
}

#[test]
fn internal_pe_restriction_and_unread_barrier_are_explicit() {
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    catalog.note_unread_parameter();
    assert!(
        declare(&mut catalog, "<!ENTITY e 'ignored'>")
            .unwrap()
            .ignored_after_unread_parameter
    );
    assert!(matches!(
        catalog.lookup_general("e"),
        EntityLookup::UnreadEntity
    ));
    let error = declare(&mut catalog, "<!ENTITY e '%p;'>").unwrap_err();
    assert_eq!(error.kind, CatalogErrorKind::ParameterInInternalDeclaration);
    assert_eq!(
        error.location,
        ValueLocation::Declaration {
            declaration: 1,
            offset: 12
        }
    );
    assert_eq!(declare(&mut catalog, "<!ENTITY e 'x'>"), Err(error));
}

#[test]
fn replacement_literals_preserve_numeric_cr_and_predefined_errors_cannot_panic() {
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    let unit = DtdUnit::parse("<!ENTITY e 'a\r\nb&#13;'>", DtdLimits::default()).unwrap();
    catalog
        .apply(
            &unit,
            DeclarationContext {
                internal_subset: true,
                literals: LiteralSource::Replacement,
            },
        )
        .unwrap();
    assert!(matches!(
        catalog.lookup_general("e"),
        EntityLookup::Internal {
            replacement: "a\r\nb\r",
            ..
        }
    ));
    assert_eq!(catalog.lookup_general("amp"), EntityLookup::Predefined('&'));
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    assert_eq!(
        declare(&mut catalog, "<!ENTITY amp ';'>").unwrap_err().kind,
        CatalogErrorKind::InvalidPredefinedDeclaration
    );
}
