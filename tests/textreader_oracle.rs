#![forbid(unsafe_code)]
//! Differential `TextReader` checks against libxml2 2.15.4's `xmlTextReader`.
//!
//! `cases_match_the_reader_helper` runs `tooling/libxml2_textreader.c` (built
//! as in `docs/textreader.md`) named by `YAXML_TEXTREADER_HELPER` on every case
//! and compares its full per-node dump. `rde_results_match` replays libxml2's
//! own `result/*.rde` reader expectations (`xmllint --stream --debug --noent`)
//! from `YAXML_LIBXML2_SRC`, and `noblanks_streams_match_xmllint` compares
//! `xmllint --stream --debug --noblanks` (`YAXML_XMLLINT`). Each skips when its
//! variable is unset.

use std::fmt::Write;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::Command;

use yaxml::options::{DTDATTR, LEGACY, NOBLANKS, NOENT};
use yaxml::{FileResolver, NodeType, ReadError, Request, Resolver, TextReader};

/// Inputs for the helper comparison; intentional differences are in docs/textreader.md.
const CASES: &[&str] = &[
    "<r/>",
    "<r></r>",
    "<?xml version='1.0'?>\n<!-- lead -->\n<r>text</r>\n<?tail data?>",
    "<r a='1' b=\"two\"><e/><e x='y'></e>t<![CDATA[<c>]]><!--c--><?pi d?><?q?></r>",
    "<r>\n  <a>  </a>\t<b xml:space='default'> </b>\r\n</r>",
    "<r xmlns='urn:d' xmlns:p='urn:p' a='1' p:b='2'><p:c p:x='3'><d/></p:c><e xmlns=''/></r>",
    "<p:r xmlns:p='urn:p'><p:i xmlns:p='urn:q' p:a='x'/><j xmlns:q='urn:q'><q:k/></j></p:r>",
    "<r xmlns='urn:a'><s><t xmlns:z='urn:z' z:a='1'/></s></r>",
    "<r xml:lang='en' xml:base='http://example.org/a/'><s xml:base='b/'><t xml:lang='fr' xml:base='c.xml'/></s><u xml:base='urn:x'/></r>",
    "<r xml:base='rel/'><s xml:base='../up/'>x</s></r>",
    "<r a='&lt;&amp;&gt;&quot;&#9;&#10;&#13;'>&lt;&amp;&gt;\"'&#13;é</r>",
    "<!DOCTYPE r [<!ENTITY e 'x<b>y</b>z'><!ENTITY w '  '>]><r>a&e;b&w;</r>",
    "<!DOCTYPE r [<!ATTLIST r d CDATA 'default' n NMTOKENS ' a  b '>]><r n=' c  d '/>",
    "<!DOCTYPE r SYSTEM 'missing.dtd'><r/>",
    "<!DOCTYPE r PUBLIC 'p' 's' [<!ELEMENT r ANY>]><r>&#x10000;</r>",
    "<r><a><b><c>deep</c></b></a><a/></r>",
    "<r xmlns:p='urn:p'><p:a p:x='1' x='2'/></r>",
    "<r>a<!--x-->b<![CDATA[]]>c</r>",
    // Failures: see `agree`.
    "<r><a></b></r>",
    "<r>&undefined;</r>",
    "<r><a>",
    "<r/><s/>",
    "<r a='1' a='2'/>",
    "",
];

fn quoted(out: &mut String, value: Option<&str>) {
    let Some(value) = value else {
        out.push_str(" ~");
        return;
    };
    out.push_str(" \"");
    for c in value.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn cursor<R: BufRead>(reader: &TextReader<R>, out: &mut String) {
    write!(out, "{} {}", reader.depth(), reader.node_type() as i32).unwrap();
    quoted(out, reader.name());
    quoted(out, reader.local_name());
    quoted(out, reader.prefix());
    quoted(out, reader.namespace_uri());
    let flags = (reader.is_empty_element(), reader.has_value());
    write!(out, " {} {}", u8::from(flags.0), u8::from(flags.1)).unwrap();
    quoted(out, reader.value());
    let attributes = (reader.attribute_count(), reader.has_attributes());
    write!(out, " {} {}", attributes.0, u8::from(attributes.1)).unwrap();
    let (standalone, default) = (reader.standalone() as i32, reader.is_default());
    write!(out, " {standalone} {}", u8::from(default)).unwrap();
}

/// The helper's dump format, line for line.
fn describe<R: BufRead>(mut reader: TextReader<R>) -> String {
    let mut out = String::new();
    loop {
        match reader.read() {
            Ok(true) => {}
            Ok(false) => {
                let (standalone, consumed) = (reader.standalone() as i32, reader.byte_consumed());
                let normalization = u8::from(reader.normalization());
                return out + &format!("END {standalone} {normalization} {consumed}\n");
            }
            Err(_) => return out + "ERROR\n",
        }
        cursor(&reader, &mut out);
        quoted(&mut out, reader.base_uri().as_deref());
        quoted(&mut out, reader.xml_lang());
        for prefix in [None, Some("p"), Some("xml")] {
            quoted(&mut out, reader.lookup_namespace(prefix));
        }
        out.push('\n');
        if matches!(
            reader.node_type(),
            NodeType::DocumentType | NodeType::EndElement
        ) {
            continue;
        }
        for index in 0..=reader.attribute_count() {
            write!(out, "  #{index}").unwrap();
            quoted(&mut out, reader.get_attribute_no(index));
            out.push('\n');
        }
        while reader.move_to_next_attribute() {
            out.push_str("  @");
            cursor(&reader, &mut out);
            let name = reader.name().unwrap().to_owned();
            let local = reader.local_name().unwrap().to_owned();
            let uri = reader.namespace_uri().map(str::to_owned);
            reader.move_to_element();
            quoted(&mut out, reader.get_attribute(&name));
            quoted(&mut out, reader.get_attribute_ns(&local, uri.as_deref()));
            out.push('\n');
            reader.move_to_attribute(&name);
            let mut walked = false;
            while reader.read_attribute_value() {
                out.push_str("  ~v");
                cursor(&reader, &mut out);
                out.push('\n');
                walked = true;
            }
            if walked {
                let next = u8::from(reader.move_to_next_attribute());
                writeln!(out, "  ~next {next}").unwrap();
                reader.move_to_attribute(&name);
            }
        }
        write!(out, "  content {}", u8::from(reader.move_to_element())).unwrap();
        let mut content = || -> Result<[Option<String>; 3], ReadError> {
            Ok([
                reader.read_string()?,
                reader.read_inner_xml()?,
                reader.read_outer_xml()?,
            ])
        };
        match content() {
            Ok(values) => values
                .iter()
                .for_each(|value| quoted(&mut out, value.as_deref())),
            Err(_) => return out + "\nERROR\n",
        }
        out.push('\n');
    }
}

/// Exact agreement, except that libxml2 parses ahead in chunks: on failure it
/// may deliver fewer nodes, so its node lines need only be a prefix of ours.
fn agree(oracle: &str, ours: &str) -> bool {
    let nodes = |dump: &str| -> Vec<String> {
        let lines = dump
            .lines()
            .filter(|line| !line.starts_with(' ') && *line != "ERROR");
        lines.map(str::to_owned).collect()
    };
    match oracle.strip_suffix("ERROR\n") {
        Some(_) => ours.ends_with("ERROR\n") && nodes(ours).starts_with(&nodes(oracle)[..]),
        None => oracle == ours,
    }
}

/// Inputs with the parser flags they run under: kept and substituted entity
/// references in attribute values (`ReadAttributeValue`), DTD defaults
/// (`IsDefault`), `standalone`, and `NOBLANKS`.
#[rustfmt::skip]
const OPTION_CASES: &[(u32, &str)] = &[
    (0, "<!DOCTYPE r [<!ENTITY e 'ex'><!ENTITY f '&e;&e;'>]><r a='1&e;2' b='&lt;&e;&amp;&#38;&f;' c='&e;' d='' xmlns:p='urn:p' xmlns='' p:z='&e;'/>"),
    (NOENT, "<!DOCTYPE r [<!ENTITY e 'ex'><!ENTITY f '&e;&e;'>]><r a='1&e;2' b='&lt;&e;&amp;&#38;&f;' c='&e;' d='' p:z='&e;' xmlns:p='urn:p'/>"),
    (0, "<!DOCTYPE r [<!ENTITY e ''>]><r a='&e;' b='x&e;' c='&#x20;&#9;a  b'/>"),
    (NOENT, "<!DOCTYPE r [<!ENTITY e ''>]><r a='&e;' b='x&e;'/>"),
    (0, "<!DOCTYPE r [<!ATTLIST r n NMTOKENS #IMPLIED>]><r n=' a  b '/>"),
    (0, "<r a='' b='v' c='&lt;&gt;&amp;'/>"),
    (0, "<r xmlns='' xmlns:p='urn:p' p:a='1'><s xmlns=''/></r>"),
    (DTDATTR, "<!DOCTYPE r [<!ATTLIST r a CDATA 'dflt' b CDATA 'x' n NMTOKENS ' a  b '>]><r b='given'/>"),
    (0, "<!DOCTYPE r [<!ATTLIST r a CDATA 'dflt' b CDATA 'x'>]><r b='given'/>"),
    (DTDATTR, "<!DOCTYPE r [<!ENTITY e 'ex'><!ATTLIST r a CDATA 'p&e;q'>]><r/>"),
    (DTDATTR | NOENT, "<!DOCTYPE r [<!ENTITY e 'ex'><!ATTLIST r a CDATA 'p&e;q'>]><r/>"),
    (0, "<!DOCTYPE r [<!ATTLIST r xmlns:q CDATA 'urn:q'>]><r/>"),
    (DTDATTR, "<!DOCTYPE r [<!ATTLIST r xmlns:q CDATA 'urn:q' xmlns CDATA 'urn:d'>]><r/>"),
    (0, "<r/>"),
    (0, "<?xml version='1.0'?><r/>"),
    (0, "<?xml version='1.0' encoding='UTF-8'?>\n<r><a/></r>\n"),
    (0, "<?xml version='1.0' standalone='yes'?><r/>"),
    (0, "<?xml version=\"1.0\" standalone=\"no\"?><r>x</r><!--tail-->\n"),
    (NOBLANKS, "<r>\n  <a>\n    <b/>\n  </a>\n  <c> </c>\n  <d>x <e/> y</d><f> <g/></f><h><i/> </h>\n</r>"),
    (NOBLANKS, "<r> </r>"),
    (NOBLANKS, "<r>  <a/>  </r>"),
    (NOBLANKS, "<r><a/> <b/> <c/></r>"),
    (NOBLANKS, "<r>x <a/> <b/> y</r>"),
    (NOBLANKS, "<r> <a/>x</r>"),
    (NOBLANKS, "<r>x<a/> </r>"),
    (NOBLANKS, "<r>\r\n <a/>\r\n</r>"),
    (NOBLANKS, "<r xml:space='preserve'> <a> </a><b xml:space='default'> <c/> </b></r>"),
    (NOBLANKS, "<r xml:space='preserve'><a xml:space='other'> <b/> </a></r>"),
    (NOBLANKS, "<r> <!--c--> <?p d?> <![CDATA[ ]]> </r>"),
    (NOBLANKS, "<r>x<!--c--> <a/></r>"),
    (NOBLANKS, "<r><a/>x<b/> <c/></r>"),
    (NOBLANKS, "<r xml:space='default'><a/>x<b/> <c/></r>"),
    (NOBLANKS, "<r><a/>x<b/><s><a/> <b/></s> <c/></r>"),
    (NOBLANKS | NOENT, "<!DOCTYPE r [<!ENTITY e 'x'>]><r><a/>&e;<a/> <b/></r>"),
    (NOBLANKS | NOENT, "<!DOCTYPE r [<!ELEMENT r (a|b)*><!ELEMENT a (#PCDATA)><!ELEMENT b ANY>]><r> <a> </a> <b> <a/> </b> </r>"),
    (NOBLANKS, "<!DOCTYPE r [<!ELEMENT r (a|b)*><!ELEMENT a (#PCDATA)><!ELEMENT b ANY>]><r> <a> </a> <b> <a/> </b> </r>"),
    (NOBLANKS, "<!DOCTYPE r [<!ELEMENT r (#PCDATA|a)*><!ELEMENT a EMPTY>]><r> <a/> <a/> </r>"),
    (NOBLANKS, "<!DOCTYPE r [<!ENTITY e 'x'>]><r><a/> &e;<a/>  &e;  </r>"),
    (NOBLANKS | NOENT, "<!DOCTYPE r [<!ENTITY e 'x'>]><r><a/> &e;<a/>  &e;  </r>"),
    (NOBLANKS | NOENT, "<!DOCTYPE r [<!ENTITY e ' '>]><r><a/>&e;<a/></r>"),
    (NOBLANKS, "<!DOCTYPE r [<!ENTITY e ' '>]><r><a/>&e;<a/></r>"),
    (0, "<r>\n  <a>\n    <b/>\n  </a>\n</r>"),
];

/// Runs the helper on every `(flags, input)` and compares full dumps.
fn compare_with_helper(name: &str, cases: &[(u32, &str)]) {
    let Some(helper) = std::env::var_os("YAXML_TEXTREADER_HELPER") else {
        eprintln!("skipping: YAXML_TEXTREADER_HELPER is unset");
        return;
    };
    let directory = std::env::temp_dir().join(format!("yaxml-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let mut mismatches = Vec::new();
    for (index, (flags, case)) in cases.iter().enumerate() {
        let path = directory.join(format!("case{index}.xml"));
        std::fs::write(&path, case).unwrap();
        let output = Command::new(&helper)
            .arg(&path)
            .arg(flags.to_string())
            .output()
            .unwrap();
        assert!(output.status.success(), "helper failed on {case:?}");
        let oracle = String::from_utf8(output.stdout).unwrap();
        let base = path.to_str().unwrap();
        let options = yaxml::ParseOptions {
            base: Some(base.to_owned()),
            ..yaxml::ParseOptions::new(*flags)
        };
        let ours = describe(TextReader::with_options(case.as_bytes(), options).unwrap());
        if !agree(&oracle, &ours) {
            let (mut theirs, mut mine) = (oracle.lines(), ours.lines());
            let line = std::iter::from_fn(|| match (theirs.next(), mine.next()) {
                (None, None) => None,
                (a, b) => Some((a, b)),
            });
            let first = line.into_iter().find(|(a, b)| a != b);
            mismatches.push(format!("{flags:#x} {case:?}\nfirst difference: {first:?}"));
        }
    }
    std::fs::remove_dir_all(&directory).ok();
    println!(
        "{} of {} {name} match",
        cases.len() - mismatches.len(),
        cases.len()
    );
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

#[test]
fn cases_match_the_reader_helper() {
    let cases: Vec<_> = CASES.iter().map(|case| (LEGACY, *case)).collect();
    compare_with_helper("cases", &cases);
}

#[test]
fn option_cases_match_the_reader_helper() {
    compare_with_helper("option cases", OPTION_CASES);
}

/// runtest.c's `processNode` line format for `.rdr`/`.rde` results.
fn rde<R: BufRead>(mut reader: TextReader<R>) -> String {
    let mut out = String::new();
    while let Ok(true) = reader.read() {
        let flags = (reader.is_empty_element(), reader.has_value());
        let name = reader.name().unwrap_or("--");
        let (depth, kind) = (reader.depth(), reader.node_type() as i32);
        write!(
            out,
            "{depth} {kind} {name} {} {}",
            u8::from(flags.0),
            u8::from(flags.1)
        )
        .unwrap();
        match reader.value() {
            Some(value) => writeln!(out, " {value}").unwrap(),
            None => out.push('\n'),
        }
    }
    out
}

const DEFAULT_SOURCE: &str =
    "/Users/pbdeuchler/workspace/src/github.com/pbdeuchler/yaxml-oracle/libxml2-2.15.4";

/// Inputs whose `.rde` differs for documented reasons: EBCDIC and EUC-JP are
/// unsupported encodings, and `&#13;` in replacement text stays CR.
const RDE_DIFFERENCES: &[&str] = &["ebcdic_566012.xml", "ent11", "icu_parse_test.xml"];

#[test]
fn rde_results_match() {
    replay("rde", yaxml::options::NOENT, RDE_DIFFERENCES);
}

/// Inputs whose `.rdr` (no `NOENT`) differs for documented reasons: the
/// unsupported encodings.
const RDR_DIFFERENCES: &[&str] = &["ebcdic_566012.xml", "icu_parse_test.xml"];

#[test]
fn rdr_results_match() {
    replay("rdr", 0, RDR_DIFFERENCES);
}

/// Replays runtest.c's reader results with `extension` for parser `flags`.
fn replay(extension: &str, flags: u32, known: &[&str]) {
    let source =
        std::env::var_os("YAXML_LIBXML2_SRC").map_or(PathBuf::from(DEFAULT_SOURCE), PathBuf::from);
    if !source.join("result").is_dir() {
        eprintln!("skipping: no libxml2 source at {}", source.display());
        return;
    }
    let tests = source.join("test");
    let mut names: Vec<String> = std::fs::read_dir(source.join("result"))
        .unwrap()
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().into_string().ok()?;
            Some(name.strip_suffix(&format!(".{extension}"))?.to_owned())
        })
        .collect();
    names.sort();
    let (mut matched, mut differences) = (0, Vec::new());
    for name in &names {
        let expected =
            std::fs::read_to_string(source.join("result").join(format!("{name}.{extension}")))
                .unwrap();
        let ours = rde(reader(&tests, name, flags));
        if ours == expected {
            matched += 1;
        } else if !known.contains(&name.as_str()) {
            differences.push(format!("{name}\n--- libxml2\n{expected}--- yaxml\n{ours}"));
        }
    }
    println!("{matched} of {} .{extension} results match", names.len());
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

/// Like runtest.c's `xmlReaderForFile(flags)`: what the flags load comes
/// from beside the input; without `DTDLOAD` the external subset is not read.
fn reader(tests: &Path, name: &str, flags: u32) -> TextReader<std::io::BufReader<std::fs::File>> {
    let file = std::fs::File::open(tests.join(name)).unwrap();
    let mut files = FileResolver::new(tests).unwrap();
    let resolver = move |request: Request<'_>| files.resolve(request);
    let options = yaxml::ParseOptions::new(flags).with_resolver(name, Box::new(resolver));
    TextReader::with_options(std::io::BufReader::new(file), options).unwrap()
}

/// `xmllint --stream --debug --noblanks` against ours with `NOBLANKS` over the
/// inputs of the `.rdr` results, without and with `NOENT`: needs `YAXML_XMLLINT`.
#[test]
fn noblanks_streams_match_xmllint() {
    let Some(xmllint) = std::env::var_os("YAXML_XMLLINT") else {
        eprintln!("skipping: YAXML_XMLLINT is unset");
        return;
    };
    let source =
        std::env::var_os("YAXML_LIBXML2_SRC").map_or(PathBuf::from(DEFAULT_SOURCE), PathBuf::from);
    let tests = source.join("test");
    let mut names: Vec<String> = std::fs::read_dir(source.join("result"))
        .unwrap()
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().into_string().ok()?;
            Some(name.strip_suffix(".rdr")?.to_owned())
        })
        .collect();
    names.sort();
    // The pinned xmllint lacks ISO-8859-5, besides the unsupported encodings.
    let known = [RDE_DIFFERENCES, &["iso-8859-5.xml"]].concat();
    // With `NOENT`, libxml2 judges an entity's replacement text in a context
    // of its own (blank at its start: dropped before an element, at its end:
    // kept); here replacement text is never dropped.
    let replaced = [known.clone(), vec!["ent10", "ent2"]].concat();
    for (flags, arguments, known) in [
        (NOBLANKS, &["--noblanks"][..], &known),
        (NOBLANKS | NOENT, &["--noblanks", "--noent"], &replaced),
    ] {
        let (mut matched, mut differences) = (0, Vec::new());
        for name in &names {
            let output = Command::new(&xmllint)
                .args(["--stream", "--debug"])
                .args(arguments)
                .arg(tests.join(name))
                .current_dir(&tests)
                .output()
                .unwrap();
            let expected = String::from_utf8_lossy(&output.stdout);
            let ours = rde(reader(&tests, name, flags));
            if ours == expected {
                matched += 1;
            } else if !known.contains(&name.as_str()) {
                let first = ours.lines().zip(expected.lines()).find(|(a, b)| a != b);
                differences.push(format!("{name}: {first:?}"));
            }
        }
        println!("{matched} of {} match {arguments:?}", names.len());
        assert!(differences.is_empty(), "{arguments:?}: {differences:?}");
    }
}
