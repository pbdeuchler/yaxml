//! XML character classes, libxml2's `chvalid.h`.
//!
//! Each predicate takes a code point as `u32`, `char` or `u8`, so one function
//! covers libxml2's function, its `Q` macro and, for a byte, its `_ch` macro;
//! surrogates and values above U+10FFFF belong to no class. [`is_char`],
//! [`is_blank`] and [`is_pubid_char`] are the XML 1.0 Fifth Edition `Char`,
//! `S` and `PubidChar` productions the parser itself uses. The others are the
//! XML 1.0 Fourth Edition Appendix B classes (`BaseChar`, `Ideographic`,
//! `CombiningChar`, `Digit`, `Extender`) that libxml2 keeps for
//! `XML_PARSE_OLD10` names; Fifth Edition names are wider.
//!
//! ```
//! use yaxml::chars;
//! assert!(chars::is_char('\u{e9}') && !chars::is_char(0xfffe_u32));
//! assert!(chars::is_base_char('A') && chars::is_ideographic(0x4e00_u32));
//! assert!(chars::is_blank(b'\t') && !chars::is_pubid_char('\t'));
//! ```

use crate::regexp::unicode::{BASE_CHAR, COMBINING, DIGIT, EXTENDER, IDEOGRAPHIC};

/// `Char` (`xmlIsChar`, `xmlIsCharQ`, `xmlIsChar_ch`).
pub fn is_char(c: impl Into<u32>) -> bool {
    char::from_u32(c.into()).is_some_and(crate::escape::xml_char)
}

/// White space `S`: space, tab, line feed, carriage return (`xmlIsBlank`).
pub fn is_blank(c: impl Into<u32>) -> bool {
    matches!(c.into(), 0x20 | 0x9 | 0xa | 0xd)
}

/// `PubidChar` (`xmlIsPubidChar`).
pub fn is_pubid_char(c: impl Into<u32>) -> bool {
    char::from_u32(c.into()).is_some_and(crate::dtd::pubid_char)
}

/// `BaseChar` (`xmlIsBaseChar`).
pub fn is_base_char(c: impl Into<u32>) -> bool {
    within(c.into(), BASE_CHAR)
}

/// `Ideographic` (`xmlIsIdeographic`).
pub fn is_ideographic(c: impl Into<u32>) -> bool {
    within(c.into(), IDEOGRAPHIC)
}

/// `CombiningChar` (`xmlIsCombining`).
pub fn is_combining(c: impl Into<u32>) -> bool {
    within(c.into(), COMBINING)
}

/// `Digit` (`xmlIsDigit`).
pub fn is_digit(c: impl Into<u32>) -> bool {
    within(c.into(), DIGIT)
}

/// `Extender` (`xmlIsExtender`).
pub fn is_extender(c: impl Into<u32>) -> bool {
    within(c.into(), EXTENDER)
}

/// Whether `c` lies in one of the sorted, disjoint inclusive `ranges`
/// (`xmlCharInRange`).
fn within(c: u32, ranges: &[(u32, u32)]) -> bool {
    let index = ranges.partition_point(|&(_, high)| high < c);
    ranges.get(index).is_some_and(|&(low, _)| low <= c)
}
