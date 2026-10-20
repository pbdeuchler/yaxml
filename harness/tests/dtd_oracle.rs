//! Differential tests of `yaxml::chars`, the `valid::Dtd` declaration views
//! and `save::encode_*` against `tooling/libxml2_dtd.c` (libxml2 2.15.4).
//! Runs only when `YAXML_DTD_HELPER` names the compiled helper;
//! `YAXML_XMLCONF` and `YAXML_LIBXML2_SRC` override the corpus locations.
//! See docs/compatibility.md for the build line and the recorded results.
#![forbid(unsafe_code)]

#[path = "../examples/support/process.rs"]
mod process;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use process::ProcessLimits;
use yaxml::valid::{AttributeDefault, Dtd, EntityKind};
use yaxml::{FileResolver, OwnedDocument, ParseOptions, chars, options, save};

const WORKSPACE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/..");

/// A character class predicate over one argument type.
type Class<T> = fn(T) -> bool;

fn helper(args: &[&str], input: &[u8]) -> Option<String> {
    let helper = std::env::var_os("YAXML_DTD_HELPER")?;
    let limits = ProcessLimits {
        timeout: Duration::from_secs(600),
        max_output_bytes: 256 << 20,
    };
    let output = process::run(Path::new(&helper), args, input, limits).expect("helper runs");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The helper's field encoding: tab-prefixed, `-` for none.
fn field(out: &mut String, value: Option<&str>) {
    out.push('\t');
    let Some(value) = value else {
        return out.push('-');
    };
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
}

fn runs(out: &mut String, name: &str, is: impl Fn(u32) -> bool, last: u32) {
    let mut low = None;
    for c in 0..=last + 1 {
        match (c <= last && is(c), low) {
            (true, None) => low = Some(c),
            (false, Some(start)) => {
                writeln!(out, "{name} {start:x} {:x}", c - 1).unwrap();
                low = None;
            }
            _ => {}
        }
    }
}

#[test]
fn character_classes_match_libxml2_over_every_code_point() {
    let Some(expected) = helper(&["chars"], b"") else {
        return;
    };
    let mut ours = String::new();
    let classes: [(&str, Class<u32>); 8] = [
        ("Char", chars::is_char),
        ("Blank", chars::is_blank),
        ("BaseChar", chars::is_base_char),
        ("Digit", chars::is_digit),
        ("Combining", chars::is_combining),
        ("Extender", chars::is_extender),
        ("Ideographic", chars::is_ideographic),
        ("PubidChar", chars::is_pubid_char),
    ];
    for (name, is) in classes {
        runs(&mut ours, name, is, 0x10ffff);
    }
    let bytes: [(&str, Class<u8>); 6] = [
        ("Char_ch", chars::is_char),
        ("Blank_ch", chars::is_blank),
        ("BaseChar_ch", chars::is_base_char),
        ("Digit_ch", chars::is_digit),
        ("Extender_ch", chars::is_extender),
        ("PubidChar_ch", chars::is_pubid_char),
    ];
    for (name, is) in bytes {
        runs(&mut ours, name, |c| is(c as u8), 0xff);
    }
    assert_eq!(ours, expected);
    eprintln!("chars: 8 classes x 1114112 code points and 6 byte macros agree");
}

#[test]
fn encoders_match_libxml2() {
    let mut texts: Vec<String> = (1..0x80u8).map(|b| char::from(b).to_string()).collect();
    texts.extend(
        [
            "",
            "a&b<c>d\"e'f\r\n\tg",
            "caf\u{e9} \u{fffe}\u{ffff}\u{fffd}",
            "\u{10000}\u{10ffff}\u{a0}\u{7ff}\u{800}",
        ]
        .map(String::from),
    );
    let mut state = 7u64;
    let alphabet = [
        "a",
        "&",
        "<",
        ">",
        "\"",
        "'",
        "\r",
        "\n",
        "\t",
        "\u{1}",
        "\u{1f}",
        "\u{7f}",
        "\u{e9}",
        "\u{20ac}",
        "\u{1f600}",
        "&amp;",
    ];
    for _ in 0..2000 {
        let mut text = String::new();
        for _ in 0..(state % 12) {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            text.push_str(alphabet[(state >> 33) as usize % alphabet.len()]);
        }
        texts.push(text);
    }
    let input: String = texts
        .iter()
        .map(|text| match text.is_empty() {
            true => "0\n".to_owned(),
            false => text.bytes().map(|b| format!("{b:02x}")).collect::<String>() + "\n",
        })
        .collect();
    let Some(expected) = helper(&["encode"], input.as_bytes()) else {
        return;
    };
    let mut ours = String::new();
    for text in &texts {
        ours.push_str("enc");
        field(
            &mut ours,
            Some(&save::encode_entities(text, save::Escape::Xml)),
        );
        field(
            &mut ours,
            Some(&save::encode_entities(text, save::Escape::Ascii)),
        );
        field(
            &mut ours,
            Some(&save::encode_entities(text, save::Escape::Html)),
        );
        field(&mut ours, Some(&save::encode_special_chars(text)));
        ours.push('\n');
    }
    let differing: Vec<_> = (ours.lines().zip(expected.lines()).zip(&texts))
        .filter(|((a, b), _)| a != b)
        .map(|((a, b), text)| format!("{text:?}\n  ours {a}\n  libxml2 {b}"))
        .collect();
    assert_eq!(ours.lines().count(), expected.lines().count());
    assert!(differing.is_empty(), "{}", differing.join("\n"));
    eprintln!("encode: {} strings x 4 modes agree", texts.len());
}

/// libxml2's valid documents and the W3C suite's valid cases, relative to
/// their corpus root.
fn corpus() -> Vec<(PathBuf, Vec<String>)> {
    let root = |variable: &str, default: &str| {
        std::env::var_os(variable).map_or_else(|| Path::new(WORKSPACE).join(default), PathBuf::from)
    };
    let xmlconf = root("YAXML_XMLCONF", "yaxml-suites/xmlconf");
    let libxml2 = root("YAXML_LIBXML2_SRC", "yaxml-oracle/libxml2-2.15.4").join("test/valid");
    let mut suites = Vec::new();
    for (root, valid_only) in [(libxml2, false), (xmlconf, true)] {
        let (mut files, mut pending) = (Vec::new(), vec![root.clone()]);
        while let Some(directory) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&directory) else {
                continue;
            };
            for path in entries.map(|entry| entry.unwrap().path()) {
                let relative = path
                    .strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                if path.is_dir() {
                    pending.push(path);
                } else if (relative.ends_with(".xml") || relative.ends_with(".xhtml"))
                    && !relative.contains("/out/")
                    && (!valid_only || relative.contains("valid/"))
                    && !relative.contains("invalid")
                {
                    files.push(relative);
                }
            }
        }
        files.sort();
        suites.push((root, files));
    }
    suites
}

/// One file's lines in the helper's format, with the subset column of
/// elements, attributes and notations dropped (yaxml keeps one table).
fn dump(dtd: &Dtd) -> BTreeSet<String> {
    let mut lines = BTreeSet::new();
    let mut names = BTreeSet::new();
    for entity in dtd.entities() {
        let mut line = format!(
            "entity\t{}\t{}",
            if entity.external_subset { "ext" } else { "int" },
            entity.kind as u8
        );
        for value in [
            Some(entity.name),
            entity.public_id,
            entity.system_id,
            entity.uri,
            entity.content,
        ] {
            field(&mut line, value);
        }
        lines.insert(line);
        names.insert(entity.name);
    }
    for name in names
        .iter()
        .copied()
        .chain(["lt", "gt", "amp", "apos", "quot", "none"])
    {
        let kind = |entity: Option<yaxml::valid::Entity<'_>>| {
            entity.map_or("-".into(), |e| (e.kind as u8).to_string())
        };
        lines.insert(format!("doc\t{name}\t{}", kind(dtd.entity(name))));
        if names.contains(name) {
            lines.insert(format!("pe\t{name}\t{}", kind(dtd.parameter_entity(name))));
            let external = dtd.entities().find(|e| {
                e.name == name
                    && e.external_subset
                    && !matches!(
                        e.kind,
                        EntityKind::InternalParameter | EntityKind::ExternalParameter
                    )
            });
            lines.insert(format!("dtd\t{name}\t{}", kind(external)));
        }
    }
    for element in dtd.elements() {
        let mut line = format!("element\t{}", element.kind as u8);
        field(&mut line, Some(element.name));
        field(&mut line, element.model);
        lines.insert(line);
        for attribute in element.attributes() {
            assert_eq!(dtd.attribute(element.name, attribute.name), Some(attribute));
            let (default, value) = match attribute.default {
                AttributeDefault::Value(value) => (1, Some(value)),
                AttributeDefault::Required => (2, None),
                AttributeDefault::Implied => (3, None),
                AttributeDefault::Fixed(value) => (4, Some(value)),
            };
            let mut line = "attribute".to_owned();
            field(&mut line, Some(attribute.element));
            field(&mut line, Some(attribute.name));
            write!(line, "\t{}\t{default}", attribute.kind as u8).unwrap();
            field(&mut line, value);
            let values = attribute.values.join("|");
            field(&mut line, (!values.is_empty()).then_some(&values));
            lines.insert(line);
        }
        assert_eq!(
            dtd.element(element.name).map(|e| e.name),
            Some(element.name)
        );
    }
    for notation in dtd.notations() {
        assert_eq!(dtd.notation(notation.name), Some(notation));
        let mut line = "notation".to_owned();
        for value in [Some(notation.name), notation.public_id, notation.system_id] {
            field(&mut line, value);
        }
        lines.insert(line);
    }
    lines
}

/// libxml2's two subsets merged as yaxml keeps them: the first declaration
/// of a name, an element's defined declaration over an attribute-list one.
/// Also returns the names of entities declared in both subsets.
fn merge(lines: &[&str]) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut kept: BTreeMap<String, (bool, String)> = BTreeMap::new();
    let (mut out, mut twice) = (BTreeSet::new(), BTreeSet::new());
    for line in lines {
        let fields: Vec<&str> = line.split('\t').collect();
        let without_subset = || [&fields[..1], &fields[2..]].concat().join("\t");
        let (key, defined, text) = match fields[0] {
            "entity" => {
                let parameter = matches!(fields[2], "4" | "5");
                (
                    format!("entity {parameter} {}", fields[3]),
                    true,
                    line.to_string(),
                )
            }
            "element" => (
                format!("element {}", fields[3]),
                fields[2] != "0",
                without_subset(),
            ),
            "attribute" => (
                format!("attribute {} {}", fields[2], fields[3]),
                true,
                without_subset(),
            ),
            "notation" => (format!("notation {}", fields[2]), true, without_subset()),
            _ => {
                out.insert(line.to_string());
                continue;
            }
        };
        let replace = match kept.get(&key) {
            None => true,
            Some(&(was_defined, _)) => {
                if fields[0] == "entity" {
                    twice.insert(fields[3].to_owned());
                }
                defined && !was_defined
            }
        };
        if replace {
            kept.insert(key, (defined, text));
        }
    }
    out.extend(kept.into_values().map(|(_, text)| text));
    (out, twice)
}

/// The difference classes this comparison accepts (docs/compatibility.md).
const EXTERNAL_TEXT: &str = "external entity text libxml2 does not store";
const CRLF: &str = "line ends libxml2 stores unnormalized in an entity value";
const BOTH_SUBSETS: &str = "xmlGetDtdEntity of a name both subsets declare";

/// Removes the accepted differences from `ours` and `expected`, counting
/// them in `classes`.
fn classify(
    ours: &mut BTreeSet<String>,
    expected: &mut BTreeSet<String>,
    twice: &BTreeSet<String>,
    classes: &mut BTreeMap<&'static str, usize>,
) {
    let key = |line: &str| {
        let fields = if line.starts_with("entity") { 4 } else { 2 };
        line.splitn(fields + 1, '\t')
            .take(fields)
            .collect::<Vec<_>>()
            .join("\t")
    };
    let theirs: BTreeMap<String, String> = expected
        .difference(ours)
        .map(|line| (key(line), line.clone()))
        .collect();
    for line in ours.difference(expected).cloned().collect::<Vec<_>>() {
        let Some(other) = theirs.get(&key(&line)) else {
            continue;
        };
        let fields: Vec<&str> = line.split('\t').collect();
        let class = match fields[0] {
            "entity" if matches!(fields[2], "2" | "5") && other.ends_with("\t-") => EXTERNAL_TEXT,
            "entity" if other.replace("\\r\\n", "\\n").replace("\\r", "\\n") == line => CRLF,
            "dtd" if fields[2] == "-" && twice.contains(fields[1]) => BOTH_SUBSETS,
            _ => continue,
        };
        *classes.entry(class).or_default() += 1;
        ours.remove(&line);
        expected.remove(other);
    }
}

#[test]
fn declarations_match_libxml2() {
    if std::env::var_os("YAXML_DTD_HELPER").is_none() {
        return;
    }
    let verbose = std::env::var_os("YAXML_DTD_VERBOSE").is_some();
    let mut failures = Vec::new();
    for (mode, flags) in [
        ("dtdload", options::DTDLOAD),
        ("dtdload+noent", options::DTDLOAD | options::NOENT),
    ] {
        let (mut compared, mut agree, mut skipped, mut declarations) = (0, 0, 0, 0);
        let mut classes = BTreeMap::new();
        for (root, files) in corpus() {
            let input: String = files.iter().map(|file| format!("{file}\n")).collect();
            let output = helper(
                &["dtd", root.to_str().unwrap(), &flags.to_string()],
                input.as_bytes(),
            )
            .unwrap();
            let mut blocks = output.split("end\n");
            for file in &files {
                let block: Vec<&str> = blocks.next().unwrap().lines().collect();
                assert_eq!(block[0], format!("file\t{file}"));
                let bytes = std::fs::read(root.join(file)).unwrap();
                let options = || {
                    ParseOptions::new(flags)
                        .with_resolver(file, Box::new(FileResolver::new(&root).unwrap()))
                };
                let (Ok(document), true) = (
                    OwnedDocument::parse_with(&bytes, options()),
                    block.len() > 1,
                ) else {
                    skipped += 1;
                    continue;
                };
                let Some(dtd) = document.dtd() else {
                    skipped += 1;
                    continue;
                };
                let (mut expected, twice) = merge(&block[1..]);
                let mut ours = dump(dtd);
                if let Ok(borrowed) = yaxml::Document::parse_with(&bytes, options()) {
                    assert_eq!(
                        borrowed.dtd().map(dump).as_ref(),
                        Some(&ours),
                        "{file}: both DOMs"
                    );
                }
                compared += 1;
                declarations += expected.len();
                classify(&mut ours, &mut expected, &twice, &mut classes);
                if ours == expected {
                    agree += 1;
                } else {
                    let missing: Vec<_> = expected.difference(&ours).collect();
                    let extra: Vec<_> = ours.difference(&expected).collect();
                    if verbose {
                        eprintln!(
                            "{mode} {file}\n  libxml2 only: {missing:#?}\n  yaxml only: {extra:#?}"
                        );
                    }
                    failures.push(format!("{mode} {file}"));
                }
            }
        }
        eprintln!(
            "{mode}: {compared} documents ({declarations} declaration and lookup lines), {agree} agree after {classes:?}; {skipped} without a DTD or not parsed by both"
        );
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
