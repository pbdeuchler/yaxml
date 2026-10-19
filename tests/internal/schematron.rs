use super::{printf_g, truncate};

#[test]
fn printf_g_matches_c() {
    let cases = [
        (0.0, "0"),
        (-0.0, "-0"),
        (1.0, "1"),
        (100.0, "100"),
        (0.1, "0.1"),
        (1.0 / 3.0, "0.333333"),
        (123456.0, "123456"),
        (1234567.0, "1.23457e+06"),
        (999999.5, "1e+06"),
        (0.0001, "0.0001"),
        (0.00001234, "1.234e-05"),
        (-2.5e-300, "-2.5e-300"),
        (1e100, "1e+100"),
        (f64::NAN, "nan"),
        (f64::INFINITY, "inf"),
        (f64::NEG_INFINITY, "-inf"),
    ];
    for (value, expected) in cases {
        assert_eq!(printf_g(value), expected, "{value}");
    }
}

#[test]
fn truncate_keeps_998_bytes_on_a_char_boundary() {
    let cases = [
        (String::from("short"), 5),
        ("a".repeat(2000), 998),
        (format!("{}é", "a".repeat(997)), 997),
    ];
    for (text, expected) in cases {
        assert_eq!(truncate(text).len(), expected);
    }
}
