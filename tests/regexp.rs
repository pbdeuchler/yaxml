//! Table-driven XML Schema regular expression tests. Expected results were
//! produced by libxml2 2.15.4 (`tooling/libxml2_schema_types.c`) except
//! where a row says otherwise; `schema_types_oracle.rs` checks many more.
#![forbid(unsafe_code)]

use std::time::{Duration, Instant};

use yaxml::regexp::{DEFAULT_BUDGET, Error, Regex};

fn check(pattern: &str, matching: &[&str], failing: &[&str]) {
    let regex = Regex::new(pattern).unwrap_or_else(|e| panic!("{pattern:?}: {e}"));
    for value in matching {
        assert!(regex.is_match(value), "{pattern:?} should match {value:?}");
    }
    for value in failing {
        assert!(
            !regex.is_match(value),
            "{pattern:?} should not match {value:?}"
        );
    }
}

#[test]
fn branches_pieces_and_anchoring() {
    let cases: &[(&str, &[&str], &[&str])] = &[
        ("", &[""], &["a"]),
        ("abc", &["abc"], &["", "ab", "abcd", "xabc"]),
        ("a|b|", &["a", "b", ""], &["ab"]),
        ("(a|bc)d", &["ad", "bcd"], &["d", "abcd"]),
        ("()", &[""], &["a"]),
        ("(a(b(c)))", &["abc"], &["ab"]),
        ("^a$", &["^a$"], &["a"]),
        ("{}", &["{}"], &[""]),
        ("a}", &["a}"], &["a"]),
        ("é漢😀", &["é漢😀"], &["e漢😀"]),
    ];
    for (pattern, matching, failing) in cases {
        check(pattern, matching, failing);
    }
}

#[test]
fn quantifiers() {
    let cases: &[(&str, &[&str], &[&str])] = &[
        ("a?", &["", "a"], &["aa"]),
        ("a*", &["", "a", "aaaa"], &["b"]),
        ("a+", &["a", "aaa"], &[""]),
        ("a{2}", &["aa"], &["a", "aaa"]),
        ("a{2,}", &["aa", "aaaaa"], &["a"]),
        ("a{2,3}", &["aa", "aaa"], &["a", "aaaa"]),
        ("a{0}", &[""], &["a"]),
        ("a{0,0}b", &["b"], &["ab"]),
        ("a{0,2}", &["", "a", "aa"], &["aaa"]),
        // libxml2: a maximum of 0 means "same as the minimum".
        ("a{2,0}", &["aa"], &["a", "aaa"]),
        // libxml2: a maximum below the minimum matches nothing.
        ("a{3,2}|b", &["b"], &["aa", "aaa"]),
        ("(ab){3,2}", &[], &["ababab", ""]),
        ("a{0,2147483647}", &["", "aaaa"], &["b"]),
        (
            "(ab|c){1,2}d",
            &["abd", "cd", "abcd", "ccd"],
            &["d", "abccd"],
        ),
        (
            "((a|b|\\p{Nd}){1,2}|aaa|bbbb){1,2}",
            &["bab", "aaabbbb", "a0b"],
            &["aaca"],
        ),
        (
            "(\\d{1,3}\\.){3}\\d{1,3}",
            &["1.2.3.4", "255.255.0.12"],
            &["1.2.3", "1234.1.1.1"],
        ),
        ("(a*)*b", &["b", "aaab"], &["aaa"]),
        ("(a|ab)(c|bcd)", &["abcd", "ac", "abc"], &["abd"]),
        ("((a){2}){3}", &["aaaaaa"], &["aaaaa"]),
    ];
    for (pattern, matching, failing) in cases {
        check(pattern, matching, failing);
    }
}

#[test]
fn character_class_expressions() {
    let cases: &[(&str, &[&str], &[&str])] = &[
        ("[abc]", &["a", "c"], &["d", "ab", ""]),
        ("[a-z0-9]", &["q", "5"], &["A", "-"]),
        ("[^a-z]", &["A", "-", "é"], &["q"]),
        ("[a-z-[aeiou]]", &["b", "z"], &["a", "u", "B"]),
        ("[a-z-[b-y-[c]]]", &["a", "z"], &["b", "c", "d"]),
        ("[]", &[], &["", "a", "]"]),
        ("[-a]", &["-", "a"], &["b"]),
        ("[a-]", &["-", "a"], &["b"]),
        ("[^-]", &["a"], &["-"]),
        ("[.?*+(){}|^$]", &[".", "?", "*", "|", "^", "$"], &["a"]),
        ("[\\]\\[\\-\\\\]", &["]", "[", "-", "\\"], &["a"]),
        ("[\\n\\r\\t]", &["\n", "\r", "\t"], &[" "]),
        ("[\\d\\s]", &["7", " ", "\t", "٣"], &["a"]),
        ("[\\p{Lu}\\p{Nd}]", &["A", "9"], &["a"]),
        ("[\\u0041-\\u0043]", &["A", "C"], &["D"]),
        ("[\\uD83D\\uDE00]", &["😀"], &["a"]),
        (
            "[\\i-[:]][\\c-[:]]*",
            &["a", "_x.1", "é-"],
            &["1", ":a", "a:b"],
        ),
        // libxml2 quirks reproduced: `\P` is not negated inside brackets,
        // `^` inside a subtraction is ignored, and a stray `-` is skipped.
        ("[\\P{L}]", &["a"], &["1"]),
        ("[a-z-[^aeiou]]", &["a", "b"], &["A"]),
        ("[a-c-e]", &["a", "e"], &["d", "-"]),
        ("[\\--x]", &["x"], &["-", "a"]),
        ("[^a-]", &["-", "b"], &[]),
    ];
    for (pattern, matching, failing) in cases {
        check(pattern, matching, failing);
    }
}

#[test]
fn escapes_and_properties() {
    let cases: &[(&str, &[&str], &[&str])] = &[
        (".", &["a", " ", "😀"], &["\n", "\r", ""]),
        ("\\s", &[" ", "\t", "\n", "\r"], &["a", "\u{a0}"]),
        ("\\S", &["a"], &[" "]),
        ("\\i", &["a", "_", ":", "\u{4e00}"], &["1", "-"]),
        ("\\I", &["1", "-"], &["a"]),
        (
            "\\c",
            &["a", "1", ".", "-", "\u{b7}", "\u{300}"],
            &[" ", "\u{37e}"],
        ),
        ("\\C", &[" "], &["a"]),
        ("\\d", &["0", "٣"], &["a", "²"]),
        ("\\D", &["a"], &["0"]),
        ("\\w", &["a", "1", "+"], &[" ", ".", "\u{7f}"]),
        ("\\W", &[" ", ".", "-"], &["a", "+"]),
        (
            "\\n\\r\\t\\\\\\|\\.\\?\\*\\+\\(\\)\\{\\}\\-\\[\\]\\^",
            &["\n\r\t\\|.?*+(){}-[]^"],
            &[],
        ),
        (
            "\\!\\\"\\#\\$\\%\\,\\/\\:\\;\\=\\>\\@\\`\\~",
            &["!\"#$%,/:;=>@`~"],
            &[],
        ),
        ("\\u00e9\\u6F22", &["é漢"], &["e漢"]),
        ("\\uD83D\\uDE00", &["😀"], &[]),
        // Only the endpoints U+4E00 and U+9FA5 of the CJK range are `L`.
        ("\\p{L}", &["a", "Z", "\u{4e00}"], &["1", " ", "漢"]),
        ("\\p{Lu}\\p{Ll}\\p{Lt}\\p{Lm}\\p{Lo}", &["Aaǅʰא"], &[]),
        (
            "\\p{M}\\p{Mn}\\p{Mc}\\p{Me}",
            &["\u{300}\u{300}\u{903}\u{488}"],
            &[],
        ),
        ("\\p{N}\\p{Nd}\\p{Nl}\\p{No}", &["11Ⅰ²"], &[]),
        (
            "\\p{P}\\p{Pc}\\p{Pd}\\p{Ps}\\p{Pe}\\p{Pi}\\p{Pf}\\p{Po}",
            &["!_-()«»!"],
            &[],
        ),
        ("\\p{Z}\\p{Zs}\\p{Zl}\\p{Zp}", &["  \u{2028}\u{2029}"], &[]),
        ("\\p{S}\\p{Sm}\\p{Sc}\\p{Sk}\\p{So}", &["++$^©"], &[]),
        ("\\p{C}\\p{Cc}\\p{Cf}", &["\t\t\u{ad}"], &[]),
        // libxml2's Unicode 4.0.1 tables list only the endpoints of
        // first/last ranges, so private use U+E001 is not `Co`.
        ("\\p{Co}", &["\u{e000}", "\u{f8ff}"], &["\u{e001}"]),
        ("\\p{Cn}", &[], &["\u{378}", "a"]),
        ("\\P{L}", &["1", " "], &["a"]),
        ("\\p{IsBasicLatin}+", &["abc~"], &["é"]),
        ("\\p{IsGreek}\\p{IsGreekandCoptic}", &["αβ"], &["ab"]),
        ("\\p{IsArabicPresentationForms-A}", &["\u{fb50}"], &["a"]),
        ("\\P{IsBasicLatin}", &["é"], &["a"]),
        ("\\p{IsCJKUnifiedIdeographs}", &["漢"], &["a"]),
    ];
    for (pattern, matching, failing) in cases {
        check(pattern, matching, failing);
    }
}

#[test]
fn non_xml_characters_never_match() {
    check(".", &[], &["\u{1}", "\u{fffe}", "\0"]);
    check("[^a]", &["b"], &["\u{1}"]);
}

#[test]
fn syntax_errors() {
    let cases = [
        ("(", 1, "expecting ')'"),
        ("a)", 1, "extra characters"),
        ("a**", 2, "extra characters"),
        ("*", 0, "extra characters"),
        ("]", 0, "extra characters"),
        ("[", 1, "expecting ']'"),
        ("[a", 2, "expecting ']'"),
        ("[]]", 2, "extra characters"),
        ("[a-]b]", 5, "extra characters"),
        ("[z-a]", 3, "end of range is before start of range"),
        ("[a-[b]", 6, "']' expected"),
        ("a{", 2, "improper quantifier"),
        ("a{,2}", 2, "improper quantifier"),
        ("a{2", 3, "unterminated quantifier"),
        ("a{2,x}", 4, "improper quantifier"),
        ("a{2147483648}", 2, "improper quantifier"),
        ("x{", 2, "improper quantifier"),
        ("\\", 1, "invalid escape"),
        ("\\a", 1, "invalid escape"),
        ("\\p", 2, "expecting '{'"),
        ("\\p{X}", 3, "unknown char property"),
        ("\\p{Lx}", 4, "expecting '}'"),
        ("\\p{IsFoo}", 8, "unknown block name"),
        ("\\p{Is}", 5, "unknown block name"),
        ("\\p{Ix}", 4, "IsXXXX expected"),
        ("\\u12", 4, "expecting hex digit"),
        ("\\uD800", 6, "invalid low surrogate pair code unit"),
        ("\\uD800\\u0041", 11, "invalid low surrogate pair code unit"),
        ("[\\a]", 2, "invalid escape"),
        ("[a-\\q]", 4, "invalid escape"),
        ("[;^((-", 6, "expecting the end of a char range"),
    ];
    for (pattern, offset, message) in cases {
        assert_eq!(
            Regex::new(pattern).err(),
            Some(Error::Syntax { offset, message }),
            "{pattern:?}"
        );
    }
    let open = |n: usize| "(".repeat(n) + &")".repeat(n);
    assert!(Regex::new(&open(50)).is_ok());
    assert!(matches!(
        Regex::new(&open(51)),
        Err(Error::Syntax { offset: 51, .. })
    ));
    // libxml2 does not limit nested subtractions; here they share the limit.
    let subtractions = "[a".to_owned() + &"-[a".repeat(60) + &"]".repeat(61);
    assert!(matches!(
        Regex::new(&subtractions),
        Err(Error::Syntax { .. })
    ));
}

#[test]
fn patterns_end_at_nul() {
    check("ab\0c", &["ab"], &["ab\0c"]);
}

#[test]
fn budget_bounds_counted_repetition() {
    for pattern in [
        ".{2147483647}",
        "a{100000}",
        "(a{1000}){1000}",
        "((){1000000}){1000000}",
        "(((a{0,40}){0,40}){0,40})",
    ] {
        assert_eq!(
            Regex::new(pattern).err(),
            Some(Error::TooComplex),
            "{pattern:?}"
        );
    }
    assert!(Regex::with_budget("a{10}", 21).is_ok());
    assert_eq!(
        Regex::with_budget("a{10}", 20).err(),
        Some(Error::TooComplex)
    );
    assert_eq!(
        Regex::with_budget("\\p{L}", 10).err(),
        Some(Error::TooComplex)
    );
    assert!(Regex::with_budget("a{49000}", DEFAULT_BUDGET).is_ok());
    check("a{5000}", &[&"a".repeat(5000)], &[&"a".repeat(4999)]);
}

#[test]
fn matching_is_linear_on_pathological_patterns() {
    let long = "a".repeat(200_000);
    let cases = [
        "(a*)*b",
        "(a|a)*b",
        "(a?){30}a{30}(a|a)*b",
        "((a+)+)+b",
        "(.*){20}b",
        "(\\p{L}|\\w|.)*b",
    ];
    for pattern in cases {
        let regex = Regex::new(pattern).unwrap();
        let start = Instant::now();
        assert!(!regex.is_match(&long), "{pattern}");
        assert!(regex.is_match(&(long.clone() + "b")), "{pattern}");
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{pattern}: {:?}",
            start.elapsed()
        );
    }
}

#[test]
fn cache_overflow_keeps_results() {
    // Thousands of DFA states (one per position of the counted run) overflow
    // the cache and force rebuilds mid-input.
    let regex = Regex::new("[ab]*a[ab]{12}").unwrap();
    let mut value: String = (0..5000)
        .map(|i| if i * 7 % 3 == 0 { 'a' } else { 'b' })
        .collect();
    let expected = value.as_bytes()[value.len() - 13] == b'a';
    assert_eq!(regex.is_match(&value), expected);
    value.push_str("abbbbbbbbbbbb");
    assert!(regex.is_match(&value));
    assert!(!regex.is_match("abbbbbbbbbbb"));
}

#[test]
fn regex_api() {
    let regex = Regex::new("a+").unwrap();
    let copy = regex.clone();
    assert_eq!(copy.as_str(), "a+");
    assert!(copy.is_match("aa") && regex.is_match("a"));
    assert_eq!(format!("{regex:?}"), "Regex(\"a+\")");
    let shared = std::sync::Arc::new(regex);
    let threads: Vec<_> = (0..4)
        .map(|i| {
            let regex = shared.clone();
            std::thread::spawn(move || {
                (0..500).all(|n| regex.is_match(&"a".repeat(1 + (n + i) % 40)))
            })
        })
        .collect();
    assert!(threads.into_iter().all(|t| t.join().unwrap()));
    assert_eq!(
        Error::Syntax {
            offset: 3,
            message: "boom"
        }
        .to_string(),
        "invalid regular expression at byte 3: boom"
    );
    assert_eq!(
        Error::TooComplex.to_string(),
        "regular expression exceeds the compile budget"
    );
}

/// libxml2's own `test/regexp` suite (`result/regexp/*` holds both the
/// patterns and libxml2's verdicts).
#[test]
fn libxml2_regexp_suite() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/libxml2-regexp");
    let (mut checked, mut differences) = (0, Vec::new());
    for entry in std::fs::read_dir(dir).unwrap() {
        let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
        let mut lines = text.lines().peekable();
        let mut regex = None;
        while let Some(line) = lines.next() {
            if let Some(pattern) = line.strip_prefix("Regexp: ") {
                let rejected = lines.peek() == Some(&"   failed to compile");
                let compiled = Regex::new(pattern);
                if compiled.is_ok() == rejected {
                    differences.push(format!("{pattern:?}: {:?}", compiled.as_ref().err()));
                }
                regex = compiled.ok();
            } else if let (Some((value, verdict)), Some(regex)) = (line.rsplit_once(": "), &regex) {
                checked += 1;
                if regex.is_match(value) != (verdict == "Ok") {
                    differences.push(format!(
                        "{:?} on {value:?}: libxml2 {verdict}",
                        regex.as_str()
                    ));
                }
            }
        }
    }
    // libxml2 counts `.{2147483647}` without expanding it; here it exceeds
    // the budget.
    assert_eq!(differences, [r#"".{2147483647}": Some(TooComplex)"#]);
    assert!(checked > 100, "{checked}");
}
