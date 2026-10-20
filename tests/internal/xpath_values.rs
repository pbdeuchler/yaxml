use super::*;

#[test]
fn numbers_format_like_libxml2() {
    let cases: &[(f64, &str)] = &[
        (0.0, "0"),
        (-0.0, "0"),
        (f64::NAN, "NaN"),
        (f64::INFINITY, "Infinity"),
        (f64::NEG_INFINITY, "-Infinity"),
        (1.0, "1"),
        (-42.0, "-42"),
        (2147483646.0, "2147483646"),
        (2147483647.0, "2.147483647e+09"),
        (-2147483647.0, "-2147483647"),
        (-2147483648.0, "-2.147483648e+09"),
        (0.5, "0.5"),
        (-0.25, "-0.25"),
        (1.0 / 3.0, "0.333333333333333"),
        (2.0 / 3.0, "0.666666666666667"),
        (123.456, "123.456"),
        (999999999.5, "999999999.5"),
        (1e9 + 0.5, "1.0000000005e+09"),
        (1e10, "1e+10"),
        (1.5e300, "1.5e+300"),
        (0.00001, "0.00001"),
        (0.000001, "1e-06"),
        (-1.5e-7, "-1.5e-07"),
        (5e-324, "4.94065645841247e-324"),
        (f64::MAX, "1.79769313486232e+308"),
    ];
    for &(value, expected) in cases {
        assert_eq!(number_to_string(value), expected, "{value:e}");
    }
}

#[test]
fn strings_convert_to_numbers_like_libxml2() {
    let cases: &[(&str, f64)] = &[
        ("0", 0.0),
        ("12", 12.0),
        ("-12", -12.0),
        (" \t\r\n7 \n", 7.0),
        ("1.", 1.0),
        (".5", 0.5),
        ("-.5", -0.5),
        ("0.1", 0.1),
        ("1e3", 1000.0),
        ("1E+3", 1000.0),
        ("25e-1", 2.5),
        ("1e", 1.0),
        ("0.000000000000000000000000123", 1.23e-25),
        ("1e99999999", f64::INFINITY),
    ];
    for &(text, expected) in cases {
        assert_eq!(string_to_number(text), expected, "{text:?}");
    }
    for text in [
        "", " ", ".", "-", "+1", "--1", "- 1", "1 2", "1x", "x", "0x1", "\u{a0}1", "\u{ff11}",
    ] {
        assert!(string_to_number(text).is_nan(), "{text:?}");
    }
}

#[test]
fn scan_number_reports_consumed_bytes() {
    assert_eq!(scan_number(b"12.5]"), Some((12.5, 4)));
    assert_eq!(scan_number(b".5e1+"), Some((5.0, 4)));
    assert_eq!(scan_number(b"7e-"), Some((7.0, 3)));
    assert_eq!(scan_number(b"."), None);
    assert_eq!(scan_number(b"e1"), None);
    assert_eq!(scan_number(b""), None);
    // Fraction digits beyond 20 significant digits are consumed and ignored;
    // libxml2 divides, so the result is not always correctly rounded.
    assert_eq!(
        scan_number(b"0.123456789012345678901234567"),
        Some((0.12345678901234569, 29))
    );
}

#[test]
fn rounding_substring_and_translate_follow_libxml2() {
    let rounds: &[(f64, f64)] = &[
        (2.5, 3.0),
        (2.4, 2.0),
        (-2.5, -2.0),
        (-2.6, -3.0),
        (0.4, 0.0),
        (f64::INFINITY, f64::INFINITY),
        (f64::NEG_INFINITY, f64::NEG_INFINITY),
    ];
    for &(value, expected) in rounds {
        assert_eq!(round(value), expected, "{value}");
    }
    assert!(round(-0.4).is_sign_negative() && round(-0.4) == 0.0);
    assert!(round(f64::NAN).is_nan());

    let max = f64::from(i32::MAX);
    let substrings: &[(&str, f64, Option<f64>, &str)] = &[
        ("12345", 2.0, Some(3.0), "234"),
        ("12345", 2.0, None, "2345"),
        ("12345", 1.5, Some(2.6), "234"),
        ("12345", 0.0, Some(3.0), "12"),
        ("12345", -1.0, Some(3.0), "1"),
        ("12345", f64::NAN, Some(3.0), ""),
        ("12345", 1.0, Some(f64::NAN), ""),
        ("12345", -42.0, Some(f64::INFINITY), "12345"),
        ("12345", f64::NEG_INFINITY, Some(f64::INFINITY), ""),
        ("12345", max, None, ""),
        ("12345", 1.0, Some(max), "12345"),
        ("12345", 5.0, Some(1e300), "5"),
        ("12345", 6.0, None, ""),
        ("", 1.0, None, ""),
        ("a\u{e9}\u{1d11e}b", 2.0, Some(2.0), "\u{e9}\u{1d11e}"),
    ];
    for &(text, start, length, expected) in substrings {
        assert_eq!(
            substring(text, start, length),
            expected,
            "{text} {start} {length:?}"
        );
    }

    for (text, from, to, expected) in [
        ("bar", "abc", "ABC", "BAr"),
        ("aaa", "aa", "xy", "xxx"),
        ("abc", "", "xyz", "abc"),
        ("abc", "abc", "", ""),
        ("", "a", "b", ""),
        // Replacements wider than what they replace.
        ("abc", "b", "\u{e9}\u{1d11e}", "a\u{e9}c"),
    ] {
        assert_eq!(translate(text, from, to), Ok(expected.to_owned()), "{text}");
    }
}
