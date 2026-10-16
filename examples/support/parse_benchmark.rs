//! Strict protocol and deterministic corpus for separately built parse helpers.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

pub const MAGIC: &str = "YAXML-PARSE-BENCH-1";
pub const MAX_INPUT: usize = 8 * 1024 * 1024;
pub const MAX_WORK: usize = 512 * 1024 * 1024;
pub const MODES: [&str; 5] = ["dom", "owned", "reader", "push", "buffered"];
pub const FIXTURES: [&str; 9] = [
    "tiny",
    "flat",
    "text",
    "attributes",
    "namespaces",
    "normalized",
    "unicode",
    "deep",
    "mixed",
];

#[derive(Debug, Eq, PartialEq)]
pub struct Config {
    pub libxml2: PathBuf,
    pub yaxml: PathBuf,
    pub input: Option<PathBuf>,
    pub fixture: Option<String>,
    pub mode: String,
    pub iterations: usize,
    pub chunk: usize,
    pub samples: usize,
    pub timeout_ms: u64,
}

fn decimal(text: &str) -> Result<u64, &'static str> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("expected unsigned decimal integer");
    }
    text.parse().map_err(|_| "decimal integer overflow")
}

pub fn config(arguments: impl IntoIterator<Item = OsString>) -> Result<Config, &'static str> {
    let mut arguments = arguments.into_iter();
    let mut values = BTreeMap::new();
    while let Some(key) = arguments.next() {
        let key = key.into_string().map_err(|_| "option is not UTF-8")?;
        if ![
            "--libxml2-helper",
            "--yaxml-helper",
            "--input",
            "--fixture",
            "--mode",
            "--iterations",
            "--chunk-size",
            "--samples",
            "--timeout-ms",
        ]
        .contains(&key.as_str())
        {
            return Err("unknown option");
        }
        let value = arguments.next().ok_or("option requires a value")?;
        if values.insert(key, value).is_some() {
            return Err("duplicate option");
        }
    }
    let mut text = |key: &str, default: Option<&str>| -> Result<String, &'static str> {
        values.remove(key).map_or_else(
            || default.map(str::to_owned).ok_or("required option missing"),
            |value| value.into_string().map_err(|_| "value is not UTF-8"),
        )
    };
    let mode = text("--mode", None)?;
    if !MODES.contains(&mode.as_str()) {
        return Err("unknown mode");
    }
    let iterations = decimal(&text("--iterations", Some("10"))?)?;
    let chunk = decimal(&text("--chunk-size", Some("8192"))?)?;
    let samples = decimal(&text("--samples", Some("7"))?)?;
    let timeout_ms = decimal(&text("--timeout-ms", Some("60000"))?)?;
    if !(1..=10000).contains(&iterations)
        || !(1..=65536).contains(&chunk)
        || !(1..=31).contains(&samples)
        || !(1..=60000).contains(&timeout_ms)
    {
        return Err("numeric option outside bounds");
    }
    let fixture = values
        .remove("--fixture")
        .map(|value| value.into_string().map_err(|_| "fixture is not UTF-8"))
        .transpose()?;
    if fixture
        .as_deref()
        .is_some_and(|name| !FIXTURES.contains(&name))
    {
        return Err("unknown fixture");
    }
    let input = values.remove("--input").map(PathBuf::from);
    if input.is_some() == fixture.is_some() {
        return Err("select exactly one input or fixture");
    }
    let libxml2 = values
        .remove("--libxml2-helper")
        .ok_or("missing libxml2 helper")?;
    let yaxml = values
        .remove("--yaxml-helper")
        .ok_or("missing yaxml helper")?;
    if libxml2.is_empty() || yaxml.is_empty() {
        return Err("helper path is empty");
    }
    Ok(Config {
        libxml2: libxml2.into(),
        yaxml: yaxml.into(),
        input,
        fixture,
        mode,
        iterations: iterations as usize,
        chunk: chunk as usize,
        samples: samples as usize,
        timeout_ms,
    })
}

pub fn fixture(name: &str) -> Result<Vec<u8>, &'static str> {
    let body = match name {
        "tiny" => "<root a='v'>text</root>".into(),
        "flat" => format!(
            "<root>{}</root>",
            "<item a='value'>text</item>".repeat(4096)
        ),
        "text" => format!("<root>{}</root>", "text ".repeat(100_000)),
        "attributes" => {
            let attributes = (0..128)
                .map(|n| format!(" a{n}='value{n}'"))
                .collect::<String>();
            format!(
                "<root>{}</root>",
                format!("<item{attributes}/>").repeat(128)
            )
        }
        "namespaces" => format!(
            "<r xmlns:p='urn:p'>{}</r>",
            "<p:item xmlns:q='urn:q' p:a='v'><q:leaf/></p:item>".repeat(4096)
        ),
        "normalized" => format!(
            "<root>{}</root>",
            "<item a='a\r\nb&#x9;c'>a&amp;b\r\nc&#13;d</item>".repeat(4096)
        ),
        "unicode" => format!(
            "<根 xmlns:前='urn:example'>{}</根>",
            "<前:項 前:属='é😀'>かなé😀</前:項>".repeat(4096)
        ),
        "deep" => format!("{}text{}", "<n>".repeat(128), "</n>".repeat(128)),
        "mixed" => format!(
            "<?xml version='1.0'?><!--before--><r>{}</r><?after ok?>",
            "a<![CDATA[b]]>&amp;c<!-- note --><?go data?><e/>d".repeat(2048)
        ),
        _ => return Err("unknown fixture"),
    };
    Ok(body.into_bytes())
}

pub fn input_hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(14695981039346656037, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(1099511628211)
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Row {
    pub text: String,
    pub engine: String,
    pub mode: String,
    pub bytes: u64,
    pub iterations: u64,
    pub chunk: u64,
    pub times: [u64; 3],
    pub observations: [u64; 6],
    pub hash: u64,
}

impl Row {
    pub fn parse(bytes: &[u8]) -> Result<Self, &'static str> {
        let text = std::str::from_utf8(bytes).map_err(|_| "row is not UTF-8")?;
        let text = text.strip_suffix('\n').ok_or("row lacks final newline")?;
        let fields: Vec<_> = text.split('\t').collect();
        const KEYS: [&str; 15] = [
            "engine",
            "mode",
            "bytes",
            "iterations",
            "chunk",
            "parse_ns",
            "observe_ns",
            "drop_ns",
            "elements",
            "attributes",
            "namespaces",
            "text_bytes",
            "comments",
            "pis",
            "hash",
        ];
        if fields.len() != 16 || fields[0] != MAGIC {
            return Err("wrong row schema or field count");
        }
        let mut values = Vec::with_capacity(KEYS.len());
        for (field, key) in fields[1..].iter().zip(KEYS) {
            let (name, value) = field.split_once('=').ok_or("field has no equals sign")?;
            if name != key || value.is_empty() || value.bytes().any(|byte| byte.is_ascii_control())
            {
                return Err("wrong field name, order or value");
            }
            values.push(value);
        }
        if !["yaxml", "libxml2"].contains(&values[0]) || !MODES.contains(&values[1]) {
            return Err("unknown engine or mode");
        }
        let mut numbers = [0; 12];
        for (number, value) in numbers.iter_mut().zip(&values[2..14]) {
            *number = decimal(value)?;
        }
        if values[14].len() != 16
            || !values[14]
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err("invalid hash");
        }
        Ok(Self {
            text: text.into(),
            engine: values[0].into(),
            mode: values[1].into(),
            bytes: numbers[0],
            iterations: numbers[1],
            chunk: numbers[2],
            times: [numbers[3], numbers[4], numbers[5]],
            observations: numbers[6..12].try_into().expect("six observation fields"),
            hash: u64::from_str_radix(values[14], 16).map_err(|_| "invalid hash")?,
        })
    }

    pub fn validate(
        &self,
        config: &Config,
        engine: &str,
        bytes: usize,
    ) -> Result<(), &'static str> {
        let mode = if engine == "libxml2" && config.mode == "owned" {
            "dom"
        } else {
            &config.mode
        };
        if self.engine != engine
            || self.mode != mode
            || self.bytes != bytes as u64
            || self.iterations != config.iterations as u64
            || self.chunk != config.chunk as u64
        {
            return Err("helper row does not match requested work");
        }
        if self.observations[0] == 0
            || self.times[0] == 0
            || (mode != "dom" && mode != "owned" && self.times[1] != 0)
        {
            return Err("helper row has impossible work or timing");
        }
        self.times[0]
            .checked_add(self.times[2])
            .ok_or("total parse/drop time overflow")?;
        Ok(())
    }

    pub fn same_observation(&self, other: &Self) -> Result<(), &'static str> {
        if self.observations != other.observations || self.hash != other.hash {
            return Err("semantic observation differs");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> Vec<OsString> {
        [
            "--libxml2-helper",
            "c",
            "--yaxml-helper",
            "r",
            "--fixture",
            "tiny",
            "--mode",
            "dom",
        ]
        .map(OsString::from)
        .to_vec()
    }

    fn row() -> String {
        format!(
            "{MAGIC}\tengine=yaxml\tmode=dom\tbytes=23\titerations=10\tchunk=8192\tparse_ns=10\tobserve_ns=20\tdrop_ns=30\telements=1\tattributes=1\tnamespaces=0\ttext_bytes=4\tcomments=0\tpis=0\thash=0123456789abcdef\n"
        )
    }

    #[test]
    fn options_are_complete_unique_and_bounded() {
        let parsed = config(args()).unwrap();
        assert_eq!(
            (parsed.mode.as_str(), parsed.samples, parsed.chunk),
            ("dom", 7, 8192)
        );
        for extra in [
            vec!["--samples", "0"],
            vec!["--samples", "32"],
            vec!["--iterations", "10001"],
            vec!["--chunk-size", "65537"],
            vec!["--chunk-size", "+1"],
            vec!["--samples", "1.5"],
            vec!["--timeout-ms", "0"],
            vec!["--timeout-ms", "60001"],
            vec!["--samples", "18446744073709551616"],
            vec!["--input", "x"],
            vec!["--mode", "reader"],
            vec!["--bad", "1"],
            vec!["--samples"],
        ] {
            let mut bad = args();
            bad.extend(extra.iter().map(OsString::from));
            assert!(config(bad).is_err(), "{extra:?}");
        }
        for index in [0, 2, 4, 6] {
            let mut missing = args();
            missing.drain(index..index + 2);
            assert!(config(missing).is_err());
        }
        let mut input = args();
        input[4] = "--input".into();
        assert!(config(input).unwrap().input.is_some());
        let mut unknown = args();
        unknown[5] = "missing".into();
        assert!(config(unknown).is_err());
    }

    #[test]
    fn strict_rows_reject_each_corrupted_field_and_extra_output() {
        let text = row();
        let parsed = Row::parse(text.as_bytes()).unwrap();
        assert_eq!(parsed.observations, [1, 1, 0, 4, 0, 0]);
        let fields: Vec<_> = text.trim_end().split('\t').collect();
        for index in 0..fields.len() {
            let mut bad = fields.clone();
            bad[index] = "wrong";
            assert!(Row::parse(format!("{}\n", bad.join("\t")).as_bytes()).is_err());
        }
        for bad in [
            text.replace("yaxml", "other"),
            text.replace("mode=dom", "mode=x"),
            text.replace("parse_ns=10", "parse_ns=-1"),
            text.replace("0123456789abcdef", "0123456789ABCDEF"),
            text.replace("0123456789abcdef", "123"),
            format!("{text}extra\n"),
            text.replace('\n', "\r\n"),
            text.trim_end().to_owned(),
            text.replace("\tbytes=23", "\tbytes=23\tbytes=23"),
        ] {
            assert!(Row::parse(bad.as_bytes()).is_err(), "{bad:?}");
        }
        assert!(Row::parse(&[0xff]).is_err());
    }

    #[test]
    fn requested_work_and_semantic_identity_are_independent_gates() {
        let parsed = Row::parse(row().as_bytes()).unwrap();
        let config = config(args()).unwrap();
        parsed.validate(&config, "yaxml", 23).unwrap();
        parsed.same_observation(&parsed).unwrap();
        for index in 0..6 {
            let mut bad = parsed.clone();
            bad.observations[index] += 1;
            assert!(parsed.same_observation(&bad).is_err());
        }
        let mut bad = parsed.clone();
        bad.hash ^= 1;
        assert!(parsed.same_observation(&bad).is_err());
        for change in [
            "engine",
            "mode",
            "bytes",
            "iterations",
            "chunk",
            "empty",
            "clock",
            "overflow",
        ] {
            let mut bad = parsed.clone();
            match change {
                "engine" => bad.engine = "libxml2".into(),
                "mode" => bad.mode = "reader".into(),
                "bytes" => bad.bytes += 1,
                "iterations" => bad.iterations += 1,
                "chunk" => bad.chunk += 1,
                "empty" => bad.observations[0] = 0,
                "clock" => bad.times[0] = 0,
                "overflow" => bad.times[0] = u64::MAX,
                _ => unreachable!(),
            }
            assert!(bad.validate(&config, "yaxml", 23).is_err(), "{change}");
        }
        let mut owned = config;
        owned.mode = "owned".into();
        let mut c = parsed.clone();
        c.engine = "libxml2".into();
        c.validate(&owned, "libxml2", 23).unwrap();
        owned.mode = "reader".into();
        c.mode = "reader".into();
        assert!(c.validate(&owned, "libxml2", 23).is_err());
        c.times[1] = 0;
        c.validate(&owned, "libxml2", 23).unwrap();
    }

    #[test]
    fn versioned_fixtures_are_valid_bounded_and_distinct() {
        let mut identities = std::collections::BTreeSet::new();
        for name in FIXTURES {
            let bytes = fixture(name).unwrap();
            assert!(bytes.len() <= MAX_INPUT);
            yaxml::Document::parse_bytes(&bytes).unwrap();
            assert!(
                identities.insert(input_hash(&bytes)),
                "duplicate fixture {name}"
            );
        }
        assert!(fixture("unknown").is_err());
        assert_eq!(input_hash(b""), 14695981039346656037);
        assert_eq!(input_hash(b"hello"), 0xa430d84680aabd0b);
        const { assert!(MAX_WORK > MAX_INPUT) };
    }
}
