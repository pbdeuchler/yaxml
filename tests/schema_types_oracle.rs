//! Differential tests against `tooling/libxml2_schema_types.c` (libxml2
//! 2.15.4). Runs only when `YAXML_SCHEMA_TYPES_HELPER` names the compiled
//! helper; see `docs/schema-types.md`.
#![forbid(unsafe_code)]

#[path = "../examples/support/process.rs"]
mod process;

use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::Path;
use std::time::Duration;

use process::ProcessLimits;
use yaxml::regexp::{Error, Regex};
use yaxml::schema_types::{Comparison, Context, Facet, Type, normalize};

fn lcg(state: &mut u64) -> usize {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    (*state >> 33) as usize
}

fn soup(state: &mut u64, tokens: &[&str], max: usize) -> String {
    (0..lcg(state) % (max + 1))
        .map(|_| tokens[lcg(state) % tokens.len()])
        .collect()
}

fn encode(s: &str) -> String {
    if s.is_empty() {
        return "0".into();
    }
    s.bytes().fold(String::new(), |mut out, b| {
        write!(out, "{b:02x}").unwrap();
        out
    })
}

/// Sends `requests` (operation and fields) and returns one line per request.
fn ask(requests: &[Vec<String>]) -> Option<Vec<String>> {
    let helper = std::env::var_os("YAXML_SCHEMA_TYPES_HELPER")?;
    let input: String = requests
        .iter()
        .map(|r| {
            let fields: Vec<String> = r[1..].iter().map(|f| encode(f)).collect();
            format!("{}\t{}\n", r[0], fields.join("\t"))
        })
        .collect();
    let limits = ProcessLimits {
        timeout: Duration::from_secs(600),
        max_output_bytes: 64 << 20,
    };
    let output =
        process::run(Path::new(&helper), &[], input.as_bytes(), limits).expect("helper runs");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let lines: Vec<String> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(lines.len(), requests.len(), "one response line per request");
    Some(lines)
}

/// Prints the tally and returns the unexplained differences.
fn report(name: &str, total: usize, classes: BTreeMap<&'static str, Vec<String>>) -> Vec<String> {
    let differing: usize = classes.values().map(Vec::len).sum();
    eprintln!(
        "{name}: {total} cases, {} agree, {differing} differ",
        total - differing
    );
    for (class, cases) in &classes {
        eprintln!(
            "  {class}: {} (e.g. {})",
            cases.len(),
            cases[..cases.len().min(3)].join("; ")
        );
    }
    classes.get("unexplained").cloned().unwrap_or_default()
}

const PATTERN_TOKENS: &[&str] = &[
    "a",
    "b",
    "c",
    "1",
    "é",
    "-",
    "^",
    ".",
    "\\d",
    "\\D",
    "\\s",
    "\\S",
    "\\w",
    "\\W",
    "\\i",
    "\\c",
    "\\p{L}",
    "\\p{Lu}",
    "\\P{Nd}",
    "\\p{IsBasicLatin}",
    "\\p{IsLatin-1Supplement}",
    "\\u0062",
    "[a-c]",
    "[^ab]",
    "[a-z-[aeiou]]",
    "[\\d-[1]]",
    "[\\P{L}]",
    "[",
    "]",
    "[^",
    "-[",
    "(",
    ")",
    "(",
    ")",
    "|",
    "|",
    "?",
    "*",
    "+",
    "{2}",
    "{1,3}",
    "{0,2}",
    "{2,}",
    "{0}",
    "{",
    "}",
    ",",
    "\\",
    "\\-",
    "\\.",
    "(a?)",
    "(a|ab)",
    "{1,2}",
    "(b*)",
];
const VALUE_TOKENS: &[&str] = &["a", "b", "c", "1", "2", " ", "-", "é", "A", "_", "^", "."];

#[test]
fn regexp_matches_libxml2() {
    let mut state = 0x5eed;
    let mut cases = Vec::new();
    for _ in 0..6000 {
        let pattern = soup(&mut state, PATTERN_TOKENS, 8);
        for _ in 0..8 {
            cases.push((pattern.clone(), soup(&mut state, VALUE_TOKENS, 6)));
        }
    }
    let requests: Vec<Vec<String>> = cases
        .iter()
        .map(|(p, v)| vec!["re".into(), p.clone(), v.clone()])
        .collect();
    let Some(lines) = ask(&requests) else {
        eprintln!("YAXML_SCHEMA_TYPES_HELPER unset; skipping the libxml2 differential");
        return;
    };
    let mut classes: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    for ((pattern, value), line) in cases.iter().zip(&lines) {
        let ours = Regex::new(pattern);
        let class = match (&ours, line.as_str()) {
            (Err(_), "ERR") => continue,
            (Ok(regex), verdict) if verdict == "1" || verdict == "0" => {
                match regex.is_match(value) == (verdict == "1") {
                    true => continue,
                    // libxml2's determinism check applies the negation of `\P`
                    // twice, so it drops backtracking points it needs.
                    false if pattern.contains("\\P") => "libxml2 \\P determinism",
                    false => "unexplained",
                }
            }
            (Ok(_), "ERR") => "unexplained",
            (Ok(_), _) => "libxml2 match error",
            (Err(Error::TooComplex), _) => "budget",
            (Err(_), _) => "unexplained",
        };
        classes.entry(class).or_default().push(format!(
            "{pattern:?} on {value:?}: libxml2 {line}, yaxml {:?}",
            ours.map(|r| r.is_match(value))
        ));
    }
    let unexplained = report("regexp", cases.len(), classes);
    assert!(
        unexplained.is_empty(),
        "{}",
        unexplained[..unexplained.len().min(40)].join("\n")
    );
}

const NUMBER: &[&str] = &[
    "0",
    "1",
    "9",
    "00",
    "-",
    "+",
    ".",
    "e",
    "E",
    "5",
    "INF",
    "NaN",
    " ",
    "\t",
    "0.50",
    "12345678901234567890",
    "2147483647",
    "2147483648",
    "-128",
    "255",
    "256",
    "65535",
    "4294967296",
    "9223372036854775807",
    "9223372036854775808",
    "18446744073709551616",
    "x",
];
const DATE: &[&str] = &[
    "2001",
    "-",
    "-",
    "--",
    "---",
    "01",
    "12",
    "13",
    "29",
    "30",
    "31",
    "00",
    "24",
    ":",
    ":",
    "T",
    "Z",
    "+",
    "14:00",
    "05:30",
    ".5",
    "0001",
    "10000",
    "-0001",
    "02",
    "59",
    "60",
    " ",
    "2000-02-29",
    "1999-12-31T23:59:59",
    "00:00:00",
    "24:00:00",
    "--12-25",
    "--02-29",
    "---31",
    "--02",
    "2001-10",
    "-0044-03-15",
    "2001-10-26T21:32:52.12",
    "12:30:00",
    "Z",
    "+05:30",
    "-14:00",
    "+14:00",
];
const DURATION: &[&str] = &[
    "P",
    "-P",
    "P1Y",
    "P2M",
    "P30D",
    "P1Y2M3DT4H5M6.5S",
    "PT1H",
    "PT61M",
    "PT3600S",
    "PT.5S",
    "-P1M",
    "1Y",
    "13M",
    "400D",
    "T",
    "25H",
    "1.5S",
    "0D",
    "Y",
    "M",
    "D",
    "H",
    "S",
    ".",
    "5",
    "99999999999999999999",
    " ",
];
const NAME: &[&str] = &[
    "a", "é", "1", "-", ".", ":", "_", " ", "\t", "xml", "p:", "Ab", "·", "\u{300}",
];
const BINARY: &[&str] = &[
    "0", "9", "A", "f", "g", "=", "+", "/", "Zm", "9v", "YQ", "==", " ", "Zg", "QQ",
];
const URI: &[&str] = &[
    "http://", "a", "%20", "#", "?", " ", "[", "]", "::", "é", "<", "%zz", ":", "/",
];
const WORD: &[&str] = &[
    "true",
    "false",
    "0",
    "1",
    " ",
    "TRUE",
    "en",
    "-",
    "US",
    "123",
    "abcdefghi",
    "x",
];

fn tokens(ty: Type) -> &'static [&'static str] {
    use Type::*;
    match ty.primitive() {
        Decimal | Float | Double => NUMBER,
        Duration => DURATION,
        DateTime | Time | Date | GYearMonth | GYear | GMonthDay | GDay | GMonth => DATE,
        HexBinary | Base64Binary => BINARY,
        AnyUri => URI,
        Boolean => WORD,
        _ if ty == Language || ty == String || ty == Token => WORD,
        _ => NAME,
    }
}

fn values(state: &mut u64, ty: Type, count: usize) -> Vec<String> {
    (0..count)
        .map(|i| match i % 8 {
            7 => soup(state, NAME, 4),
            _ => soup(state, tokens(ty), 6),
        })
        .collect()
}

/// Generated lexical forms that yaxml accepts; the validation differential
/// checks acceptance itself.
fn valid(state: &mut u64, ty: Type, count: usize) -> Vec<String> {
    let mut values = values(state, ty, count);
    values.retain(|v| ty.parse(v, None).is_ok());
    values
}

#[test]
fn datatypes_validate_like_libxml2() {
    let mut state = 0x7ea;
    let cases: Vec<(Type, String)> = Type::all()
        .flat_map(|ty| {
            values(&mut state, ty, 1500)
                .into_iter()
                .map(move |v| (ty, v))
        })
        .collect();
    let requests: Vec<Vec<String>> = cases
        .iter()
        .map(|(ty, v)| vec!["val".into(), ty.name().into(), v.clone()])
        .collect();
    let Some(lines) = ask(&requests) else {
        return;
    };
    let mut classes: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    for ((ty, value), line) in cases.iter().zip(&lines) {
        let ours = ty.parse(value, None).is_ok();
        if ours != (line == "0") {
            classes.entry("unexplained").or_default().push(format!(
                "{} {value:?}: libxml2 {line}, yaxml {ours}",
                ty.name()
            ));
        }
    }
    let valid = lines.iter().filter(|line| *line == "0").count();
    eprintln!("validate: {valid} of the cases are valid");
    let unexplained = report("validate", cases.len(), classes);
    assert!(
        unexplained.is_empty(),
        "{}",
        unexplained[..unexplained.len().min(40)].join("\n")
    );
}

/// Groups of mutually comparable types, plus a few incomparable pairs.
const GROUPS: &[&[Type]] = {
    use Type::*;
    &[
        &[
            Decimal,
            Integer,
            Long,
            Int,
            Short,
            Byte,
            NonNegativeInteger,
            UnsignedLong,
            UnsignedByte,
            PositiveInteger,
            NonPositiveInteger,
            NegativeInteger,
        ],
        &[Float, Double],
        &[Duration],
        &[
            DateTime, Date, Time, GYearMonth, GYear, GMonthDay, GDay, GMonth,
        ],
        &[DateTime],
        &[Time],
        &[Date],
        &[
            String,
            NormalizedString,
            Token,
            Name,
            NCName,
            NmToken,
            Language,
            AnyUri,
            AnySimpleType,
            Id,
            IdRef,
        ],
        &[String],
        &[HexBinary, Base64Binary],
        &[Boolean],
        &[QName],
        &[Decimal, Float, Boolean, String, Duration, Date, NmTokens],
    ]
};

fn comparison(line: &str) -> Option<Comparison> {
    Some(match line.parse::<i32>().ok()? {
        -2 => Comparison::Incomparable,
        0 => Comparison::Equal,
        2 => Comparison::Indeterminate,
        n if n < 0 => Comparison::Less,
        _ => Comparison::Greater,
    })
}

#[test]
fn datatypes_compare_like_libxml2() {
    let mut state = 0xc0de;
    let mut cases = Vec::new();
    for group in GROUPS {
        let pool: Vec<(Type, String)> = group
            .iter()
            .flat_map(|&ty| {
                valid(&mut state, ty, 4000)
                    .into_iter()
                    .map(move |v| (ty, v))
            })
            .collect();
        for _ in 0..pool.len().min(6000) {
            let x = pool[lcg(&mut state) % pool.len()].clone();
            let y = pool[lcg(&mut state) % pool.len()].clone();
            cases.push((x, y));
        }
    }
    let requests: Vec<Vec<String>> = cases
        .iter()
        .map(|((xt, x), (yt, y))| {
            vec![
                "cmp".into(),
                xt.name().into(),
                x.clone(),
                yt.name().into(),
                y.clone(),
            ]
        })
        .collect();
    let Some(lines) = ask(&requests) else {
        return;
    };
    let mut classes: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    for (((xt, x), (yt, y)), line) in cases.iter().zip(&lines) {
        let (xv, yv) = (xt.parse(x, None).unwrap(), yt.parse(y, None).unwrap());
        let ours = xv.compare(&yv);
        if comparison(line) == Some(ours) {
            continue;
        }
        classes.entry("unexplained").or_default().push(format!(
            "{} {x:?} vs {} {y:?}: libxml2 {line}, yaxml {ours:?}",
            xt.name(),
            yt.name()
        ));
    }
    let mut verdicts: BTreeMap<&str, usize> = BTreeMap::new();
    for line in &lines {
        *verdicts.entry(line.as_str()).or_default() += 1;
    }
    eprintln!("compare: libxml2 verdicts {verdicts:?}");
    let unexplained = report("compare", cases.len(), classes);
    assert!(
        unexplained.is_empty(),
        "{}",
        unexplained[..unexplained.len().min(40)].join("\n")
    );
}

/// Facets and the types they apply to.
const FACETS: &[(&str, &[Type])] = {
    use Type::*;
    let lengths: &[Type] = &[
        String,
        NormalizedString,
        Token,
        NmToken,
        Name,
        AnyUri,
        HexBinary,
        Base64Binary,
        NmTokens,
        IdRefs,
        QName,
    ];
    let bounds: &[Type] = &[
        Decimal,
        Integer,
        Int,
        UnsignedByte,
        Float,
        Double,
        Duration,
        DateTime,
        Date,
        Time,
        GYear,
        GMonthDay,
    ];
    &[
        ("length", lengths),
        ("minLength", lengths),
        ("maxLength", lengths),
        ("minInclusive", bounds),
        ("maxInclusive", bounds),
        ("minExclusive", bounds),
        ("maxExclusive", bounds),
        ("totalDigits", &[Decimal, Integer, Long, NonNegativeInteger]),
        ("fractionDigits", &[Decimal, Integer]),
        ("pattern", &[String, Token, Decimal, Date, HexBinary]),
        (
            "enumeration",
            &[
                String, Token, Decimal, Integer, Float, Date, DateTime, Duration, HexBinary,
                Boolean, QName, AnyUri,
            ],
        ),
        ("whiteSpace", &[String, Token]),
    ]
};

const FACET_VALUES: &[&str] = &[
    "0", "1", "2", "3", "5", "10", "-1", "1.5", "x", "", "a+", "\\d{1,3}", "[a-f]*", "preserve",
    "collapse",
];

#[test]
fn facets_check_like_libxml2() {
    let mut state = 0xfac;
    let mut cases = Vec::new();
    for (facet, types) in FACETS {
        for &ty in *types {
            let pool = valid(&mut state, ty, 20_000);
            assert!(
                pool.len() > 20,
                "{} has {} valid values",
                ty.name(),
                pool.len()
            );
            for i in 0..400 {
                let pick = |state: &mut u64| pool[lcg(state) % pool.len()].clone();
                let value = match i % 10 {
                    0 => values(&mut state, ty, 1).pop().unwrap(),
                    _ => pick(&mut state),
                };
                let facet_value = match lcg(&mut state) % 2 {
                    0 => FACET_VALUES[lcg(&mut state) % FACET_VALUES.len()].to_owned(),
                    _ => pick(&mut state),
                };
                cases.push((ty, *facet, facet_value, value));
            }
        }
    }
    let requests: Vec<Vec<String>> = cases
        .iter()
        .map(|(ty, f, fv, v)| {
            vec![
                "facet".into(),
                ty.name().into(),
                f.to_string(),
                fv.clone(),
                v.clone(),
            ]
        })
        .collect();
    let Some(lines) = ask(&requests) else {
        return;
    };
    let mut classes: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    for ((ty, name, facet_value, value), line) in cases.iter().zip(&lines) {
        let facet = Facet::new(name, facet_value, *ty, None);
        let parsed = ty.parse(value, None);
        let ours = match (&facet, &parsed) {
            (Ok(facet), Ok(parsed)) => {
                Some(facet.check(parsed, &normalize(value, ty.whitespace())))
            }
            _ => None,
        };
        let theirs = match line.as_str() {
            "ERR" => None,
            n => Some(n == "0"),
        };
        if ours == theirs {
            continue;
        }
        let class = match (line.as_str(), ours) {
            // A failed comparison is libxml2's internal error -1.
            ("-1", Some(false)) => continue,
            _ => "unexplained",
        };
        classes.entry(class).or_default().push(format!(
            "{} {name}={facet_value:?} on {value:?}: libxml2 {line}, yaxml {ours:?}",
            ty.name()
        ));
    }
    let mut verdicts: BTreeMap<&str, usize> = BTreeMap::new();
    for line in &lines {
        *verdicts.entry(line.as_str()).or_default() += 1;
    }
    eprintln!("facets: libxml2 verdicts {verdicts:?}");
    let unexplained = report("facets", cases.len(), classes);
    assert!(
        unexplained.is_empty(),
        "{}",
        unexplained[..unexplained.len().min(40)].join("\n")
    );
}

/// The context of the `<c>` element in the helper's fixed document.
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

const QUALIFIED: &[&str] = &[
    "p:", "q:", "xml:", "a", "gif", "n", "pic", "txt", " ", "1", ":", "é",
];

#[test]
fn context_types_match_libxml2() {
    let mut state = 0xc7c;
    let types = [
        Type::QName,
        Type::Notation,
        Type::Entity,
        Type::Entities,
        Type::IdRef,
    ];
    let mut requests = Vec::new();
    for ty in types {
        for _ in 0..2000 {
            requests.push(vec![
                "valn".into(),
                ty.name().into(),
                soup(&mut state, QUALIFIED, 4),
            ]);
        }
    }
    let names = [Type::QName, Type::Notation];
    for _ in 0..4000 {
        let mut operand = || {
            let ty = names[lcg(&mut state) % 2];
            let names = [
                "p:a", "a", "gif", "p:n", "xml:a", "n", "p:gif", "pic", "q:a",
            ];
            (
                ty.name().to_owned(),
                names[lcg(&mut state) % names.len()].to_owned(),
            )
        };
        let ((xt, x), (yt, y)) = (operand(), operand());
        requests.push(vec!["cmpn".into(), xt, x, yt, y]);
    }
    let Some(lines) = ask(&requests) else {
        return;
    };
    let context: Option<&dyn Context> = Some(&Document);
    let mut classes: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    let (mut accepted, mut compared) = (0, 0);
    for (request, line) in requests.iter().zip(&lines) {
        let ty = |name: &str| Type::from_name(name).unwrap();
        let agrees = match request[0].as_str() {
            "valn" => {
                accepted += usize::from(line == "0");
                ty(&request[1]).parse(&request[2], context).is_ok() == (line == "0")
            }
            _ => {
                let x = ty(&request[1]).parse(&request[2], context);
                let y = ty(&request[3]).parse(&request[4], context);
                match (x, y) {
                    (Ok(x), Ok(y)) => {
                        compared += 1;
                        comparison(line) == Some(x.compare(&y))
                    }
                    _ => line == "ERR",
                }
            }
        };
        if !agrees {
            classes
                .entry("unexplained")
                .or_default()
                .push(format!("{request:?}: libxml2 {line}"));
        }
    }
    eprintln!("context: {accepted} values accepted, {compared} pairs compared");
    let unexplained = report("context", requests.len(), classes);
    assert!(
        unexplained.is_empty(),
        "{}",
        unexplained[..unexplained.len().min(40)].join("\n")
    );
}
