//! Exact decimals as libxml2 2.15.4 normalizes them: arbitrary precision,
//! no leading integral or trailing fractional zeros.

use std::cmp::Ordering;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(super) struct Decimal {
    /// Never set for zero.
    negative: bool,
    /// How many of `digits` precede the decimal point.
    integral: u32,
    /// ASCII digits: `0.05` is `"05"` with `integral` 0.
    digits: Box<str>,
}

/// The sign and the rest of `text`.
fn sign(text: &str) -> (bool, &str) {
    match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    }
}

impl Decimal {
    fn new(negative: bool, integral: &str, fraction: &str) -> Option<Self> {
        let integral = integral.trim_start_matches('0');
        let fraction = fraction.trim_end_matches('0');
        let digits: Box<str> = [integral, fraction].concat().into();
        Some(Self {
            negative: negative && !digits.is_empty(),
            integral: u32::try_from(integral.len()).ok()?,
            digits,
        })
    }

    /// `xs:decimal`: an optional sign, then at least one digit with an
    /// optional point (`0.` and `.5` are valid, `.` is not).
    pub(super) fn parse(text: &str) -> Option<Self> {
        let (negative, rest) = sign(text);
        let (integral, fraction) = rest.split_once('.').unwrap_or((rest, ""));
        let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
        if rest.is_empty() || !digits(integral) || !digits(fraction) {
            return None;
        }
        if integral.is_empty() && fraction.is_empty() {
            return None;
        }
        Self::new(negative, integral, fraction)
    }

    /// `xs:integer`: an optional sign and at least one digit.
    pub(super) fn parse_integer(text: &str) -> Option<Self> {
        let (negative, digits) = sign(text);
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        Self::new(negative, digits, "")
    }

    /// The integral magnitude, saturating like `strtoul`.
    pub(super) fn to_u64(&self) -> u64 {
        let integral = &self.digits[..self.integral as usize];
        match integral.is_empty() {
            true => 0,
            false => integral.parse().unwrap_or(u64::MAX),
        }
    }

    /// Significant digits, at least one (`totalDigits`).
    pub(super) fn total_digits(&self) -> u64 {
        self.digits.len().max(1) as u64
    }

    /// The canonical form: `-1.5`, `0.0` and `12.0` for `xs:decimal`, and
    /// digits alone for the integer types.
    pub(super) fn canonical(&self, integer: bool) -> String {
        let (integral, fraction) = self.digits.split_at(self.integral as usize);
        let integral = if integral.is_empty() { "0" } else { integral };
        let sign = if self.negative { "-" } else { "" };
        match (integer, fraction.is_empty()) {
            (true, _) => format!("{sign}{integral}"),
            (false, true) => format!("{sign}{integral}.0"),
            (false, false) => format!("{sign}{integral}.{fraction}"),
        }
    }

    /// Digits after the point (`fractionDigits`).
    pub(super) fn fraction_digits(&self) -> u64 {
        (self.digits.len() - self.integral as usize) as u64
    }
}

impl Ord for Decimal {
    fn cmp(&self, other: &Self) -> Ordering {
        let magnitude = self
            .integral
            .cmp(&other.integral)
            .then_with(|| self.digits.cmp(&other.digits));
        match (self.negative, other.negative) {
            (false, false) => magnitude,
            (true, true) => magnitude.reverse(),
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
        }
    }
}

impl PartialOrd for Decimal {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
