//! Dates, times and durations as libxml2 2.15.4 `xmlschemastypes.c` parses,
//! adds and orders them, including its floating-point seconds and its
//! partial-order quirks (`xmlSchemaCompareDates`).

use std::cmp::Ordering;

use super::{Comparison, Type};

const SECS_PER_DAY: i64 = 86_400;
const DAYS_IN_MONTH: [[i64; 12]; 2] = [
    [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31],
    [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31],
];
const DAYS_BEFORE_MONTH: [[i64; 12]; 2] = [
    [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334],
    [0, 31, 60, 91, 121, 152, 182, 213, 244, 274, 305, 335],
];

/// `xmlSchemaValDate`: absent fields are zero.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Date {
    year: i64,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: f64,
    /// Whether a time zone was given; `offset` is in minutes.
    zoned: bool,
    offset: i16,
}

/// `xmlSchemaValDuration`: years are months, and hours and minutes are
/// seconds, carried into days.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Duration {
    months: i64,
    days: i64,
    seconds: f64,
}

fn leap(year: i64) -> usize {
    usize::from((year % 4 == 0 && year % 100 != 0) || year % 400 == 0)
}

fn days_in_month(year: i64, month: i64) -> i64 {
    DAYS_IN_MONTH[leap(year)][month as usize - 1]
}

/// A cursor over the collapsed lexical form; reads past the end yield NUL,
/// like the C string the libxml2 parser walks.
struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Cursor<'_> {
    fn at(&self, offset: usize) -> u8 {
        self.bytes.get(self.pos + offset).copied().unwrap_or(0)
    }

    fn digits(&mut self, count: usize) -> Option<u8> {
        let mut value = 0;
        for _ in 0..count {
            let digit = self.at(0).checked_sub(b'0').filter(|d| *d < 10)?;
            value = value * 10 + digit;
            self.pos += 1;
        }
        Some(value)
    }

    /// `PARSE_FLOAT`: two digits, then an optional fraction of at least
    /// one digit, accumulated as libxml2 does.
    fn seconds(&mut self) -> Option<f64> {
        let mut value = f64::from(self.digits(2)?);
        if self.at(0) == b'.' {
            self.pos += 1;
            if !self.at(0).is_ascii_digit() {
                return None;
            }
            let mut scale = 1.0;
            while self.at(0).is_ascii_digit() {
                scale /= 10.0;
                value += f64::from(self.at(0) - b'0') * scale;
                self.pos += 1;
            }
        }
        Some(value)
    }

    fn at_zone(&self) -> bool {
        matches!(self.at(0), 0 | b'Z' | b'+' | b'-')
    }
}

impl Date {
    /// `_xmlSchemaParseGYear`: at least four digits, no leading zero beyond
    /// four, not zero.
    fn parse_year(&mut self, cursor: &mut Cursor<'_>) -> Option<()> {
        let negative = cursor.at(0) == b'-';
        cursor.pos += usize::from(negative);
        let first = cursor.at(0);
        let mut count = 0;
        while cursor.at(0).is_ascii_digit() {
            let digit = i64::from(cursor.at(0) - b'0');
            self.year = self.year.checked_mul(10)?.checked_add(digit)?;
            cursor.pos += 1;
            count += 1;
        }
        if count < 4 || (count > 4 && first == b'0') || self.year == 0 {
            return None;
        }
        if negative {
            self.year = -self.year;
        }
        Some(())
    }

    fn parse_month(&mut self, cursor: &mut Cursor<'_>) -> Option<()> {
        self.month = cursor.digits(2).filter(|m| (1..=12).contains(m))?;
        Some(())
    }

    fn parse_day(&mut self, cursor: &mut Cursor<'_>) -> Option<()> {
        self.day = cursor.digits(2).filter(|d| (1..=31).contains(d))?;
        Some(())
    }

    fn valid_time(&self) -> bool {
        let end_of_day = self.hour == 24 && self.minute == 0 && self.second == 0.0;
        ((self.hour < 24 && self.minute < 60 && (0.0..60.0).contains(&self.second)) || end_of_day)
            && (-840..=840).contains(&self.offset)
    }

    fn valid_date(&self) -> bool {
        self.year != 0
            && (1..=12).contains(&self.month)
            && i64::from(self.day) <= days_in_month(self.year, self.month.into())
    }

    /// `_xmlSchemaParseTime`; 24 is allowed as an end-of-day hour.
    fn parse_time(&mut self, cursor: &mut Cursor<'_>) -> Option<()> {
        let hour = cursor.digits(2)?;
        if cursor.at(0) != b':' || hour > 24 {
            return None;
        }
        cursor.pos += 1;
        self.hour = hour;
        self.minute = cursor.digits(2).filter(|m| *m < 60)?;
        if cursor.at(0) != b':' {
            return None;
        }
        cursor.pos += 1;
        self.second = cursor.seconds()?;
        self.valid_time().then_some(())
    }

    /// `_xmlSchemaParseTimeZone`: nothing, `Z` or `±hh:mm` within 14 hours.
    fn parse_zone(&mut self, cursor: &mut Cursor<'_>) -> Option<()> {
        match cursor.at(0) {
            0 => (self.zoned, self.offset) = (false, 0),
            b'Z' => {
                (self.zoned, self.offset) = (true, 0);
                cursor.pos += 1;
            }
            sign @ (b'+' | b'-') => {
                cursor.pos += 1;
                let hours = cursor.digits(2).filter(|h| *h < 24)?;
                if cursor.at(0) != b':' {
                    return None;
                }
                cursor.pos += 1;
                let minutes = cursor.digits(2).filter(|m| *m < 60)?;
                let offset = i16::from(hours) * 60 + i16::from(minutes);
                self.offset = if sign == b'-' { -offset } else { offset };
                if !(-840..=840).contains(&self.offset) {
                    return None;
                }
                self.zoned = true;
            }
            _ => return None,
        }
        Some(())
    }

    /// `RETURN_TYPE_IF_VALID`: `Some(result)` ends parsing, `None` means
    /// that no time zone follows and parsing continues.
    fn finish(&mut self, cursor: &mut Cursor<'_>, found: Type) -> Option<Option<Type>> {
        let start = cursor.pos;
        if cursor.at_zone() {
            if self.parse_zone(cursor).is_none() {
                cursor.pos = start;
                return None;
            }
            return Some((cursor.at(0) == 0).then_some(found));
        }
        // The value is collapsed, so a blank here is followed by more text.
        (cursor.at(0) == b' ').then_some(None)
    }

    /// `xmlSchemaValidateDates` for the collapsed `text`, which must be of
    /// date/time type `ty`.
    pub(super) fn parse(text: &str, ty: Type) -> Option<Self> {
        let mut date = Self::default();
        let found = date.parse_any(&mut Cursor {
            bytes: text.as_bytes(),
            pos: 0,
        })?;
        (found == ty).then_some(date)
    }

    fn parse_any(&mut self, cursor: &mut Cursor<'_>) -> Option<Type> {
        let finished = |result: Option<Option<Type>>| result.flatten();
        if cursor.at(0) == b'-' && cursor.at(1) == b'-' {
            cursor.pos += 2;
            if cursor.at(0) == b'-' {
                cursor.pos += 1;
                self.parse_day(cursor)?;
                return finished(self.finish(cursor, Type::GDay));
            }
            self.parse_month(cursor)?;
            if cursor.at(0) == b'-' {
                let rewind = cursor.pos;
                cursor.pos += 1;
                // A day that turns out to be a `-hh:mm` zone stays set.
                if self.parse_day(cursor).is_some() && cursor.at(0) != b':' {
                    let valid = i64::from(self.day) <= days_in_month(0, self.month.into());
                    return valid.then(|| finished(self.finish(cursor, Type::GMonthDay)))?;
                }
                cursor.pos = rewind;
            }
            return finished(self.finish(cursor, Type::GMonth));
        }
        let start = cursor.pos;
        if cursor.at(0).is_ascii_digit()
            && self.parse_time(cursor).is_some()
            && let Some(result) = self.finish(cursor, Type::Time)
        {
            return result;
        }
        cursor.pos = start;
        if !matches!(cursor.at(0), b'0'..=b'9' | b'-' | b'+') {
            return None;
        }
        self.parse_year(cursor)?;
        for (ty, next) in [
            (
                Type::GYear,
                Self::parse_month as fn(&mut Self, &mut Cursor<'_>) -> Option<()>,
            ),
            (Type::GYearMonth, Self::parse_day),
        ] {
            if let Some(result) = self.finish(cursor, ty) {
                return result;
            }
            if cursor.at(0) != b'-' {
                return None;
            }
            cursor.pos += 1;
            next(self, cursor)?;
        }
        if !self.valid_date() {
            return None;
        }
        if let Some(result) = self.finish(cursor, Type::Date) {
            return result;
        }
        if cursor.at(0) != b'T' {
            return None;
        }
        cursor.pos += 1;
        self.parse_time(cursor)?;
        if cursor.at_zone() {
            self.parse_zone(cursor)?;
        }
        (cursor.at(0) == 0 && self.valid_date() && self.valid_time()).then_some(Type::DateTime)
    }

    /// `_xmlSchemaDateAdd` for a duration of `seconds`, after shifting to
    /// UTC; month and day zero normalize to one.
    fn add_seconds(&self, seconds: f64) -> Self {
        let fquotient = |a: f64, b: f64| (a / b).floor();
        let modulo = |a: f64, b: f64| a - fquotient(a, b) * b;
        let month = i64::from(self.month.max(1));
        let day = i64::from(self.day.max(1));
        let seconds = seconds - f64::from(self.offset) * 60.0;
        let mut r = Self {
            month: modulo((month - 1) as f64, 12.0) as u8 + 1,
            year: self.year + fquotient((month - 1) as f64, 12.0) as i64,
            zoned: self.zoned,
            ..Self::default()
        };
        if r.year == 0 {
            r.year += if self.year > 0 { -1 } else { 1 };
        }
        r.second = self.second + seconds;
        let mut carry = fquotient(r.second as i64 as f64, 60.0) as i64;
        if r.second != 0.0 {
            r.second = modulo(r.second, 60.0);
        }
        carry += i64::from(self.minute);
        r.minute = modulo(carry as f64, 60.0) as u8;
        carry = fquotient(carry as f64, 60.0) as i64 + i64::from(self.hour);
        r.hour = modulo(carry as f64, 24.0) as u8;
        carry = fquotient(carry as f64, 24.0) as i64;
        let mut days = match r.year != 0 && day > days_in_month(r.year, r.month.into()) {
            true => days_in_month(r.year, r.month.into()),
            false => day,
        } + carry;
        loop {
            let step = if days < 1 {
                let previous = i64::from(r.month) - 1;
                let month = modulo((previous - 1) as f64, 12.0) as i64 + 1;
                let mut year = r.year + fquotient((previous - 1) as f64, 12.0) as i64;
                if year == 0 {
                    year -= 1;
                }
                days += days_in_month(year, month.clamp(1, 12));
                -1
            } else if r.year != 0 && days > days_in_month(r.year, r.month.into()) {
                days -= days_in_month(r.year, r.month.into());
                1
            } else {
                break;
            };
            let month = i64::from(r.month) + step;
            r.month = modulo((month - 1) as f64, 12.0) as u8 + 1;
            r.year += fquotient((month - 1) as f64, 12.0) as i64;
            if r.year == 0 {
                r.year += if month < 1 { -1 } else { 1 };
            }
        }
        r.day = days as u8;
        r
    }

    /// `xmlSchemaDateNormalize`: shifts a zoned time, date or dateTime to
    /// UTC and then by `-offset` seconds; other values are unchanged.
    fn normalize(&self, ty: Type, offset: f64) -> Self {
        match matches!(ty, Type::Time | Type::DateTime | Type::Date) && self.offset != 0 {
            true => self.add_seconds(-offset),
            false => *self,
        }
    }

    /// `_xmlSchemaDateCastYMToDays` plus the day of month.
    fn days(&self) -> i64 {
        let month = usize::from(self.month.max(1)) - 1;
        let year = self.year;
        let base = match year <= 0 {
            true => year * 365 + (year + 1) / 4 - (year + 1) / 100 + (year + 1) / 400,
            false => (year - 1) * 365 + (year - 1) / 4 - (year - 1) / 100 + (year - 1) / 400,
        };
        base + DAYS_BEFORE_MONTH[leap(year)][month] + i64::from(self.day)
    }

    /// `TIME_TO_NUMBER`, which counts the offset of a value left zoned.
    fn time(&self) -> f64 {
        (i32::from(self.hour) * 3600 + i32::from(self.minute) * 60 + i32::from(self.offset) * 60)
            as f64
            + self.second
    }

    fn mask(ty: Type) -> u8 {
        match ty {
            Type::DateTime => 0xf,
            Type::Date => 0x7,
            Type::GYear => 0x1,
            Type::GMonth => 0x2,
            // libxml2 gives gDay the gYearMonth mask.
            Type::GDay | Type::GYearMonth => 0x3,
            Type::GMonthDay => 0x6,
            _ => 0x8,
        }
    }
}

/// libxml2's tests for one zoned and one unzoned value: less by the first
/// pair of shifted values, else greater or indeterminate on the same day by
/// the second pair. `None` leaves the decision to the later tests.
fn compare_zoned(first: (Date, Date), second: (Date, Date)) -> Option<Comparison> {
    let ((p, q), (pd, qd)) = (first, (first.0.days(), first.1.days()));
    if pd < qd || (pd == qd && p.time() - q.time() < 0.0) {
        return Some(Comparison::Less);
    }
    if pd > qd {
        return None;
    }
    let (p, q) = second;
    match p.days().cmp(&q.days()) {
        Ordering::Greater => Some(Comparison::Greater),
        Ordering::Equal if p.time() - q.time() > 0.0 => Some(Comparison::Greater),
        Ordering::Equal => Some(Comparison::Indeterminate),
        Ordering::Less => None,
    }
}

/// `xmlSchemaCompareDates`.
pub(super) fn compare_dates(x: &Date, xt: Type, y: &Date, yt: Type) -> Comparison {
    let limit = i64::MAX / 366;
    if [x.year, y.year]
        .iter()
        .any(|year| !(-limit..=limit).contains(year))
    {
        return Comparison::Incomparable;
    }
    // An unzoned value has a zero offset, which makes its shifts by 14
    // hours no-ops in libxml2.
    let (day, utc) = (14.0 * 3600.0, |d: &Date, t| d.normalize(t, 0.0));
    let early = match (x.zoned, y.zoned) {
        (true, false) => compare_zoned(
            (utc(x, xt), y.normalize(yt, day)),
            (utc(x, xt), y.normalize(yt, -day)),
        ),
        (false, true) => compare_zoned(
            (x.normalize(xt, -day), utc(y, yt)),
            (x.normalize(xt, day), utc(y, yt)),
        ),
        _ => None,
    };
    if let Some(result) = early {
        return result;
    }
    if xt == yt {
        let (p, q) = (x.normalize(xt, 0.0), y.normalize(yt, 0.0));
        return match p.days().cmp(&q.days()) {
            Ordering::Equal => match p.time() - q.time() {
                d if d < 0.0 => Comparison::Less,
                d if d > 0.0 => Comparison::Greater,
                _ => Comparison::Equal,
            },
            ordering => ordering.into(),
        };
    }
    let (xmask, ymask) = (Date::mask(xt), Date::mask(yt));
    let seconds = x.second.partial_cmp(&y.second).unwrap_or(Ordering::Equal);
    let fields = [
        (1, x.year.cmp(&y.year)),
        (2, x.month.cmp(&y.month)),
        (4, x.day.cmp(&y.day)),
        (8, (x.hour, x.minute).cmp(&(y.hour, y.minute)).then(seconds)),
    ];
    for (bit, ordering) in fields {
        if (xmask ^ ymask) & bit != 0 {
            return Comparison::Indeterminate;
        }
        if xmask & ymask & bit != 0 && ordering.is_ne() {
            return ordering.into();
        }
    }
    Comparison::Equal
}

impl Duration {
    /// `xmlSchemaValidateDuration` for the collapsed `text`.
    pub(super) fn parse(text: &str) -> Option<Self> {
        const DESIGNATORS: &[u8; 6] = b"YMDHMS";
        let bytes = text.as_bytes();
        let negative = bytes.first() == Some(&b'-');
        let mut pos = usize::from(negative);
        if bytes.get(pos) != Some(&b'P') || pos + 1 == bytes.len() {
            return None;
        }
        pos += 1;
        let (mut duration, mut seq, mut seconds, mut fraction) = (Self::default(), 0, 0i64, 0.0);
        while pos < bytes.len() {
            if seq >= DESIGNATORS.len() || bytes[pos] == b' ' {
                return None;
            }
            if bytes[pos] == b'T' {
                if seq > 3 {
                    return None;
                }
                pos += 1;
                seq = 3;
            } else if seq == 3 {
                return None;
            }
            let (mut number, mut digits, mut fractional) = (0i64, false, false);
            while let Some(digit) = bytes.get(pos).filter(|b| b.is_ascii_digit()) {
                number = number
                    .checked_mul(10)?
                    .checked_add(i64::from(digit - b'0'))?;
                digits = true;
                pos += 1;
            }
            if bytes.get(pos) == Some(&b'.') {
                pos += 1;
                fractional = true;
                let mut scale = 1.0;
                while let Some(digit) = bytes.get(pos).filter(|b| b.is_ascii_digit()) {
                    scale /= 10.0;
                    fraction += f64::from(digit - b'0') * scale;
                    digits = true;
                    pos += 1;
                }
            }
            while bytes.get(pos) != Some(&DESIGNATORS[seq]) {
                seq += 1;
                if seq == 3 || seq == DESIGNATORS.len() {
                    return None;
                }
            }
            pos += 1;
            if !digits || (fractional && seq != 5) {
                return None;
            }
            let carry = |days: &mut i64, per_day: i64| -> Option<i64> {
                *days = days.checked_add(number / per_day)?;
                Some(number % per_day)
            };
            match seq {
                0 => duration.months = number.checked_mul(12)?,
                1 => duration.months = duration.months.checked_add(number)?,
                2 => duration.days = number,
                3 => seconds = carry(&mut duration.days, 24)? * 3600,
                4 => seconds += carry(&mut duration.days, 1440)? * 60,
                _ => seconds += carry(&mut duration.days, SECS_PER_DAY)?,
            }
            seq += 1;
        }
        duration.days = duration.days.checked_add(seconds / SECS_PER_DAY)?;
        duration.seconds = (seconds % SECS_PER_DAY) as f64 + fraction;
        if negative {
            duration = Self {
                months: -duration.months,
                days: -duration.days,
                seconds: -duration.seconds,
            };
        }
        Some(duration)
    }
}

/// `xmlSchemaCompareDurations`: months against days use the shortest and
/// longest month lengths, and leave the rest indeterminate.
pub(super) fn compare_durations(x: &Duration, y: &Duration) -> Comparison {
    const DAY_RANGE: [[i64; 12]; 2] = [
        [0, 28, 59, 89, 120, 150, 181, 212, 242, 273, 303, 334],
        [0, 31, 62, 92, 123, 153, 184, 215, 245, 276, 306, 337],
    ];
    // Wrapping arithmetic where libxml2 overflows `long`.
    let months = x.months.wrapping_sub(y.months);
    let mut seconds = x.seconds - y.seconds;
    let carry = (seconds / SECS_PER_DAY as f64) as i64;
    seconds -= carry as f64 * SECS_PER_DAY as f64;
    let days = x.days.wrapping_sub(y.days).wrapping_add(carry);
    if months == 0 {
        return match (days, seconds) {
            (0, 0.0) => Comparison::Equal,
            (0, s) if s < 0.0 => Comparison::Less,
            (0, _) => Comparison::Greater,
            (d, _) => d.cmp(&0).into(),
        };
    }
    let (months, days, invert) = if months > 0 {
        if days >= 0 && seconds >= 0.0 {
            return Comparison::Greater;
        }
        (months, days.wrapping_neg(), false)
    } else if days <= 0 && seconds <= 0.0 {
        return Comparison::Less;
    } else {
        (months.wrapping_neg(), days, true)
    };
    let years = months / 12;
    let (mut min, mut max) = (0, 0);
    if years != 0 {
        if !(0..=i64::MAX / 366).contains(&years) {
            return Comparison::Incomparable;
        }
        max = 365 * years + (years + 3) / 4;
        min = max - 1;
    }
    min += DAY_RANGE[0][(months % 12) as usize];
    max += DAY_RANGE[1][(months % 12) as usize];
    let result = if max == min && max == days {
        Comparison::Equal
    } else if max < days {
        Comparison::Less
    } else if min > days {
        Comparison::Greater
    } else {
        return Comparison::Indeterminate;
    };
    if invert { result.reverse() } else { result }
}
