//! Table-driven XML Schema datatype tests. Expected results match libxml2
//! 2.15.4 (`harness/tooling/libxml2_schema_types.c`); `schema_types_oracle.rs`
//! checks many generated cases.
#![forbid(unsafe_code)]

use std::borrow::Cow;

use yaxml::schema_types::{
    Comparison, Context, Facet, FacetError, InvalidValue, Type, WhiteSpace, normalize,
};

#[test]
fn type_metadata() {
    assert_eq!(Type::all().count(), 45);
    for ty in Type::all() {
        assert_eq!(Type::from_name(ty.name()), Some(ty));
    }
    let cases = [
        (
            Type::AnySimpleType,
            None,
            Type::AnySimpleType,
            WhiteSpace::Preserve,
        ),
        (
            Type::String,
            Some(Type::AnySimpleType),
            Type::String,
            WhiteSpace::Preserve,
        ),
        (
            Type::NormalizedString,
            Some(Type::String),
            Type::String,
            WhiteSpace::Replace,
        ),
        (
            Type::Id,
            Some(Type::NCName),
            Type::String,
            WhiteSpace::Collapse,
        ),
        (
            Type::UnsignedByte,
            Some(Type::UnsignedShort),
            Type::Decimal,
            WhiteSpace::Collapse,
        ),
        (
            Type::NegativeInteger,
            Some(Type::NonPositiveInteger),
            Type::Decimal,
            WhiteSpace::Collapse,
        ),
        (
            Type::NmTokens,
            Some(Type::AnySimpleType),
            Type::AnySimpleType,
            WhiteSpace::Collapse,
        ),
        (
            Type::GMonthDay,
            Some(Type::AnySimpleType),
            Type::GMonthDay,
            WhiteSpace::Collapse,
        ),
    ];
    for (ty, base, primitive, whitespace) in cases {
        assert_eq!(
            (ty.base(), ty.primitive(), ty.whitespace()),
            (base, primitive, whitespace),
            "{ty:?}"
        );
    }
    assert_eq!(
        Type::from_name("ENTITIES").and_then(Type::item),
        Some(Type::Entity)
    );
    assert_eq!(Type::IdRefs.item(), Some(Type::IdRef));
    assert_eq!(Type::Token.item(), None);
    assert_eq!(Type::from_name("nmtoken"), None);
    assert_eq!(Type::from_name("anyType"), None);
}

#[test]
fn whitespace_normalization() {
    let cases = [
        ("a b", WhiteSpace::Preserve, "a b", true),
        (" a\tb\n", WhiteSpace::Preserve, " a\tb\n", true),
        ("a b", WhiteSpace::Replace, "a b", true),
        (" a\tb\r\n", WhiteSpace::Replace, " a b  ", false),
        ("a b", WhiteSpace::Collapse, "a b", true),
        ("  a \t b\n", WhiteSpace::Collapse, "a b", false),
        ("a  b", WhiteSpace::Collapse, "a b", false),
        (" ", WhiteSpace::Collapse, "", false),
        ("", WhiteSpace::Collapse, "", true),
        ("a\u{a0}\u{c}b", WhiteSpace::Collapse, "a\u{a0}\u{c}b", true),
    ];
    for (input, whitespace, expected, borrowed) in cases {
        let output = normalize(input, whitespace);
        assert_eq!(output, expected, "{input:?} {whitespace:?}");
        assert_eq!(
            matches!(output, Cow::Borrowed(_)),
            borrowed,
            "{input:?} {whitespace:?}"
        );
    }
}

#[test]
fn lexical_spaces() {
    use Type::*;
    let cases: &[(Type, &[&str], &[&str])] = &[
        (AnySimpleType, &["", " any\tthing "], &[]),
        (String, &["", " a\tb ", "<&>"], &[]),
        (NormalizedString, &["", " a\tb "], &[]),
        (Token, &["", "  a   b  ", "a\tb"], &[]),
        (
            Language,
            &["en", "en-US", "x-1abc", " de-CH-1996 ", "abcdefgh"],
            &[
                "",
                "1en",
                "en-",
                "-en",
                "abcdefghi",
                "en-abcdefghi",
                "en_US",
                "en US",
            ],
        ),
        (
            NmToken,
            &["a", "-1.x", ":", " a:b ", "·"],
            &["", "a b", "a,b", "<"],
        ),
        (NmTokens, &["a", " a  b\tc "], &["", " ", "a,b"]),
        (
            Name,
            &["a", "_x", ":a", "a:b:c", " é "],
            &["", "1a", "-a", "a b", ".a"],
        ),
        (NCName, &["a", "_x.y-z", "é"], &["", "a:b", ":a", "1a"]),
        (Id, &["id1", " x "], &["1", "a:b", ""]),
        (IdRef, &["ref"], &["a b", "1"]),
        // libxml2 accepts an empty IDREFS.
        (IdRefs, &["", "a b", " a "], &["a 1", "a:b"]),
        (
            QName,
            &["a", "p:a", " p:a ", "xml:lang", "unbound:a"],
            &["", "p:", ":a", "a:b:c", "1:a", "p:1", "a b"],
        ),
        (
            AnyUri,
            &["", "http://a/b?c#d", "a b", "é", "../x", "#f", "<x>"],
            &["http://[::1", "http://a:b:c/", ":"],
        ),
        (
            Boolean,
            &["true", "false", "1", "0", " true "],
            &["", "TRUE", "yes", "2", "t", "true false"],
        ),
        (
            Decimal,
            &[
                "0",
                "-0",
                "+1.5",
                ".5",
                "5.",
                "0.",
                "-.5",
                "00012.3400",
                " 7 ",
                "123456789012345678901234567890.123456789",
            ],
            &[
                "", ".", "-", "+", "+.", "1e3", "1.2.3", "INF", "1 2", "--1", "0x1",
            ],
        ),
        (
            Integer,
            &["0", "-0", "+42", "00", "123456789012345678901234567890"],
            &["", "+", "-", "1.0", "1.", ".1", "1e2", "a"],
        ),
        (
            NonPositiveInteger,
            &["0", "-0", "+0", "-5", "-99999999999999999999"],
            &["1", "+1"],
        ),
        (
            NegativeInteger,
            &["-1", "-99999999999999999999"],
            &["0", "-0", "1"],
        ),
        (
            Long,
            &["9223372036854775807", "-9223372036854775808"],
            &["9223372036854775808", "-9223372036854775809"],
        ),
        (
            Int,
            &["2147483647", "-2147483648"],
            &["2147483648", "-2147483649"],
        ),
        (Short, &["32767", "-32768"], &["32768", "-32769"]),
        (Byte, &["127", "-128", "+0"], &["128", "-129"]),
        (
            NonNegativeInteger,
            &["0", "-0", "99999999999999999999"],
            &["-1"],
        ),
        (
            UnsignedLong,
            &["18446744073709551615", "-0"],
            &["18446744073709551616", "-1"],
        ),
        (UnsignedInt, &["4294967295"], &["4294967296", "-1"]),
        (UnsignedShort, &["65535"], &["65536"]),
        (UnsignedByte, &["255", "000255"], &["256", "-1"]),
        (
            PositiveInteger,
            &["1", "+1", "00099999999999999999999"],
            &["0", "-0", "-1"],
        ),
        (
            Float,
            &[
                "0", "-0", "1.5", ".5", "5.", "-1E4", "1e+3", "1e-3", "1e", "1E+", "INF", "-INF",
                "NaN", "1e999", " 3 ",
            ],
            &[
                "", ".", "e3", "+INF", "inf", "-NaN", "nan", "1e3.5", "1.2.3", "0x1p3", "--1",
                "+-1", "1 2",
            ],
        ),
        (
            Double,
            &["123.456e-78", "-INF", "1.7976931348623157e309", "INF "],
            &["IN F", "1,5"],
        ),
        (
            Duration,
            &[
                "P1Y",
                "-P1Y2M3DT4H5M6.7S",
                "PT0S",
                "P0D",
                "PT1.5S",
                "PT36H",
                "P99999999999D",
            ],
            &[
                "",
                "P",
                "-P",
                "PT",
                "P1H",
                "PT1D",
                "P1.5Y",
                "P1S",
                "P1DT",
                "1Y",
                "P-1D",
                "P1Y1Y",
                "PT1M1H",
                "P99999999999999999999D",
                "P 1D",
            ],
        ),
        (
            DateTime,
            &[
                "2001-10-26T21:32:52",
                "2001-10-26T21:32:52.12679",
                "2001-10-26T21:32:52+02:00",
                "2001-10-26T19:32:52Z",
                "-0001-01-01T00:00:00",
                "2000-02-29T24:00:00",
                "12345-01-01T00:00:00-14:00",
            ],
            &[
                "2001-10-26",
                "2001-10-26T21:32",
                "2001-10-26T25:00:00",
                "2001-10-26T24:00:01",
                "2001-02-29T00:00:00",
                "0000-01-01T00:00:00",
                "01-10-26T21:32:52",
                "2001-10-26T21:32:52+14:01",
                "2001-10-26T21:32:60",
                "02001-01-01T00:00:00",
                "2001-10-26T21:32:52 Z",
            ],
        ),
        (
            Time,
            &["21:32:52", "00:00:00.5Z", "24:00:00", "23:59:59.999-05:00"],
            &[
                "24:00:01",
                "21:32",
                "21:60:00",
                "1:00:00",
                "21:32:52+25:00",
                "21:32:52.",
                "",
            ],
        ),
        (
            Date,
            &[
                "2001-10-26",
                "2001-10-26Z",
                "2001-10-26-14:00",
                "2000-02-29",
                "-2001-10-26",
                "1600-02-29",
            ],
            &[
                "2001-02-29",
                "1900-02-29",
                "2001-13-01",
                "2001-10-32",
                "2001-10",
                "2001-10-26T00:00:00",
                "2001-1-26",
            ],
        ),
        (
            GYearMonth,
            &["2001-10", "2001-10+05:00", "-0001-12"],
            &["2001", "2001-13", "2001-1", "01-10"],
        ),
        (
            GYear,
            &["2001", "-2001", "20010", "2001Z", "2001-14:00"],
            &["01", "0000", "02001", "+2001", "2001-15:00", ""],
        ),
        (
            GMonthDay,
            &["--10-26", "--02-29", "--10-26Z", "--10-26-05:00"],
            &["--02-30", "--13-01", "10-26", "--10-26:00", "---26"],
        ),
        (
            GDay,
            &["---26", "---01Z", "---31+14:00"],
            &["---32", "---00", "--26", "---1"],
        ),
        (
            GMonth,
            &["--10", "--12Z", "--10-05:00"],
            &["--13", "--00", "--1", "--10--"],
        ),
        (
            HexBinary,
            &["", "0fB7", " 00 ", "ABCDEF"],
            &["0", "0g", "0 f", "0x00"],
        ),
        (
            Base64Binary,
            &["", "Zm9v", "Zm9vYg==", "Zm9vYmE=", "Zm 9v", "Zm!9v", "YQ=="],
            &[
                "Zm9", "Zm9vY", "Zm9vYh==", "Zm9vYmF=", "Zm9v=", "Y===", "Zg=a", "Z===",
            ],
        ),
        (Entity, &[], &["pic", "1"]),
        (Entities, &[], &["pic"]),
        (Notation, &[], &["gif"]),
    ];
    let covered: std::collections::HashSet<Type> = cases.iter().map(|case| case.0).collect();
    assert_eq!(covered.len(), Type::all().count(), "every type has a row");
    for (ty, valid, invalid) in cases {
        for value in *valid {
            assert!(
                ty.parse(value, None).is_ok(),
                "{ty:?} should accept {value:?}"
            );
        }
        for value in *invalid {
            assert_eq!(
                ty.parse(value, None).err(),
                Some(InvalidValue),
                "{ty:?} should reject {value:?}"
            );
        }
    }
}

struct Document;

impl Context for Document {
    fn namespace(&self, prefix: &str) -> Option<&str> {
        match prefix {
            "" => Some("urn:default"),
            "p" => Some("urn:p"),
            _ => None,
        }
    }

    fn has_notation(&self, name: &str) -> bool {
        matches!(name, "gif" | "p:n")
    }

    fn has_unparsed_entity(&self, name: &str) -> bool {
        name == "pic"
    }
}

/// Only namespaces: no notations or unparsed entities.
struct Namespaces;

impl Context for Namespaces {
    fn namespace(&self, _: &str) -> Option<&str> {
        None
    }
}

#[test]
fn context_dependent_types() {
    use Type::*;
    let document: Option<&dyn Context> = Some(&Document);
    let cases: &[(Type, &[&str], &[&str])] = &[
        (QName, &["a", "p:a", "xml:a"], &["q:a", "p:"]),
        (
            Notation,
            &["gif", "p:n", " gif "],
            &["png", "q:n", "p:gif", ""],
        ),
        (Entity, &["pic"], &["txt", "1", ""]),
        (Entities, &["pic", "pic  pic"], &["", "pic txt"]),
    ];
    for (ty, valid, invalid) in cases {
        for value in *valid {
            assert!(
                ty.parse(value, document).is_ok(),
                "{ty:?} should accept {value:?}"
            );
        }
        for value in *invalid {
            assert!(
                ty.parse(value, document).is_err(),
                "{ty:?} should reject {value:?}"
            );
        }
    }
    let namespaces: Option<&dyn Context> = Some(&Namespaces);
    assert!(Notation.parse("gif", namespaces).is_err());
    assert!(Entity.parse("pic", namespaces).is_err());
    assert!(QName.parse("p:a", namespaces).is_err());
    assert!(QName.parse("xml:a", namespaces).is_ok());
    // Resolved QNames compare by namespace and local name.
    let qname = |text, context| QName.parse(text, context).unwrap();
    let cases = [
        (
            qname("p:a", document),
            qname("p:a", document),
            Comparison::Equal,
        ),
        (
            qname("a", document),
            qname("a", None),
            Comparison::Indeterminate,
        ),
        (qname("p:a", None), qname("p:a", None), Comparison::Equal),
        (
            qname("p:a", document),
            qname("a", document),
            Comparison::Indeterminate,
        ),
        (
            Notation.parse("gif", document).unwrap(),
            qname("gif", document),
            Comparison::Indeterminate,
        ),
        (
            Notation.parse("gif", document).unwrap(),
            qname("gif", None),
            Comparison::Equal,
        ),
    ];
    for (x, y, expected) in cases {
        assert_eq!(x.compare(&y), expected, "{x:?} {y:?}");
    }
}

#[test]
fn comparisons() {
    use Comparison::*;
    use Type::*;
    let cases = [
        (Decimal, "1.5", Decimal, "1.50", Equal),
        (Decimal, "-0", Integer, "0", Equal),
        (Decimal, "1.25", Decimal, "1.3", Less),
        (Decimal, "-1.25", Decimal, "-1.3", Greater),
        (Decimal, "100", Decimal, "99.999", Greater),
        (
            Integer,
            "123456789012345678901234567891",
            Long,
            "1",
            Greater,
        ),
        (UnsignedByte, "7", Byte, "-7", Greater),
        (Decimal, "0.001", Decimal, "0", Greater),
        (Decimal, "1", Float, "1", Incomparable),
        (Float, "1.5", Double, "1.5", Equal),
        (Float, "0.1", Double, "0.1", Greater),
        (Double, "NaN", Double, "NaN", Equal),
        (Double, "NaN", Float, "INF", Greater),
        (Float, "-INF", Double, "NaN", Less),
        (Double, "-0", Double, "0", Equal),
        (Boolean, "false", Boolean, "true", Less),
        (Boolean, "1", Boolean, "true", Equal),
        (Boolean, "1", Integer, "1", Incomparable),
        (Duration, "P1Y", Duration, "P12M", Equal),
        (Duration, "P1D", Duration, "PT24H", Equal),
        (Duration, "P1M", Duration, "P30D", Indeterminate),
        (Duration, "P1M", Duration, "P27D", Greater),
        (Duration, "P1M", Duration, "P32D", Less),
        (Duration, "-P1D", Duration, "PT1S", Less),
        (Duration, "P1Y", Duration, "P365D", Indeterminate),
        (Duration, "P1Y", Duration, "P367D", Less),
        (
            DateTime,
            "2001-10-26T21:32:52Z",
            DateTime,
            "2001-10-26T23:32:52+02:00",
            Equal,
        ),
        (
            DateTime,
            "2001-10-26T21:32:52",
            DateTime,
            "2001-10-26T21:32:53",
            Less,
        ),
        (
            DateTime,
            "2000-01-01T12:00:00",
            DateTime,
            "1999-12-31T23:00:00Z",
            Greater,
        ),
        (
            DateTime,
            "2000-01-01T12:00:00",
            DateTime,
            "2000-01-01T12:00:00Z",
            Indeterminate,
        ),
        (
            DateTime,
            "2000-01-15T12:00:00",
            DateTime,
            "2000-01-16T12:00:00Z",
            Less,
        ),
        // libxml2 orders by day before time, so 24:00:00 ends its own day.
        (
            DateTime,
            "2000-01-01T24:00:00",
            DateTime,
            "2000-01-02T00:00:00",
            Less,
        ),
        // Shifts to UTC cross days, months, leap days and the missing year 0.
        (
            DateTime,
            "2000-01-01T00:30:00+01:00",
            DateTime,
            "1999-12-31T23:30:00Z",
            Equal,
        ),
        (
            DateTime,
            "1999-12-31T23:30:00-01:00",
            DateTime,
            "2000-01-01T00:30:00Z",
            Equal,
        ),
        (
            DateTime,
            "2000-02-29T23:00:00-02:00",
            DateTime,
            "2000-03-01T01:00:00Z",
            Equal,
        ),
        (
            DateTime,
            "0001-01-01T00:30:00+01:00",
            DateTime,
            "-0001-12-31T23:30:00Z",
            Equal,
        ),
        (Date, "2000-03-01+14:00", Date, "2000-02-29Z", Greater),
        (Date, "2001-10-26", Date, "2001-10-27", Less),
        (Date, "2001-10-26+02:00", Date, "2001-10-26Z", Less),
        (Date, "-0001-01-01", Date, "0001-01-01", Less),
        // libxml2 moves a time shifted to UTC into year 1, after unshifted times.
        (Time, "13:20:00-05:00", Time, "18:20:00Z", Greater),
        (Time, "00:30:00+01:00", Time, "23:30:00Z", Equal),
        (GYear, "2001", GYear, "2002", Less),
        (GYear, "2001", GYearMonth, "2001-01", Indeterminate),
        (
            Date,
            "2001-10-26",
            DateTime,
            "2001-10-26T00:00:00",
            Indeterminate,
        ),
        (GMonth, "--10", GMonth, "--11", Less),
        (GDay, "---05", GDay, "---04", Greater),
        (GMonthDay, "--02-29", GMonthDay, "--03-01", Less),
        (
            DateTime,
            "2001-10-26T21:32:52",
            Duration,
            "P1D",
            Incomparable,
        ),
        (String, "a", String, "a", Equal),
        (String, "a", String, "b", Indeterminate),
        (Token, "a", Token, "b", Less),
        (String, "a b", Token, " a  b ", Equal),
        (String, "a  b", Token, "a b", Less),
        (NormalizedString, "a\tb", String, "a b", Equal),
        (Token, "b", NormalizedString, "a", Less),
        (Token, "a b", Token, "a\"", Incomparable),
        (AnySimpleType, " a  b ", Token, "a b", Equal),
        (AnyUri, "http://a", String, "http://a", Equal),
        (NmTokens, "a b", NmTokens, "a b", Incomparable),
        (String, "1", Integer, "1", Incomparable),
        (HexBinary, "0F", HexBinary, "0f", Equal),
        (HexBinary, "0F", HexBinary, "0F00", Less),
        (HexBinary, "FF", HexBinary, "0F", Greater),
        (Base64Binary, "Zm9v", Base64Binary, "Zm 9v", Equal),
        (Base64Binary, "YQ==", Base64Binary, "Zm9v", Less),
        (HexBinary, "00", Base64Binary, "AA==", Incomparable),
    ];
    for (xt, x, yt, y, expected) in cases {
        let (xv, yv) = (xt.parse(x, None).unwrap(), yt.parse(y, None).unwrap());
        assert_eq!(xv.compare(&yv), expected, "{xt:?} {x:?} vs {yt:?} {y:?}");
    }
}

#[test]
fn value_equality() {
    use Type::*;
    let cases = [
        (Decimal, "1.0", Integer, "1", true),
        (Float, "1", Double, "1", false),
        (String, "a b", Token, "a b", true),
        (String, "a  b", Token, "a  b", false),
        (Token, "a", AnyUri, "a", false),
        (Date, "2001-01-01", DateTime, "2001-01-01T00:00:00", false),
        (Time, "12:00:00Z", Time, "13:00:00+01:00", false),
        (Time, "12:00:00+01:00", Time, "13:00:00+02:00", true),
        (NmTokens, "a b", NmTokens, " a b ", true),
        (Double, "NaN", Double, "NaN", true),
        (Boolean, "0", Boolean, "false", true),
    ];
    for (xt, x, yt, y, expected) in cases {
        let (xv, yv) = (xt.parse(x, None).unwrap(), yt.parse(y, None).unwrap());
        assert_eq!(xv.equals(&yv), expected, "{xt:?} {x:?} vs {yt:?} {y:?}");
        assert_eq!(xv.ty(), xt);
    }
}

#[test]
fn facets() {
    use Type::*;
    let cases = [
        ("length", "3", String, "abc", true),
        ("length", "3", String, "ab", false),
        ("length", "2", String, "é漢", true),
        ("minLength", "2", NormalizedString, " a ", true),
        ("maxLength", "2", Token, " a b ", false),
        ("length", "2", HexBinary, "0fB7", true),
        ("length", "3", Base64Binary, "Zm9v", true),
        ("maxLength", "1", Base64Binary, "Zm9vYg==", false),
        ("length", "2", NmTokens, "a  b", true),
        ("minLength", "1", IdRefs, "", false),
        ("length", "7", QName, "p:a", true),
        ("length", "1", Decimal, "1", false),
        ("length", "18446744073709551616", String, "a", false),
        ("pattern", "\\d{3}", Decimal, "007", true),
        ("pattern", "\\d{3}", Decimal, "7", false),
        ("pattern", "a b", Token, "a b", true),
        ("enumeration", "1.0", Decimal, "1", true),
        ("enumeration", "1.0", Decimal, "1.01", false),
        ("enumeration", "a b", Token, "a   b", true),
        ("enumeration", "a b", String, "a  b", false),
        ("whiteSpace", "collapse", String, "  ", true),
        ("minInclusive", "10", Integer, "10", true),
        ("minInclusive", "10", Integer, "9", false),
        ("minExclusive", "10", Integer, "10", false),
        ("minExclusive", "10", Integer, "11", true),
        ("maxInclusive", "1.5", Decimal, "1.50", true),
        ("maxInclusive", "1.5", Decimal, "1.51", false),
        ("maxExclusive", "1.5", Decimal, "1.49", true),
        ("maxExclusive", "1.5", Decimal, "1.5", false),
        ("maxInclusive", "INF", Double, "1e308", true),
        ("minInclusive", "0", Double, "NaN", true),
        ("maxInclusive", "0", Double, "NaN", false),
        ("maxExclusive", "P1M", Duration, "P30D", false),
        ("maxExclusive", "P1M", Duration, "P27D", true),
        ("minInclusive", "2001-01-01", Date, "2001-01-01Z", false),
        (
            "maxInclusive",
            "2001-01-01T00:00:00Z",
            DateTime,
            "2000-12-31T23:59:59Z",
            true,
        ),
        ("totalDigits", "3", Decimal, "12.3", true),
        ("totalDigits", "3", Decimal, "1.234", false),
        ("totalDigits", "1", Decimal, "0.0", true),
        ("totalDigits", "2", Decimal, "0.05", true),
        ("totalDigits", "3", Integer, "-00100", true),
        ("totalDigits", "3", Float, "1", false),
        ("fractionDigits", "0", Decimal, "1.000", true),
        ("fractionDigits", "1", Decimal, "1.05", false),
        ("fractionDigits", "2", Decimal, "0.05", true),
    ];
    for (name, facet_value, base, value, expected) in cases {
        let facet = Facet::new(name, facet_value, base, None).unwrap();
        let parsed = base.parse(value, None).unwrap();
        let normalized = normalize(value, base.whitespace());
        assert_eq!(
            facet.check(&parsed, &normalized),
            expected,
            "{name}={facet_value:?} {base:?} {value:?}"
        );
    }
}

#[test]
fn facet_construction_errors() {
    use Type::*;
    let cases = [
        ("length", "-1", String, FacetError::Value),
        ("length", "x", String, FacetError::Value),
        ("totalDigits", "0", Decimal, FacetError::Value),
        ("fractionDigits", "1.5", Decimal, FacetError::Value),
        ("minInclusive", "x", Integer, FacetError::Value),
        ("enumeration", "300", UnsignedByte, FacetError::Value),
        ("whiteSpace", "trim", String, FacetError::Value),
        ("whiteSpace", " collapse", String, FacetError::Value),
        ("maxScale", "1", Decimal, FacetError::Unknown),
    ];
    for (name, value, base, expected) in cases {
        assert_eq!(
            Facet::new(name, value, base, None).err(),
            Some(expected),
            "{name}={value:?}"
        );
    }
    let pattern = Facet::new("pattern", "(", String, None).unwrap_err();
    assert!(matches!(pattern, FacetError::Pattern(_)));
    assert!(pattern.to_string().starts_with("invalid pattern facet: "));
    assert_eq!(FacetError::Unknown.to_string(), "unknown facet");
    assert_eq!(FacetError::Value.to_string(), "invalid facet value");
    assert_eq!(
        InvalidValue.to_string(),
        "invalid lexical value for the datatype"
    );
    let notation = Facet::new("enumeration", "gif", Notation, Some(&Document)).unwrap();
    let value = Notation.parse("gif", Some(&Document)).unwrap();
    assert!(notation.check(&value, "gif"));
    assert!(matches!(
        Facet::new("whiteSpace", "replace", Token, None),
        Ok(Facet::WhiteSpace(WhiteSpace::Replace))
    ));
    assert!(matches!(
        Facet::new("whiteSpace", "preserve", Token, None),
        Ok(Facet::WhiteSpace(WhiteSpace::Preserve))
    ));
}
