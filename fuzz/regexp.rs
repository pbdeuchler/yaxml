//! XML Schema regular expressions on arbitrary text: the first line is the
//! pattern, each further line a value. A small budget derived from the
//! pattern either reports `TooComplex` or compiles the same language.

use yaxml::regexp::{Error, Regex};

const BUDGET: usize = 20_000;

fn xml_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | ' '..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..)
}

/// A pattern matching exactly `value`, written with `\u` escapes.
fn literal(value: &str) -> String {
    let mut units = [0; 2];
    value
        .chars()
        .flat_map(|c| c.encode_utf16(&mut units).to_vec())
        .map(|unit| format!("\\u{unit:04X}"))
        .collect()
}

pub(super) fn exercise(input: &[u8]) {
    let Ok(text) = std::str::from_utf8(input) else {
        return;
    };
    let (pattern, values) = text.split_once('\n').unwrap_or((text, ""));
    let pattern = pattern.split('\0').next().unwrap_or("");
    let regex = match Regex::with_budget(pattern, BUDGET) {
        Ok(regex) => regex,
        Err(Error::Syntax { offset, message }) => {
            assert!(
                offset <= pattern.len() && !message.is_empty(),
                "{pattern:?}"
            );
            return;
        }
        Err(error) => {
            assert_eq!(error, Error::TooComplex);
            return;
        }
    };
    assert_eq!(regex.as_str(), pattern);
    assert!(format!("{regex:?}").starts_with("Regex"));
    let small = pattern.bytes().fold(0usize, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(byte.into())
    }) % 2048;
    let tight = match Regex::with_budget(pattern, small) {
        Ok(tight) => Some(tight),
        Err(error) => {
            assert_eq!(error, Error::TooComplex);
            None
        }
    };
    // More budget never changes the result; a fresh cache agrees with a warm one.
    let roomy = Regex::with_budget(pattern, 2 * BUDGET).expect("a larger budget compiles");
    let star = Regex::with_budget(&format!("({pattern})*"), 4 * BUDGET);
    for value in values.split('\n').take(16) {
        let matched = regex.is_match(value);
        assert_eq!(regex.is_match(value), matched, "{pattern:?} {value:?}");
        assert_eq!(roomy.is_match(value), matched, "{pattern:?} {value:?}");
        if let Some(tight) = &tight {
            assert_eq!(tight.is_match(value), matched, "{pattern:?} {value:?}");
        }
        assert!(
            !matched || value.chars().all(xml_char),
            "{pattern:?} {value:?}"
        );
        // A star accepts the empty string and repetitions of a match; an
        // alternation with a literal accepts that literal.
        if let Ok(star) = &star {
            assert!(star.is_match(""), "({pattern:?})*");
            assert!(
                !matched || star.is_match(&value.repeat(2)),
                "({pattern:?})* {value:?}"
            );
        }
        if let Ok(either) =
            Regex::with_budget(&format!("({pattern})|{}", literal(value)), 4 * BUDGET)
        {
            let expected = matched || value.chars().all(xml_char);
            assert_eq!(
                either.is_match(value),
                expected,
                "{pattern:?}|literal {value:?}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::exercise;

    #[test]
    fn seeds_keep_every_invariant() {
        for input in [
            "a{2,3}\na\naa\naaaa",
            "[\\i-[:]][\\c-[:]]*\nx\n:x\né",
            "(a|b)*c{0,2}\nababcc\n\u{1}",
            "\\p{IsBasicLatin}+|\\P{L}\nabc\n1\n\u{e9}",
            "((a?){20}){5}\naaaa",
            "(\n",
            "a{99999}\na",
            "😀{2}\n😀😀",
        ] {
            exercise(input.as_bytes());
        }
    }
}
