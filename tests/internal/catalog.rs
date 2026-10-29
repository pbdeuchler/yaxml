use super::*;

#[test]
fn pi_catalog_reads_the_catalog_pseudo_attribute_like_xml_parse_catalog_pi() {
    // (processing instruction data, catalog; Err = libxml2's syntax warning)
    let cases: &[(&str, Result<Option<&str>, ()>)] = &[
        ("catalog=\"cat.xml\"", Ok(Some("cat.xml"))),
        ("catalog='cat.xml'", Ok(Some("cat.xml"))),
        ("  catalog  =  \"cat.xml\"  \t\n", Ok(Some("cat.xml"))),
        ("catalog=\"\"", Ok(Some(""))),
        ("catalog=\"a'b\"", Ok(Some("a'b"))),
        ("catalog=\"a b\"", Ok(Some("a b"))),
        // No `=` after `catalog`: ignored silently.
        ("catalog", Ok(None)),
        ("catalogue=\"cat.xml\"", Ok(None)),
        // Syntax errors: no catalog is added.
        ("catalog=cat.xml", Err(())),
        ("catalog=\"cat.xml", Err(())),
        ("catalog=\"cat.xml\" x", Err(())),
        ("catalog=\"a\" catalog=\"b\"", Err(())),
        ("catalog=", Err(())),
        ("catalog='a\"", Err(())),
        ("Catalog=\"cat.xml\"", Err(())),
        ("cat=\"cat.xml\"", Err(())),
        ("", Err(())),
    ];
    for &(data, expected) in cases {
        assert_eq!(pi_catalog(data), expected, "{data:?}");
    }
}
