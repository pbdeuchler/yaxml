use super::*;

#[test]
fn pi_catalog_reads_the_catalog_pseudo_attribute_like_xml_parse_catalog_pi() {
    // (processing instruction data, catalog)
    let cases: &[(&str, Option<&str>)] = &[
        ("catalog=\"cat.xml\"", Some("cat.xml")),
        ("catalog='cat.xml'", Some("cat.xml")),
        ("  catalog  =  \"cat.xml\"  \t\n", Some("cat.xml")),
        ("catalog=\"\"", Some("")),
        ("catalog=\"a'b\"", Some("a'b")),
        ("catalog=\"a b\"", Some("a b")),
        // Syntax errors: no catalog is added.
        ("catalog=cat.xml", None),
        ("catalog=\"cat.xml", None),
        ("catalog=\"cat.xml\" x", None),
        ("catalog=\"a\" catalog=\"b\"", None),
        ("catalog", None),
        ("catalog=", None),
        ("catalog='a\"", None),
        ("catalogue=\"cat.xml\"", None),
        ("Catalog=\"cat.xml\"", None),
        ("cat=\"cat.xml\"", None),
        ("", None),
    ];
    for &(data, expected) in cases {
        assert_eq!(pi_catalog(data), expected, "{data:?}");
    }
}
