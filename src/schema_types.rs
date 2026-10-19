//! XML Schema 1.0 built-in datatypes (Part 2) as libxml2 2.15.4
//! `xmlschemastypes.c` validates, compares and constrains them, for use by
//! XML Schema and RELAX NG validation (see `docs/schema-types.md`).
//!
//! [`Type::parse`] normalizes whitespace and computes a [`Value`];
//! [`Value::compare`] orders values partially; a [`Facet`] constrains them.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::fmt;

use crate::name::{name_char, name_start};
use crate::regexp::{self, Regex};

mod decimal;
mod temporal;

use decimal::Decimal;
use temporal::{Date, Duration};

const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";

/// A built-in simple type.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Type {
    AnySimpleType,
    String,
    NormalizedString,
    Token,
    Language,
    NmToken,
    NmTokens,
    Name,
    NCName,
    Id,
    IdRef,
    IdRefs,
    Entity,
    Entities,
    QName,
    Notation,
    AnyUri,
    Boolean,
    Decimal,
    Integer,
    NonPositiveInteger,
    NegativeInteger,
    Long,
    Int,
    Short,
    Byte,
    NonNegativeInteger,
    UnsignedLong,
    UnsignedInt,
    UnsignedShort,
    UnsignedByte,
    PositiveInteger,
    Float,
    Double,
    Duration,
    DateTime,
    Time,
    Date,
    GYearMonth,
    GYear,
    GMonthDay,
    GDay,
    GMonth,
    HexBinary,
    Base64Binary,
}

/// Each type, in declaration order, with its name and base type.
const TYPES: [(Type, &str, Type); 45] = {
    use Type::*;
    [
        (AnySimpleType, "anySimpleType", AnySimpleType),
        (String, "string", AnySimpleType),
        (NormalizedString, "normalizedString", String),
        (Token, "token", NormalizedString),
        (Language, "language", Token),
        (NmToken, "NMTOKEN", Token),
        (NmTokens, "NMTOKENS", AnySimpleType),
        (Name, "Name", Token),
        (NCName, "NCName", Name),
        (Id, "ID", NCName),
        (IdRef, "IDREF", NCName),
        (IdRefs, "IDREFS", AnySimpleType),
        (Entity, "ENTITY", NCName),
        (Entities, "ENTITIES", AnySimpleType),
        (QName, "QName", AnySimpleType),
        (Notation, "NOTATION", AnySimpleType),
        (AnyUri, "anyURI", AnySimpleType),
        (Boolean, "boolean", AnySimpleType),
        (Decimal, "decimal", AnySimpleType),
        (Integer, "integer", Decimal),
        (NonPositiveInteger, "nonPositiveInteger", Integer),
        (NegativeInteger, "negativeInteger", NonPositiveInteger),
        (Long, "long", Integer),
        (Int, "int", Long),
        (Short, "short", Int),
        (Byte, "byte", Short),
        (NonNegativeInteger, "nonNegativeInteger", Integer),
        (UnsignedLong, "unsignedLong", NonNegativeInteger),
        (UnsignedInt, "unsignedInt", UnsignedLong),
        (UnsignedShort, "unsignedShort", UnsignedInt),
        (UnsignedByte, "unsignedByte", UnsignedShort),
        (PositiveInteger, "positiveInteger", NonNegativeInteger),
        (Float, "float", AnySimpleType),
        (Double, "double", AnySimpleType),
        (Duration, "duration", AnySimpleType),
        (DateTime, "dateTime", AnySimpleType),
        (Time, "time", AnySimpleType),
        (Date, "date", AnySimpleType),
        (GYearMonth, "gYearMonth", AnySimpleType),
        (GYear, "gYear", AnySimpleType),
        (GMonthDay, "gMonthDay", AnySimpleType),
        (GDay, "gDay", AnySimpleType),
        (GMonth, "gMonth", AnySimpleType),
        (HexBinary, "hexBinary", AnySimpleType),
        (Base64Binary, "base64Binary", AnySimpleType),
    ]
};

/// Inclusive bounds of the integer types; empty is unbounded.
const INTEGER_RANGES: &[(Type, &str, &str)] = &[
    (Type::Integer, "", ""),
    (Type::NonPositiveInteger, "", "0"),
    (Type::NegativeInteger, "", "-1"),
    (Type::Long, "-9223372036854775808", "9223372036854775807"),
    (Type::Int, "-2147483648", "2147483647"),
    (Type::Short, "-32768", "32767"),
    (Type::Byte, "-128", "127"),
    (Type::NonNegativeInteger, "0", ""),
    (Type::UnsignedLong, "0", "18446744073709551615"),
    (Type::UnsignedInt, "0", "4294967295"),
    (Type::UnsignedShort, "0", "65535"),
    (Type::UnsignedByte, "0", "255"),
    (Type::PositiveInteger, "1", ""),
];

/// A `whiteSpace` facet value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WhiteSpace {
    Preserve,
    /// Tab, newline and carriage return become spaces.
    Replace,
    /// As `Replace`, then runs of spaces become one and the ends are trimmed.
    Collapse,
}

/// Applies `whitespace` to `value`, borrowing when nothing changes.
pub fn normalize(value: &str, whitespace: WhiteSpace) -> Cow<'_, str> {
    const CONTROLS: [char; 3] = ['\t', '\n', '\r'];
    match whitespace {
        WhiteSpace::Preserve => Cow::Borrowed(value),
        WhiteSpace::Replace if !value.contains(CONTROLS) => Cow::Borrowed(value),
        WhiteSpace::Replace => Cow::Owned(value.replace(CONTROLS, " ")),
        WhiteSpace::Collapse
            if !value.contains(CONTROLS)
                && !value.contains("  ")
                && !value.starts_with(' ')
                && !value.ends_with(' ') =>
        {
            Cow::Borrowed(value)
        }
        WhiteSpace::Collapse => Cow::Owned(
            value
                .split([' ', '\t', '\n', '\r'])
                .filter(|item| !item.is_empty())
                .collect::<Vec<_>>()
                .join(" "),
        ),
    }
}

/// What `QName`, `NOTATION`, `ENTITY` and `ENTITIES` values need from the
/// document. Without a context, `QName` prefixes stay unresolved (the
/// value is the whole lexical form) and the other three never validate,
/// as in libxml2 without a node.
pub trait Context {
    /// The namespace bound to `prefix`, `""` meaning the default namespace.
    /// The `xml` prefix is always bound and is not asked for.
    fn namespace(&self, prefix: &str) -> Option<&str>;

    /// Whether a notation of this (lexical) name is declared.
    fn has_notation(&self, _name: &str) -> bool {
        false
    }

    /// Whether an unparsed entity of this name is declared.
    fn has_unparsed_entity(&self, _name: &str) -> bool {
        false
    }
}

/// The lexical form is not valid for the type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidValue;

impl fmt::Display for InvalidValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid lexical value for the datatype")
    }
}

impl std::error::Error for InvalidValue {}

impl Type {
    /// Every built-in type.
    pub fn all() -> impl Iterator<Item = Type> {
        TYPES.iter().map(|row| row.0)
    }

    /// The type of an XML Schema local name such as `"dateTime"`.
    pub fn from_name(name: &str) -> Option<Type> {
        TYPES.iter().find(|row| row.1 == name).map(|row| row.0)
    }

    pub fn name(self) -> &'static str {
        TYPES[self as usize].1
    }

    /// The base type; `anySimpleType` for primitive and list types, and
    /// `None` for `anySimpleType` itself.
    pub fn base(self) -> Option<Type> {
        Some(TYPES[self as usize].2).filter(|_| self != Type::AnySimpleType)
    }

    /// The primitive ancestor (`anySimpleType` for list types).
    pub fn primitive(self) -> Type {
        match self.base() {
            Some(Type::AnySimpleType) | None => self.item().map_or(self, |_| Type::AnySimpleType),
            Some(base) => base.primitive(),
        }
    }

    /// The item type of a list type.
    pub fn item(self) -> Option<Type> {
        match self {
            Type::NmTokens => Some(Type::NmToken),
            Type::IdRefs => Some(Type::IdRef),
            Type::Entities => Some(Type::Entity),
            _ => None,
        }
    }

    /// The whitespace handling `parse` applies.
    pub fn whitespace(self) -> WhiteSpace {
        match self {
            Type::String | Type::AnySimpleType => WhiteSpace::Preserve,
            Type::NormalizedString => WhiteSpace::Replace,
            _ => WhiteSpace::Collapse,
        }
    }

    /// Validates `lexical` and computes its value, like
    /// `xmlSchemaValidatePredefinedType`.
    pub fn parse(
        self,
        lexical: &str,
        context: Option<&dyn Context>,
    ) -> Result<Value, InvalidValue> {
        let text = normalize(lexical, self.whitespace());
        let data = self.data(&text, context).ok_or(InvalidValue)?;
        Ok(Value { ty: self, data })
    }

    fn data(self, text: &str, context: Option<&dyn Context>) -> Option<Data> {
        use Type::*;
        let named = |valid: bool| valid.then(|| Data::Text(text.into()));
        Some(match self {
            AnySimpleType | String | NormalizedString | Token => Data::Text(text.into()),
            Language => return named(language(text)),
            NmToken => return named(!text.is_empty() && text.chars().all(|c| name_char(c, true))),
            Name => return named(is_name(text, true)),
            NCName | Id | IdRef => return named(is_name(text, false)),
            Entity => return named(is_name(text, false) && context?.has_unparsed_entity(text)),
            NmTokens | IdRefs | Entities => {
                let item = self.item()?;
                let mut count = 0;
                for token in text.split(' ').filter(|token| !token.is_empty()) {
                    item.data(token, context)?;
                    count += 1;
                }
                // libxml2 accepts an empty IDREFS.
                return named(
                    (count > 0 || self == IdRefs) && (self != Entities || context.is_some()),
                );
            }
            QName | Notation => qname(self, text, context)?,
            AnyUri => {
                let escaped: std::string::String = text
                    .chars()
                    .map(|c| match c {
                        '\0'..='\u{20}'
                        | '\u{7f}'..
                        | '<'
                        | '>'
                        | '"'
                        | '{'
                        | '}'
                        | '|'
                        | '\\'
                        | '^'
                        | '`'
                        | '\'' => '_',
                        c => c,
                    })
                    .collect();
                if !text.is_empty() {
                    crate::Uri::parse(&escaped).ok()?;
                }
                Data::Text(text.into())
            }
            Boolean => Data::Boolean(match text {
                "true" | "1" => true,
                "false" | "0" => false,
                _ => return None,
            }),
            Decimal => Data::Decimal(decimal::Decimal::parse(text)?),
            Float => Data::Float(float(text)?.parse().ok()?),
            Double => Data::Double(float(text)?.parse().ok()?),
            Duration => Data::Duration(temporal::Duration::parse(text)?),
            HexBinary => {
                let valid =
                    text.len().is_multiple_of(2) && text.bytes().all(|b| b.is_ascii_hexdigit());
                return valid.then(|| Data::Binary {
                    octets: text.len() as u64 / 2,
                    text: text.to_ascii_uppercase().into(),
                });
            }
            Base64Binary => base64(text)?,
            DateTime | Time | Date | GYearMonth | GYear | GMonthDay | GDay | GMonth => {
                Data::Date(temporal::Date::parse(text, self)?)
            }
            _ => {
                let value = decimal::Decimal::parse_integer(text)?;
                let &(_, min, max) = INTEGER_RANGES.iter().find(|row| row.0 == self)?;
                let bound = |bound: &str| decimal::Decimal::parse_integer(bound);
                let low = bound(min).is_none_or(|min| min <= value);
                let high = bound(max).is_none_or(|max| value <= max);
                return (low && high).then_some(Data::Decimal(value));
            }
        })
    }
}

fn is_name(text: &str, colon: bool) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(|c| name_start(c, colon)) && chars.all(|c| name_char(c, colon))
}

/// `xmlSchemaCheckLanguageType`: `[a-zA-Z]{1,8}(-[a-zA-Z0-9]{1,8})*`.
fn language(text: &str) -> bool {
    let mut parts = text.split('-');
    let part = |part: &str, digits: bool| {
        (1..=8).contains(&part.len())
            && part
                .bytes()
                .all(|b| b.is_ascii_alphabetic() || (digits && b.is_ascii_digit()))
    };
    parts.next().is_some_and(|first| part(first, false)) && parts.all(|rest| part(rest, true))
}

/// `QName` and `NOTATION`: two NCNames around an optional colon, resolved
/// through `context` when there is one.
fn qname(ty: Type, text: &str, context: Option<&dyn Context>) -> Option<Data> {
    let (prefix, local) = match text.split_once(':') {
        Some((prefix, local)) => (Some(prefix), local),
        None => (None, text),
    };
    if !prefix.is_none_or(|prefix| is_name(prefix, false)) || !is_name(local, false) {
        return None;
    }
    let Some(context) = context else {
        return (ty == Type::QName).then(|| Data::QName {
            namespace: None,
            local: text.into(),
        });
    };
    let resolve = |prefix: &str| match prefix {
        "xml" => Some(XML_NAMESPACE),
        prefix => context.namespace(prefix),
    };
    // An unprefixed QName takes the default namespace; a NOTATION does not.
    let namespace = match prefix {
        Some(prefix) => Some(resolve(prefix)?),
        None if ty == Type::QName => resolve(""),
        None => None,
    };
    if ty == Type::Notation && !context.has_notation(text) {
        return None;
    }
    Some(Data::QName {
        namespace: namespace.map(Into::into),
        local: local.into(),
    })
}

/// The part of a valid float or double lexical form that Rust parses as
/// C's `sscanf` would: an exponent without digits is dropped.
fn float(text: &str) -> Option<Cow<'_, str>> {
    match text {
        "NaN" => return Some(Cow::Borrowed("NaN")),
        "INF" => return Some(Cow::Borrowed("inf")),
        "-INF" => return Some(Cow::Borrowed("-inf")),
        _ => {}
    }
    let (mantissa, exponent) = text.split_once(['e', 'E']).unwrap_or((text, ""));
    let unsigned = mantissa.strip_prefix(['-', '+']).unwrap_or(mantissa);
    let (integral, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    let exponent_digits = exponent.strip_prefix(['-', '+']).unwrap_or(exponent);
    if (integral.is_empty() && fraction.is_empty())
        || !digits(integral)
        || !digits(fraction)
        || !digits(exponent_digits)
    {
        return None;
    }
    Some(match exponent_digits.is_empty() {
        true => Cow::Borrowed(mantissa),
        false => Cow::Borrowed(text),
    })
}

/// libxml2's `base64Binary`: characters outside the alphabet are ignored,
/// padding must end the data and the padded quantum's unused bits must be
/// zero. The value keeps only alphabet and padding characters.
fn base64(text: &str) -> Option<Data> {
    let decode = |b: u8| match b {
        b'A'..=b'Z' => Some(b - b'A'),
        b'a'..=b'z' => Some(b - b'a' + 26),
        b'0'..=b'9' => Some(b - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        b'=' => Some(64),
        _ => None,
    };
    let bytes = text.as_bytes();
    let data_end = bytes.iter().position(|&b| b == b'=').unwrap_or(bytes.len());
    let symbols = bytes[..data_end]
        .iter()
        .filter(|&&b| decode(b).is_some())
        .count() as u64;
    if bytes[data_end..]
        .iter()
        .any(|&b| decode(b).is_some_and(|d| d < 64))
    {
        return None;
    }
    let padding = bytes[data_end..].iter().filter(|&&b| b == b'=').count();
    let last = bytes[..data_end]
        .iter()
        .rev()
        .find_map(|&b| decode(b))
        .unwrap_or(0);
    let extra = match (padding, symbols % 4) {
        (0, 0) => 0,
        (1, 3) if last & !0x3c == 0 => 2,
        (2, 2) if last & !0x30 == 0 => 1,
        _ => return None,
    };
    Some(Data::Binary {
        octets: 3 * (symbols / 4) + extra,
        text: bytes
            .iter()
            .filter(|&&b| decode(b).is_some())
            .map(|&b| char::from(b))
            .collect(),
    })
}

/// A computed value.
#[derive(Clone, Debug)]
pub struct Value {
    ty: Type,
    data: Data,
}

#[derive(Clone, Debug)]
enum Data {
    /// The normalized text of string, name, URI and list types.
    Text(Box<str>),
    Boolean(bool),
    Decimal(Decimal),
    Float(f32),
    Double(f64),
    Duration(Duration),
    Date(Date),
    /// Upper-case hex digits, or base64 alphabet and padding characters.
    Binary {
        octets: u64,
        text: Box<str>,
    },
    QName {
        namespace: Option<Box<str>>,
        local: Box<str>,
    },
}

/// The result of comparing two values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Comparison {
    Less,
    Equal,
    Greater,
    /// Comparable types, but no order (partially ordered dates and
    /// durations, differing strings or QNames).
    Indeterminate,
    /// The types cannot be compared.
    Incomparable,
}

impl Comparison {
    fn reverse(self) -> Self {
        match self {
            Self::Less => Self::Greater,
            Self::Greater => Self::Less,
            other => other,
        }
    }
}

impl From<Ordering> for Comparison {
    fn from(ordering: Ordering) -> Self {
        match ordering {
            Ordering::Less => Self::Less,
            Ordering::Equal => Self::Equal,
            Ordering::Greater => Self::Greater,
        }
    }
}

fn blank(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r')
}

/// libxml2's `xmlSchemaCompare{Preserve,Replace}CollapseStrings`: `x`
/// against a `y` read as collapsed, inverted for swapped operands except,
/// in the replace variant, on differing non-blank bytes.
fn collapse_order(x: &[u8], y: &[u8], replace: bool, invert: bool) -> Comparison {
    let flip = |ordering: Ordering| match invert {
        true => ordering.reverse().into(),
        false => ordering.into(),
    };
    let skip = |j: &mut usize| *j += y[*j..].iter().take_while(|&&b| blank(b)).count();
    let (mut i, mut j) = (0, 0);
    skip(&mut j);
    while i < x.len() && j < y.len() {
        if blank(y[j]) {
            if !(blank(x[i]) && (replace || x[i] == b' ')) {
                return flip(x[i].cmp(&b' '));
            }
            (i, j) = (i + 1, j + 1);
            skip(&mut j);
        } else if replace && blank(x[i]) {
            return flip(b' '.cmp(&y[j]));
        } else {
            let ordering = x[i].cmp(&y[j]);
            (i, j) = (i + 1, j + 1);
            if ordering.is_ne() {
                return if replace {
                    ordering.into()
                } else {
                    flip(ordering)
                };
            }
        }
    }
    skip(&mut j);
    match (i < x.len(), j < y.len()) {
        (true, _) => flip(Ordering::Greater),
        (_, true) => flip(Ordering::Less),
        _ => Comparison::Equal,
    }
}

/// libxml2's string comparisons by the operands' whitespace handling
/// (preserve for `string`, replace for `normalizedString`, otherwise
/// collapse), on normalized text except for `anySimpleType`.
fn compare_text(x: &str, xt: Type, y: &str, yt: Type) -> Comparison {
    let ws = |ty| match ty {
        Type::String => WhiteSpace::Preserve,
        Type::NormalizedString => WhiteSpace::Replace,
        _ => WhiteSpace::Collapse,
    };
    let (xb, yb) = (x.as_bytes(), y.as_bytes());
    match (ws(xt), ws(yt)) {
        (WhiteSpace::Preserve, WhiteSpace::Preserve) => match x == y {
            true => Comparison::Equal,
            false => Comparison::Indeterminate,
        },
        (WhiteSpace::Preserve, WhiteSpace::Collapse) => collapse_order(xb, yb, false, false),
        (WhiteSpace::Collapse, WhiteSpace::Preserve) => collapse_order(yb, xb, false, true),
        (WhiteSpace::Replace, WhiteSpace::Collapse) => collapse_order(xb, yb, true, false),
        (WhiteSpace::Collapse, WhiteSpace::Replace) => collapse_order(yb, xb, true, true),
        (WhiteSpace::Collapse, WhiteSpace::Collapse) => {
            let (x, y) = (
                normalize(x, WhiteSpace::Collapse),
                normalize(y, WhiteSpace::Collapse),
            );
            // xmlSchemaCompareNormStrings returns the raw difference of a
            // blank and a non-blank byte: `' ' - '"'` is its error code -2.
            let first = x.bytes().zip(y.bytes()).find(|(a, b)| a != b);
            match first {
                Some((b' ', b'"')) => Comparison::Incomparable,
                _ => x.cmp(&y).into(),
            }
        }
        _ => xb.cmp(yb).into(),
    }
}

impl Value {
    pub fn ty(&self) -> Type {
        self.ty
    }

    /// `xmlSchemaCompareValues`. Numbers of the decimal family, floats with
    /// doubles, all date/time types, and the string family each compare
    /// among themselves; `NaN` equals itself and exceeds every number.
    /// Unlike libxml2, string order is plain code point order.
    pub fn compare(&self, other: &Value) -> Comparison {
        let float = |data: &Data| match *data {
            Data::Float(f) => Some(f64::from(f)),
            Data::Double(d) => Some(d),
            _ => None,
        };
        match (&self.data, &other.data) {
            (Data::Decimal(x), Data::Decimal(y)) => x.cmp(y).into(),
            (Data::Duration(x), Data::Duration(y)) => temporal::compare_durations(x, y),
            (Data::Date(x), Data::Date(y)) => temporal::compare_dates(x, self.ty, y, other.ty),
            (Data::Boolean(x), Data::Boolean(y)) => x.cmp(y).into(),
            (Data::Binary { octets, text }, Data::Binary { octets: o, text: t })
                if self.ty == other.ty =>
            {
                (octets, text).cmp(&(o, t)).into()
            }
            (
                Data::QName { namespace, local },
                Data::QName {
                    namespace: n,
                    local: l,
                },
            ) => match (namespace, local) == (n, l) {
                true => Comparison::Equal,
                false => Comparison::Indeterminate,
            },
            (Data::Text(x), Data::Text(y))
                if self.ty.item().is_none() && other.ty.item().is_none() =>
            {
                compare_text(x, self.ty, y, other.ty)
            }
            (x, y) => match (float(x), float(y)) {
                (Some(x), Some(y)) => match (x.is_nan(), y.is_nan()) {
                    (true, true) => Comparison::Equal,
                    (true, false) => Comparison::Greater,
                    (false, true) => Comparison::Less,
                    _ => x
                        .partial_cmp(&y)
                        .map_or(Comparison::Indeterminate, Into::into),
                },
                _ => Comparison::Incomparable,
            },
        }
    }

    /// Value equality as libxml2's `xmlSchemaAreValuesEqual` decides it for
    /// enumerations and identity constraints: the same primitive type, and
    /// equal text for strings or an equal comparison otherwise.
    pub fn equals(&self, other: &Value) -> bool {
        self.ty.primitive() == other.ty.primitive()
            && match (&self.data, &other.data) {
                (Data::Text(x), Data::Text(y)) => x == y,
                _ => self.compare(other) == Comparison::Equal,
            }
    }

    /// The length facets' measure: characters, list items or octets.
    /// `None` for QName and NOTATION, whose length always passes; zero for
    /// types without a length, as in libxml2.
    fn length(&self) -> Option<u64> {
        Some(match &self.data {
            Data::Binary { octets, .. } => *octets,
            Data::QName { .. } => return None,
            Data::Text(text) if self.ty.item().is_some() => {
                text.split(' ').filter(|t| !t.is_empty()).count() as u64
            }
            Data::Text(text) => text.chars().count() as u64,
            _ => 0,
        })
    }
}

/// A constraining facet. Callers combine the facets of a type: values must
/// pass every facet except that enumerations, and patterns of one
/// derivation step, need only one match.
#[derive(Clone, Debug)]
pub enum Facet {
    Length(u64),
    MinLength(u64),
    MaxLength(u64),
    Pattern(Regex),
    Enumeration(Value),
    WhiteSpace(WhiteSpace),
    MinInclusive(Value),
    MaxInclusive(Value),
    MinExclusive(Value),
    MaxExclusive(Value),
    TotalDigits(u64),
    FractionDigits(u64),
}

/// Why a facet could not be built.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum FacetError {
    /// Not an XML Schema facet name.
    Unknown,
    /// The facet's value is not valid for it or for the base type.
    Value,
    Pattern(regexp::Error),
}

impl fmt::Display for FacetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown => f.write_str("unknown facet"),
            Self::Value => f.write_str("invalid facet value"),
            Self::Pattern(error) => write!(f, "invalid pattern facet: {error}"),
        }
    }
}

impl std::error::Error for FacetError {}

impl From<InvalidValue> for FacetError {
    fn from(_: InvalidValue) -> Self {
        Self::Value
    }
}

impl Facet {
    /// Builds facet `name` (an XML Schema element name such as
    /// `"maxInclusive"`) from its lexical `value` for a type derived from
    /// `base`, like `xmlSchemaCheckFacet`.
    pub fn new(
        name: &str,
        value: &str,
        base: Type,
        context: Option<&dyn Context>,
    ) -> Result<Facet, FacetError> {
        let count = |ty: Type| match ty.parse(value, None)?.data {
            Data::Decimal(decimal) => Ok::<_, FacetError>(decimal.to_u64()),
            _ => Err(FacetError::Value),
        };
        let bound = || base.parse(value, context);
        Ok(match name {
            "length" => Facet::Length(count(Type::NonNegativeInteger)?),
            "minLength" => Facet::MinLength(count(Type::NonNegativeInteger)?),
            "maxLength" => Facet::MaxLength(count(Type::NonNegativeInteger)?),
            "pattern" => Facet::Pattern(Regex::new(value).map_err(FacetError::Pattern)?),
            "enumeration" => Facet::Enumeration(bound()?),
            "whiteSpace" => Facet::WhiteSpace(match value {
                "preserve" => WhiteSpace::Preserve,
                "replace" => WhiteSpace::Replace,
                "collapse" => WhiteSpace::Collapse,
                _ => return Err(FacetError::Value),
            }),
            "minInclusive" => Facet::MinInclusive(bound()?),
            "maxInclusive" => Facet::MaxInclusive(bound()?),
            "minExclusive" => Facet::MinExclusive(bound()?),
            "maxExclusive" => Facet::MaxExclusive(bound()?),
            "totalDigits" => Facet::TotalDigits(count(Type::PositiveInteger)?),
            "fractionDigits" => Facet::FractionDigits(count(Type::NonNegativeInteger)?),
            _ => return Err(FacetError::Unknown),
        })
    }

    /// Whether `value` satisfies the facet; `normalized` is its lexical form
    /// after whitespace normalization, which patterns match.
    pub fn check(&self, value: &Value, normalized: &str) -> bool {
        let digits = |measure: fn(&Decimal) -> u64, limit: u64| match &value.data {
            Data::Decimal(decimal) => measure(decimal) <= limit,
            _ => false,
        };
        let order = |bound: &Value| value.compare(bound);
        match self {
            Facet::Length(n) => value.length().is_none_or(|len| len == *n),
            Facet::MinLength(n) => value.length().is_none_or(|len| len >= *n),
            Facet::MaxLength(n) => value.length().is_none_or(|len| len <= *n),
            Facet::Pattern(regex) => regex.is_match(normalized),
            Facet::Enumeration(allowed) => allowed.equals(value),
            Facet::WhiteSpace(_) => true,
            Facet::MinInclusive(bound) => {
                matches!(order(bound), Comparison::Greater | Comparison::Equal)
            }
            Facet::MaxInclusive(bound) => {
                matches!(order(bound), Comparison::Less | Comparison::Equal)
            }
            Facet::MinExclusive(bound) => order(bound) == Comparison::Greater,
            Facet::MaxExclusive(bound) => order(bound) == Comparison::Less,
            Facet::TotalDigits(n) => digits(Decimal::total_digits, *n),
            Facet::FractionDigits(n) => digits(Decimal::fraction_digits, *n),
        }
    }
}

#[cfg(test)]
#[path = "../tests/internal/schema_types.rs"]
mod tests;
