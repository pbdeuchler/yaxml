//! DTD validation of parser events: `valid::StreamValidator` over every
//! reader and `TextReader` with `DTDVALID` report what tree validation
//! reports, in document order and with libxml2's push wording; differentials
//! against the suites and the pinned libxml2 2.15.4 `xmllint --stream --valid`.

use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use yaxml::options::{DTDLOAD, DTDVALID, LEGACY, NOENT};
use yaxml::valid::{self, Code, Diagnostic, StreamValidator};
use yaxml::{
    BufferedReader, FileResolver, OwnedDocument, OwnedNodeKind, ParseOptions, ParserProperty,
    PushParser, Reader, Request, Resolver, TextReader,
};

use Code::*;

const DEFAULT_SOURCE: &str =
    "/Users/pbdeuchler/workspace/src/github.com/pbdeuchler/yaxml-oracle/libxml2-2.15.4";
const DEFAULT_SUITES: &str = "/Users/pbdeuchler/workspace/src/github.com/pbdeuchler/yaxml-suites";

const AB: &str = "<!ELEMENT r (a,b)><!ELEMENT a EMPTY><!ELEMENT b EMPTY>";

/// `(document, its stream diagnostics in report order as (code, element,
/// message), whether tree validation reports the same)`.
/// A diagnostic as a table states it: code, element index, message.
type Shown = (Code, Option<usize>, &'static str);

const CASES: &[(&str, &[Shown], bool)] = &[
    (
        "<!DOCTYPE r [<!ELEMENT r (a,b)><!ELEMENT a EMPTY><!ELEMENT b EMPTY>]><r><a/><b/></r>",
        &[],
        true,
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r (a,b)><!ELEMENT a EMPTY><!ELEMENT b EMPTY>]><r><b/><a/></r>",
        &[(
            ContentModel,
            Some(0),
            "Element r content does not follow the DTD, Misplaced b",
        )],
        true,
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r (a,b)><!ELEMENT a EMPTY><!ELEMENT b EMPTY>]><r><a/></r>",
        &[(
            ContentModel,
            Some(0),
            "Element r content does not follow the DTD, Expecting more children",
        )],
        true,
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r (a)><!ELEMENT a EMPTY>]><r> <a/>\n<!--c-->text<a/></r>",
        &[(
            ContentModel,
            Some(0),
            "Element r content does not follow the DTD, Text not allowed",
        )],
        true,
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r (a)><!ELEMENT a EMPTY>]><r><a/><![CDATA[ ]]></r>",
        &[(
            ContentModel,
            Some(0),
            "Element r content does not follow the DTD, Text not allowed",
        )],
        true,
    ),
    // Attributes are checked at the start tag, content as it comes; the
    // tree reports the parent's content first.
    (
        "<!DOCTYPE r [<!ELEMENT r (a)><!ELEMENT a EMPTY><!ATTLIST r x (p) #IMPLIED>]><r x='q'><b/></r>",
        &[
            (
                AttributeValue,
                Some(0),
                "Value \"q\" for attribute x of r is not among the enumerated set",
            ),
            (
                ContentModel,
                Some(0),
                "Element r content does not follow the DTD, Misplaced b",
            ),
            (UnknownElement, Some(1), "No declaration for element b"),
        ],
        true,
    ),
    // Once per element: EMPTY, (#PCDATA); mixed content per child.
    (
        "<!DOCTYPE r [<!ELEMENT r EMPTY>]><r>x<?p?></r>",
        &[(
            NotEmpty,
            Some(0),
            "Element r was declared EMPTY this one has content",
        )],
        true,
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r (#PCDATA)><!ELEMENT a EMPTY>]><r><a/><a/></r>",
        &[(
            NotPcdata,
            Some(0),
            "Element r was declared #PCDATA but contains non text nodes",
        )],
        true,
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r (#PCDATA|a)*><!ELEMENT a EMPTY><!ELEMENT b EMPTY>]><r><b/>x<b/></r>",
        &[
            (
                InvalidChild,
                Some(0),
                "Element b is not declared in r list of possible children",
            ),
            (
                InvalidChild,
                Some(0),
                "Element b is not declared in r list of possible children",
            ),
        ],
        true,
    ),
    // Required attributes at the start tag, IDREFs when the document element ends.
    (
        "<!DOCTYPE r [<!ELEMENT r ANY><!ATTLIST r i ID #IMPLIED f IDREF #IMPLIED s IDREFS #REQUIRED>]><r f='z' s='a b'><r i='a' s='a'/><r i='a' s='q'/></r>",
        &[
            (IdRedefined, Some(2), "ID a already defined"),
            (
                UnknownId,
                Some(0),
                "IDREF attribute f references an unknown ID \"z\"",
            ),
            (
                UnknownId,
                Some(0),
                "IDREFS attribute s references an unknown ID \"b\"",
            ),
            (
                UnknownId,
                Some(2),
                "IDREFS attribute s references an unknown ID \"q\"",
            ),
        ],
        true,
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r EMPTY><!ATTLIST r a CDATA #REQUIRED>]><r/>",
        &[(
            MissingAttribute,
            Some(0),
            "Element r does not carry attribute a",
        )],
        true,
    ),
    // The DOCTYPE name, and declaration-time diagnostics first.
    (
        "<!DOCTYPE x [<!ELEMENT r EMPTY><!ELEMENT r ANY>]><r/>",
        &[
            (ElementRedefined, None, "Redefinition of element r"),
            (
                RootName,
                Some(0),
                "root and DTD name do not match 'r' and 'x'",
            ),
        ],
        true,
    ),
    // Entity references are transparent.
    (
        "<!DOCTYPE r [<!ELEMENT r (a)><!ELEMENT a EMPTY><!ENTITY e '<a/>'>]><r>&e;</r>",
        &[],
        true,
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r (a)><!ELEMENT a EMPTY><!ENTITY e '<a/><a/>'>]><r>&e;</r>",
        &[(
            ContentModel,
            Some(0),
            "Element r content does not follow the DTD, Misplaced a",
        )],
        true,
    ),
    (
        "<!DOCTYPE r [<!ELEMENT r (s,s)><!ELEMENT s (a|a)><!ELEMENT a EMPTY>]><r><s><a/></s><s><b/></s></r>",
        &[
            (
                ContentNotDeterministic,
                None,
                "Content model of s is not deterministic",
            ),
            (UnknownElement, Some(4), "No declaration for element b"),
        ],
        true,
    ),
    // libxml2's parser validates nothing without a declaration; the tree
    // (`xmlValidateDocument`) checks an empty DTD.
    (
        "<r/>",
        &[(NoDtd, None, "Validation failed: no DTD found !")],
        true,
    ),
    (
        "<!DOCTYPE r []><r><a/></r>",
        &[(NoDtd, None, "Validation failed: no DTD found !")],
        false,
    ),
];

/// Validates `input` from a `BufferedReader`, passing its DTD after each event.
fn stream(input: &[u8], options: ParseOptions) -> Vec<Diagnostic<usize>> {
    let mut reader = BufferedReader::with_options(input, options).unwrap();
    let mut validator = StreamValidator::new();
    while let Some((event, origin)) = reader.read_event().unwrap() {
        validator.event(&event, origin);
        if let Some(dtd) = reader.dtd() {
            validator.set_dtd(dtd);
        }
    }
    validator.finish()
}

fn shown(diagnostics: &[Diagnostic<usize>]) -> Vec<(Code, Option<usize>, &str)> {
    let shown = diagnostics.iter();
    shown
        .map(|d| (d.code, d.node, d.message.as_str()))
        .collect()
}

/// A diagnostic both modes word alike: `(code, element index, offset,
/// message)`, without content-model and missing-DTD wording.
type Projection = (u16, Option<usize>, Option<usize>, String);

fn project(code: Code, node: Option<usize>, offset: Option<usize>, message: &str) -> Projection {
    let worded = !matches!(code, ContentModel | NoDtd);
    let message = if worded { message } else { "" };
    let offset = offset.filter(|_| code != NoDtd);
    (code as u16, node, offset, message.to_owned())
}

fn sorted(mut projections: Vec<Projection>) -> Vec<Projection> {
    projections.sort();
    projections
}

fn tree_projection(document: &OwnedDocument) -> Vec<Projection> {
    let elements: Vec<_> = (document.nodes())
        .filter(|node| node.kind() == OwnedNodeKind::Element)
        .map(|node| node.id())
        .collect();
    let index = |id| elements.iter().position(|&element| element == id);
    let diagnostics = document.validate().into_iter();
    sorted(
        diagnostics
            .map(|d| project(d.code, d.node.and_then(index), d.offset, &d.message))
            .collect(),
    )
}

fn stream_projection(diagnostics: &[Diagnostic<usize>]) -> Vec<Projection> {
    sorted(
        diagnostics
            .iter()
            .map(|d| project(d.code, d.node, d.offset, &d.message))
            .collect(),
    )
}

#[test]
fn streams_report_in_document_order_with_push_wording() {
    for &(input, expected, _) in CASES {
        for flags in [0, NOENT] {
            let diagnostics = stream(input.as_bytes(), ParseOptions::new(flags));
            assert_eq!(shown(&diagnostics), expected, "{input} {flags}");
            let errors = expected.iter().any(|(code, ..)| !code.is_warning());
            assert_eq!(valid::is_valid(&diagnostics), !errors, "{input}");
        }
    }
}

#[test]
fn streams_report_what_trees_report() {
    for &(input, expected, agree) in CASES {
        for flags in [0, NOENT] {
            let tree =
                OwnedDocument::parse_with(input.as_bytes(), ParseOptions::new(flags)).unwrap();
            let diagnostics = stream(input.as_bytes(), ParseOptions::new(flags));
            let (tree, stream) = (tree_projection(&tree), stream_projection(&diagnostics));
            assert_eq!(tree == stream, agree, "{input}\n{tree:?}\n{stream:?}");
            assert_eq!(
                valid::is_valid(&diagnostics),
                expected.is_empty(),
                "{input}"
            );
        }
    }
}

/// Every reader drives the validator; the slice reader and the push parser
/// expose no DTD to pass between events, so the internal subset is used.
#[test]
fn every_reader_feeds_the_validator() {
    for &(input, expected, _) in CASES {
        let mut reader = Reader::new(input.as_bytes()).unwrap();
        let mut validator = StreamValidator::new();
        while let Some(event) = reader.read_event().unwrap() {
            validator.event(&event, reader.event_origin().unwrap());
        }
        assert_eq!(shown(&validator.finish()), expected, "slice {input}");

        let mut validator = StreamValidator::new();
        let mut parser = PushParser::new();
        for byte in input.as_bytes().chunks(1) {
            let fed = parser.feed(byte, |event, origin| {
                validator.event(event, origin);
                ControlFlow::Continue(())
            });
            fed.unwrap();
        }
        parser
            .finish(|event, origin| {
                validator.event(event, origin);
                ControlFlow::Continue(())
            })
            .unwrap();
        assert_eq!(shown(&validator.finish()), expected, "push {input}");
    }
}

fn serve(files: &'static [(&'static str, &'static str)]) -> Box<dyn Resolver> {
    Box::new(move |request: Request<'_>| {
        let found = files.iter().find(|(uri, _)| *uri == request.uri);
        Ok(found.map(|(_, text)| text.as_bytes().to_vec()))
    })
}

const EXTERNAL: &[(&str, &str)] = &[(
    "r.dtd",
    "<!ELEMENT r (a*)><!ELEMENT a EMPTY><!ATTLIST a t NMTOKEN 'x' n NMTOKENS #IMPLIED i ID #IMPLIED>",
)];

/// The external subset a reader loads while reading the document element,
/// and the standalone checks its parser makes, reach the validator.
#[test]
fn external_subsets_and_parser_checks_reach_the_stream() {
    let table: &[(&str, &[Code])] = &[
        ("<!DOCTYPE r SYSTEM 'r.dtd'><r><a i='x'/><a/></r>", &[]),
        (
            "<!DOCTYPE r SYSTEM 'r.dtd'><r><a i='x'/><a i='x'/><b/></r>",
            &[IdRedefined, ContentModel, UnknownElement],
        ),
        (
            "<?xml version='1.0' standalone='yes'?><!DOCTYPE r SYSTEM 'r.dtd'><r> <a t='x' n=' p  q'/></r>",
            &[StandaloneWhiteSpace, NotStandalone],
        ),
        (
            "<?xml version='1.0' standalone='yes'?><!DOCTYPE r SYSTEM 'r.dtd'><r><a/></r>",
            &[StandaloneDefaulted],
        ),
        ("<!DOCTYPE r SYSTEM 'missing.dtd'><r/>", &[NoDtd]),
    ];
    let options = || ParseOptions::new(LEGACY | DTDLOAD).with_resolver("doc.xml", serve(EXTERNAL));
    for &(input, expected) in table {
        let diagnostics = stream(input.as_bytes(), options());
        let codes: Vec<_> = diagnostics.iter().map(|d| d.code).collect();
        assert_eq!(codes, expected, "{input}");
        if expected != [NoDtd] {
            let tree = OwnedDocument::parse_with(input.as_bytes(), options()).unwrap();
            assert_eq!(
                tree_projection(&tree),
                stream_projection(&diagnostics),
                "{input}"
            );
        }
    }
}

fn read_all<R: std::io::BufRead>(reader: &mut TextReader<R>) {
    while reader.read().unwrap() {}
}

/// `XML_PARSE_DTDVALID` or `XML_PARSER_VALIDATE` on a reader: (how, document,
/// codes, `is_valid`).
#[test]
fn text_readers_validate_while_reading() {
    let ab = format!("<!DOCTYPE r [{AB}]><r><a/><b/></r>");
    let ba = format!("<!DOCTYPE r [{AB}]><r><b/><a/></r>");
    let table: &[(&str, &str, &[Code], bool)] = &[
        ("option", &ab, &[], true),
        ("option", &ba, &[ContentModel], false),
        ("property", &ab, &[], true),
        ("property", &ba, &[ContentModel], false),
        ("property", "<r/>", &[NoDtd], false),
        ("none", &ba, &[], false),
        ("cleared", &ba, &[], false),
    ];
    for &(how, input, expected, valid) in table {
        let input = input.as_bytes();
        let mut reader = match how {
            "option" => TextReader::with_options(input, ParseOptions::new(DTDVALID)).unwrap(),
            _ => TextReader::new(input),
        };
        if how == "property" || how == "cleared" {
            reader
                .set_parser_property(ParserProperty::Validate, true)
                .unwrap();
        }
        if how == "cleared" {
            reader
                .set_parser_property(ParserProperty::Validate, false)
                .unwrap();
        }
        let validating = matches!(how, "option" | "property");
        assert_eq!(
            reader.parser_property(ParserProperty::Validate),
            validating,
            "{how}"
        );
        read_all(&mut reader);
        let codes: Vec<_> = reader.dtd_diagnostics().iter().map(|d| d.code).collect();
        assert_eq!(codes, expected, "{how}");
        assert_eq!(reader.is_valid(), valid, "{how}");
        if validating {
            assert_eq!(
                reader.dtd_diagnostics(),
                stream(input, ParseOptions::default()),
                "{how}"
            );
        }
    }
}

/// Validation follows the flags across `reset`, loads the external subset
/// like libxml2's `DTDVALID`, and combines with schema validation.
#[test]
fn text_reader_validation_resets_loads_and_combines() {
    let options = ParseOptions::new(DTDVALID).with_resolver("doc.xml", serve(EXTERNAL));
    let mut reader =
        TextReader::with_options(&b"<!DOCTYPE r SYSTEM 'r.dtd'><r><a/></r>"[..], options).unwrap();
    assert!(reader.parser_property(ParserProperty::LoadDtd));
    read_all(&mut reader);
    assert!(reader.is_valid() && reader.dtd_diagnostics().is_empty());
    reader.reset(&b"<!DOCTYPE r [<!ELEMENT r EMPTY>]><r>x</r>"[..]);
    read_all(&mut reader);
    assert_eq!(reader.dtd_diagnostics()[0].code, NotEmpty);
    assert!(!reader.is_valid());

    let schema = yaxml::relaxng::Schema::parse(
        b"<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><text/></element>",
        "s.rng",
        Box::new(|_: Request<'_>| Ok(None)),
        &yaxml::relaxng::Limits::default(),
    )
    .unwrap();
    for (input, valid) in [
        ("<!DOCTYPE r [<!ELEMENT r (#PCDATA)>]><r>x</r>", true),
        ("<!DOCTYPE r [<!ELEMENT r EMPTY>]><r>x</r>", false),
    ] {
        let mut reader =
            TextReader::with_options(input.as_bytes(), ParseOptions::new(DTDVALID)).unwrap();
        reader.relaxng_set_schema(Some(&schema)).unwrap();
        read_all(&mut reader);
        assert!(reader.relaxng_diagnostics().is_empty(), "{input}");
        assert_eq!(reader.is_valid(), valid, "{input}");
    }
}

/// Wide and deep documents validate in one pass over bounded state.
#[test]
fn long_and_deep_documents_validate() {
    let wide = format!(
        "<!DOCTYPE r [<!ELEMENT r (a,b?)*><!ELEMENT a EMPTY><!ELEMENT b EMPTY>]><r>{}</r>",
        "<a/><b/>".repeat(50_000)
    );
    assert!(stream(wide.as_bytes(), ParseOptions::default()).is_empty());
    let deep = format!(
        "<!DOCTYPE r [<!ELEMENT r (r?)>]>{}{}",
        "<r>".repeat(200),
        "</r>".repeat(200)
    );
    assert!(stream(deep.as_bytes(), ParseOptions::default()).is_empty());
    let bad = deep.replacen("</r>", "<x/></r>", 1);
    let codes: Vec<_> = stream(bad.as_bytes(), ParseOptions::default())
        .iter()
        .map(|d| (d.code, d.node))
        .collect();
    assert_eq!(
        codes,
        [(ContentModel, Some(199)), (UnknownElement, Some(200))]
    );
}

fn source() -> PathBuf {
    std::env::var_os("YAXML_LIBXML2_SRC").map_or(DEFAULT_SOURCE.into(), PathBuf::from)
}

fn suites() -> PathBuf {
    std::env::var_os("YAXML_SUITES").map_or(DEFAULT_SUITES.into(), PathBuf::from)
}

/// libxml2's `test/valid`, `test/errors` and `test` documents, and the XMLTS
/// `valid` and `invalid` cases, as `(root, relative paths)`.
fn corpora() -> Vec<(PathBuf, Vec<String>)> {
    let libxml2 = source();
    let mut documents = Vec::new();
    for dir in ["test/valid", "test/errors", "test"] {
        let mut found: Vec<_> = (std::fs::read_dir(libxml2.join(dir)).unwrap().flatten())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".xml"))
            .map(|name| format!("{dir}/{name}"))
            .collect();
        found.sort();
        documents.extend(found);
    }
    let catalog = std::fs::read_to_string(suites().join("xmlconf-catalog.tsv")).unwrap();
    let xmlts = catalog
        .lines()
        .map(|line| line.split('\t').collect::<Vec<_>>())
        .filter(|fields| matches!(fields.get(1), Some(&("valid" | "invalid"))))
        .map(|fields| fields[8].to_owned())
        .collect();
    vec![(libxml2, documents), (suites().join("xmlconf"), xmlts)]
}

fn loading(root: &Path, path: &str, flags: u32) -> ParseOptions {
    let resolver = Box::new(FileResolver::new(root).unwrap());
    ParseOptions::new(flags).with_resolver(path, resolver)
}

/// Stream == tree over the suites, with and without entity substitution:
/// the same diagnostics (content-model wording aside) for every document
/// whose DTD declares something.
#[test]
#[ignore = "requires the libxml2 source tree (YAXML_LIBXML2_SRC) and the XMLTS suites (YAXML_SUITES)"]
fn streams_equal_trees_on_the_suites() {
    let (mut compared, mut excused_count, mut differences) = (0, 0, Vec::new());
    for (root, documents) in corpora() {
        for path in &documents {
            for flags in [DTDLOAD, LEGACY | DTDLOAD] {
                let input = std::fs::read(root.join(path)).unwrap_or_default();
                let Ok(tree) = OwnedDocument::parse_with(&input, loading(&root, path, flags))
                else {
                    continue;
                };
                let diagnostics = stream(&input, loading(&root, path, flags));
                // A DTD declaring nothing: libxml2's parser stops, the tree validates.
                let excused = tree.dtd().is_some() && diagnostics.iter().any(|d| d.code == NoDtd);
                compared += 1;
                excused_count += usize::from(excused);
                if !excused && tree_projection(&tree) != stream_projection(&diagnostics) {
                    differences.push(format!("{path} {flags:#x}"));
                }
            }
        }
    }
    eprintln!(
        "compared {compared} (DTD declaring nothing: {excused_count}), differing {}",
        differences.len()
    );
    assert!(compared > 2000, "{compared}");
    assert!(differences.is_empty(), "{differences:#?}");
}

/// The validity errors and warnings `xmllint` prints for `path` (with
/// `--stream` or not), or `None` when it reports a parser error.
fn xmllint(xmllint: &Path, root: &Path, path: &str, stream: bool) -> Option<Vec<(bool, String)>> {
    let mut command = std::process::Command::new(xmllint);
    if stream {
        command.arg("--stream");
    }
    let output = command
        .args(["--valid", "--noout", path])
        .current_dir(root)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("parser error") || stderr.contains("failed to parse") {
        return None;
    }
    let found = stderr.lines().filter_map(|line| {
        let error = |(_, message): (&str, &str)| (true, message.to_owned());
        let warning = |(_, message): (&str, &str)| (false, message.to_owned());
        (line.split_once("validity error : ").map(error))
            .or_else(|| line.split_once("validity warning : ").map(warning))
    });
    Some(found.collect())
}

/// IDREF messages without the attribute type (the tree's) or the line
/// (libxml2's reader, which has no attribute node to name).
fn idref_neutral(message: &str) -> String {
    let message = (message.strip_prefix("IDREFS "))
        .or_else(|| message.strip_prefix("IDREF "))
        .unwrap_or(message);
    match message.split_once(" line ") {
        Some((head, tail)) if head.starts_with("attribute ") => {
            format!(
                "{head}{}",
                tail.trim_start_matches(|c: char| c.is_ascii_digit())
            )
        }
        _ => message.to_owned(),
    }
}

/// Whether multiset `ours` is contained in multiset `theirs`.
fn contained(ours: &[String], theirs: &[String]) -> bool {
    let mut rest = theirs.to_vec();
    ours.iter().all(|message| {
        let found = rest.iter().position(|m| m == message);
        found.map(|at| rest.swap_remove(at)).is_some()
    })
}

#[derive(Default, Debug)]
struct Tally {
    compared: usize,
    verdicts: usize,
    messages: usize,
    differences: Vec<String>,
}

impl Tally {
    fn add(
        &mut self,
        path: &str,
        verdicts: (bool, bool),
        messages: bool,
        detail: impl Fn() -> String,
    ) {
        self.compared += 1;
        self.verdicts += usize::from(verdicts.0 == verdicts.1);
        self.messages += usize::from(messages);
        if verdicts.0 != verdicts.1 || !messages {
            self.differences.push(format!("{path}: {}", detail()));
        }
    }
}

/// Against the pinned `xmllint` over the suites. The reader's verdict is
/// `xmllint --stream --valid`'s and each of its messages is one libxml2
/// prints (libxml2 reports most content problems twice, from its parser
/// and from its reader's push validation). The tree's verdict and messages
/// are `xmllint --valid`'s, content-model wording aside.
#[test]
#[ignore = "requires YAXML_XMLLINT, the libxml2 source tree (YAXML_LIBXML2_SRC) and the XMLTS suites (YAXML_SUITES)"]
fn verdicts_and_messages_match_xmllint() {
    let program = PathBuf::from(std::env::var_os("YAXML_XMLLINT").expect("YAXML_XMLLINT"));
    let (mut stream, mut tree) = (Tally::default(), Tally::default());
    for (root, documents) in corpora() {
        for path in &documents {
            let Ok(input) = std::fs::read(root.join(path)) else {
                continue;
            };
            let errors = |found: &[(bool, String)]| found.iter().any(|(error, _)| *error);
            let messages = |found: Vec<(bool, String)>| found.into_iter().map(|(_, m)| m);
            let options = || loading(&root, path, DTDVALID);
            let mut reader = TextReader::with_options(&input[..], options()).unwrap();
            let read =
                std::iter::from_fn(|| reader.read().map(|more| more.then_some(())).transpose());
            if let (Ok(()), Some(theirs)) = (read.collect(), xmllint(&program, &root, path, true)) {
                let ours = reader.dtd_diagnostics();
                let verdicts = (!valid::is_valid(ours), errors(&theirs));
                let theirs: Vec<_> = messages(theirs).map(|m| idref_neutral(&m)).collect();
                let ours: Vec<_> = ours.iter().map(|d| idref_neutral(&d.message)).collect();
                let detail = || format!("{ours:#?} {theirs:#?}");
                stream.add(path, verdicts, contained(&ours, &theirs), detail);
            }
            if let (Ok(document), Some(theirs)) = (
                OwnedDocument::parse_with(&input, options()),
                xmllint(&program, &root, path, false),
            ) {
                let ours = document.validity();
                let verdicts = (!valid::is_valid(ours), errors(&theirs));
                // `xmlValidateDocument` says `no DTD found!`, the parser `Validation failed: no DTD found !`.
                let worded = |m: &String| !m.contains("content does not follow the DTD");
                let neutral = |m: String| {
                    if m.contains("no DTD found") {
                        "no DTD".into()
                    } else {
                        m
                    }
                };
                let mut theirs: Vec<_> = messages(theirs).filter(worded).map(neutral).collect();
                let mut ours: Vec<_> = (ours.iter().map(|d| neutral(d.message.clone())))
                    .filter(worded)
                    .collect();
                theirs.sort();
                ours.sort();
                let detail = || format!("{ours:#?} {theirs:#?}");
                tree.add(path, verdicts, ours == theirs, detail);
            }
        }
    }
    for (label, tally) in [("stream", &stream), ("tree", &tree)] {
        eprintln!(
            "{label}: {}/{} verdicts agree, {}/{} messages {}",
            tally.verdicts,
            tally.compared,
            tally.messages,
            tally.compared,
            if label == "stream" {
                "contained"
            } else {
                "equal"
            }
        );
    }
    eprintln!("stream differences:\n{}", stream.differences.join("\n"));
    eprintln!("tree differences:\n{}", tree.differences.join("\n"));
}

/// A kept entity reference is content of an `EMPTY` element, even when its
/// replacement is empty, as libxml2 counts the reference node; substituted,
/// an empty replacement leaves no content. `(flags, codes)` for both modes.
#[test]
fn entity_references_are_content_of_empty_elements() {
    let input = "<!DOCTYPE r [<!ELEMENT r EMPTY><!ENTITY e ''><!ENTITY f 'x'>]><r>&e;</r>";
    for (input, flags, expected) in [
        (input, 0, &[NotEmpty][..]),
        (input, NOENT, &[]),
        (&*input.replace("&e;", "&f;&e;"), NOENT, &[NotEmpty]),
    ] {
        let tree = OwnedDocument::parse_with(input.as_bytes(), ParseOptions::new(flags)).unwrap();
        let diagnostics = stream(input.as_bytes(), ParseOptions::new(flags));
        let codes: Vec<_> = diagnostics.iter().map(|d| d.code).collect();
        assert_eq!(codes, expected, "{input} {flags}");
        assert_eq!(
            tree_projection(&tree),
            stream_projection(&diagnostics),
            "{input}"
        );
    }
}
