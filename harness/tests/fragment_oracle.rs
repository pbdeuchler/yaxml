//! Differential test against `tooling/libxml2_fragment.c` (libxml2 2.15.4):
//! `Editor::parse_content` against `xmlParseInNodeContext` (any node of a
//! document with namespaces, default namespaces and DTD entities; random
//! well-formed and malformed chunks under several option sets) and against
//! `xmlParseBalancedChunkMemory` (the document node, default options).
//! Compares the returned node list, the document after appending it to the
//! element parsed in, and the error codes. Runs only when
//! `YAXML_FRAGMENT_HELPER` names the compiled helper (`YAXML_FRAGMENT_CASES`,
//! `YAXML_FRAGMENT_SEED`); see `docs/owned-dom.md`.
#![forbid(unsafe_code)]

#[path = "../examples/support/process.rs"]
mod process;

use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

use process::ProcessLimits;
use yaxml::options::{NOBLANKS, NOCDATA, NOENT, NSCLEAN, RECOVER};
use yaxml::{
    EditErrorKind, EditLimits, Error, LineIndex, OwnedDocument, OwnedNode, OwnedNodeId,
    OwnedNodeKind, ParseOptions,
};

struct Random(u64);

impl Random {
    fn next(&mut self, bound: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as usize) % bound
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.next(items.len())]
    }
}

const CONTEXTS: &[&str] = &[
    "<r/>",
    "<r xmlns:p='urn:p' xmlns:q='urn:q'><c xmlns='urn:d'>t<d xmlns=''/></c><p:e/></r>",
    "<!DOCTYPE r [<!ENTITY e 'ent'><!ENTITY m '<p:x>m</p:x>'><!ENTITY n '<y>&e;</y>'>\
     <!ENTITY x SYSTEM 'missing.xml'><!NOTATION nt SYSTEM 'nt'>\
     <!ENTITY u SYSTEM 'u.bin' NDATA nt><!ATTLIST g a CDATA 'dflt' t NMTOKENS #IMPLIED>]>\
     <r xmlns:p='urn:p'>&e;<s/>&m;&n;</r>",
    "<r xmlns='urn:default'><a><!--c--><?pi x?>text</a><b xmlns:p='urn:p'/></r>",
    "<!DOCTYPE r SYSTEM 'absent.dtd'><r/>",
    "<r xmlns:p='urn:p'><p:a xmlns:p='urn:other'><b/></p:a></r>",
    "<!--before--><r xml:lang='en'><a/></r><?after?>",
];

/// Content that may be well-formed, depending on the context's
/// declarations.
const GOOD: &[&str] = &[
    "hello",
    " ",
    "\n",
    "a&amp;b",
    "&#x41;",
    "x > y",
    "\u{e9}",
    "&lt;",
    "&e;",
    "&m;",
    "&n;",
    "&x;",
    "&u;",
    "<a/>",
    "<a>t</a>",
    "<p:a/>",
    "<q:b>z</q:b>",
    "<d xmlns='urn:new'><e/></d>",
    "<p:f xmlns:p='urn:shadow'><p:g/></p:f>",
    "<g a='1' p:b='2'/>",
    "<h xml:lang='fr' xml:id='i1'/>",
    "<i a='&e;'/>",
    "<k xmlns=''><l/></k>",
    "<m t=' a  b '/>",
    "<!--c-->",
    "<?pi data?>",
    "<![CDATA[<x>]]>",
];

/// Malformed content.
const BAD: &[&str] = &[
    "]]>",
    "&undefined;",
    "<u:c/>",
    "<j a='1' a='2'/>",
    "<?xml version='1.0'?>",
    "<!DOCTYPE z>",
    "</r>",
    "<o>",
    "</o>",
    "<v",
    "<w></z>",
    "&",
    "&bad",
    "<1/>",
    "<!-- - -- -->",
    "\u{1}",
    "<p:a xmlns:p=''/>",
];

const OPTIONS: &[u32] = &[
    0,
    0,
    RECOVER,
    NOENT,
    NOENT | RECOVER,
    NOBLANKS,
    NOCDATA,
    NSCLEAN,
];

/// Up to five pieces, a fifth of them malformed, sometimes nested in an
/// element.
fn chunk(random: &mut Random) -> String {
    let mut chunk = String::new();
    for _ in 0..random.next(6) {
        let pieces = if random.next(5) == 0 { BAD } else { GOOD };
        chunk.push_str(random.pick(pieces));
        if random.next(5) == 0 {
            chunk = format!("<n>{chunk}</n>");
        }
    }
    chunk
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn field(value: Option<&str>) -> String {
    match value {
        None => "-".to_owned(),
        Some("") => "0".to_owned(),
        Some(value) => hex(value.as_bytes()),
    }
}

fn unhex(field: &str) -> Option<String> {
    match field {
        "-" => None,
        "0" => Some(String::new()),
        field => {
            let bytes: Vec<u8> = (0..field.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&field[i..i + 2], 16).unwrap())
                .collect();
            Some(String::from_utf8_lossy(&bytes).into_owned())
        }
    }
}

/// The helper's lines for a node list: preorder, not entering references.
fn dump(node: OwnedNode<'_>, depth: usize, lines: &mut Vec<String>) {
    let line = match node.kind() {
        OwnedNodeKind::Element => {
            let name = node.name().unwrap();
            let (prefix, local) = match name.namespace() {
                Some(_) => (name.prefix(), name.local()),
                None => (None, name.qualified()),
            };
            let (prefix, local, uri) = (field(prefix), field(Some(local)), field(name.namespace()));
            lines.push(format!("E {depth} {prefix} {local} {uri}"));
            let (declarations, attributes): (Vec<_>, Vec<_>) = node
                .attributes()
                .partition(|attribute| attribute.name.namespace() == Some(XMLNS));
            for declaration in declarations {
                let prefix = match declaration.name.prefix() {
                    Some(_) => Some(declaration.name.local()),
                    None => None,
                };
                lines.push(format!(
                    "D {depth} {} {}",
                    field(prefix),
                    field(Some(&declaration.value))
                ));
            }
            for attribute in attributes {
                let name = &attribute.name;
                let (prefix, local) = match name.namespace() {
                    Some(_) => (name.prefix(), name.local()),
                    None => (None, name.qualified()),
                };
                lines.push(format!(
                    "A {depth} {} {} {} {}",
                    field(prefix),
                    field(Some(local)),
                    field(name.namespace()),
                    field(Some(&attribute.value))
                ));
            }
            for child in node.children() {
                dump(child, depth + 1, lines);
            }
            return;
        }
        OwnedNodeKind::Text => format!("T {depth} {}", field(node.text())),
        OwnedNodeKind::Cdata => format!("C {depth} {}", field(node.text())),
        OwnedNodeKind::Comment => format!("M {depth} {}", field(node.text())),
        OwnedNodeKind::ProcessingInstruction => format!(
            "P {depth} {} {}",
            field(node.processing_instruction_target()),
            field(node.text())
        ),
        OwnedNodeKind::EntityRef => format!("R {depth} {}", field(node.entity_name())),
        kind => format!("? {depth} {kind:?}"),
    };
    lines.push(line);
}

const XMLNS: &str = "http://www.w3.org/2000/xmlns/";

/// libxml2's code for a yaxml error in `chunk`.
fn code(error: Error, chunk: &str) -> u16 {
    error.describe(&LineIndex::content(chunk.as_bytes())).code
}

/// What one side reported: `Ok(lines)` for a returned list (and the
/// document after inserting it), or `Err(code)`; and the first error code.
type Outcome = (Result<Vec<String>, u16>, Option<u16>);

fn parse_their(lines: &[String]) -> Outcome {
    let status: Vec<&str> = lines[0].split(' ').collect();
    let code: u16 = status[1].parse().unwrap();
    let first = status[2].parse().ok();
    let list = lines[1..lines.len() - 1].to_vec();
    // A list (even under RECOVER) is success; an empty one is libxml2's code.
    let nodes = list.iter().any(|line| !line.starts_with("DOC"));
    match nodes || code == 0 {
        true => (Ok(list), first),
        false => (Err(code), first),
    }
}

/// The element parsed in, as libxml2 finds it, and whether the list may be
/// inserted there (not in entity content).
fn insertion(node: OwnedNode<'_>) -> Option<OwnedNodeId> {
    let element = std::iter::successors(Some(node), |node| {
        node.parent()
            .filter(|parent| parent.kind() != OwnedNodeKind::EntityRef)
    })
    .find(|node| node.kind() == OwnedNodeKind::Element)?;
    let in_entity = std::iter::successors(element.parent(), |node| node.parent())
        .any(|node| node.kind() == OwnedNodeKind::EntityRef);
    (!in_entity).then(|| element.id())
}

fn ours(xml: &str, index: usize, options: u32, chunk: &str, balanced: bool) -> (Outcome, bool) {
    let mut document = OwnedDocument::parse(xml).unwrap();
    let node = document.nodes().nth(index).unwrap();
    let (context, insert) = match balanced {
        true => (document.root().id(), None),
        false => (node.id(), insertion(node)),
    };
    let mut editor = document.edit(EditLimits::default()).unwrap();
    let parsed = editor.parse_content(context, chunk.as_bytes(), ParseOptions::new(options));
    let fragment = match parsed {
        Ok(fragment) => fragment,
        Err(error) => {
            let (code, first) = match error.kind {
                EditErrorKind::Parse(kind) => {
                    let offset = error.value_offset.unwrap();
                    let code = code(Error { kind, offset }, chunk);
                    (code, code)
                }
                // An unusable node: libxml2 reports XML_ERR_ARGUMENT and
                // returns XML_ERR_INTERNAL_ERROR.
                _ => (1, 115),
            };
            return ((Err(code), Some(first)), insert.is_some());
        }
    };
    let codes: Vec<u16> = fragment.errors.iter().map(|&e| code(e, chunk)).collect();
    let first = codes.first().copied();
    if fragment.nodes.is_empty() && !codes.is_empty() {
        return ((Err(*codes.last().unwrap()), first), insert.is_some());
    }
    let mut lines = Vec::new();
    for &node in &fragment.nodes {
        dump(editor.document().node(node).unwrap(), 0, &mut lines);
    }
    if let Some(parent) = insert {
        for &node in &fragment.nodes {
            editor.append_child(parent, node).unwrap();
        }
        let saved = yaxml::save::to_vec(&document, &yaxml::save::Options::default()).unwrap();
        lines.push(format!(
            "DOC {}",
            field(Some(std::str::from_utf8(&saved).unwrap()))
        ));
    }
    ((Ok(lines), first), insert.is_some())
}

/// Documented divergence: libxml2's namespace errors (`XML_FROM_NAMESPACE`,
/// codes 200..) keep the node, where strict yaxml fails, as for a document.
fn namespace_only(theirs: &Outcome, ours: &Outcome) -> bool {
    let reported = theirs.1.is_some_and(|code| (200..300).contains(&code));
    matches!(ours.0, Err(200..=206)) && theirs.0.is_ok() && reported
}

/// Documented divergence: libxml2 reuses an entity's content parsed at the
/// document's reference to it, outside any namespace scope and under the
/// document's options; yaxml parses each reference where it stands. `m`
/// holds a prefixed element, `n` a reference.
fn cached_entity(chunk: &str) -> bool {
    chunk.contains("&m;") || chunk.contains("&n;")
}

/// The same chunk as the content of a whole document's root element `w`,
/// with the context's internal subset and in-scope namespace declarations:
/// a control telling divergences in-context parsing adds from those every
/// document shows (recovery repairs, `NOBLANKS`). Content has no external
/// subset (an undeclared entity is fatal), and under `RECOVER` it ends at
/// the end of input, so `w` stays open.
fn whole(xml: &str, index: usize, options: u32, chunk: &str, balanced: bool) -> String {
    let doctype = match xml.starts_with("<!DOCTYPE") {
        true => xml
            .find("]>")
            .map_or_else(|| xml.find('>').unwrap() + 1, |end| end + 2),
        false => 0,
    };
    let document = OwnedDocument::parse(xml).unwrap();
    let node = document.nodes().nth(index).unwrap();
    let element = std::iter::successors(Some(node), |node| {
        node.parent()
            .filter(|parent| parent.kind() != OwnedNodeKind::EntityRef)
    })
    .find(|node| node.kind() == OwnedNodeKind::Element)
    .filter(|_| !balanced);
    let mut declarations = String::new();
    for binding in element.map(OwnedNode::namespaces).unwrap_or_default() {
        match binding.prefix {
            Some("xml") => {}
            Some(prefix) => write!(declarations, " xmlns:{prefix}='{}'", binding.uri).unwrap(),
            None => write!(declarations, " xmlns='{}'", binding.uri).unwrap(),
        }
    }
    let doctype = xml[..doctype].replace(" SYSTEM 'absent.dtd'", "");
    let end = if options & RECOVER == 0 { "</w>" } else { "" };
    format!("{doctype}<w{declarations}>{chunk}{end}")
}

fn ours_whole(xml: &str, options: u32) -> Outcome {
    match OwnedDocument::parse_with(xml.as_bytes(), ParseOptions::new(options)) {
        Ok(document) => {
            let mut lines = Vec::new();
            for child in document.root_element().children() {
                dump(child, 0, &mut lines);
            }
            let first = document.errors().first();
            let first = first.map(|e| e.describe(&LineIndex::new(xml.as_bytes())).code);
            (Ok(lines), first)
        }
        Err(error) => {
            let code = error.describe(&LineIndex::new(xml.as_bytes())).code;
            (Err(code), Some(code))
        }
    }
}

fn agrees(theirs: &Outcome, ours: &Outcome) -> bool {
    theirs.0.is_err() == ours.0.is_err() && (theirs.0.is_err() || theirs.0 == ours.0)
}

/// The helper's blocks for `requests`, one per line.
fn run(helper: &Path, requests: &str) -> Vec<Vec<String>> {
    let output = process::run(
        helper,
        &[],
        requests.as_bytes(),
        ProcessLimits {
            timeout: Duration::from_secs(300),
            max_output_bytes: 1 << 30,
        },
    )
    .expect("helper runs");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let mut blocks = vec![Vec::new()];
    for line in stdout.lines() {
        blocks.last_mut().unwrap().push(line.to_owned());
        if line == "END" {
            blocks.push(Vec::new());
        }
    }
    blocks.pop();
    blocks
}

fn decode(result: &Result<Vec<String>, u16>) -> Vec<String> {
    let hex = |field: &str| {
        field.len() > 2
            && field.len().is_multiple_of(2)
            && field.bytes().all(|b| b.is_ascii_hexdigit())
    };
    match result {
        Err(code) => vec![format!("error {code}")],
        Ok(lines) => lines
            .iter()
            .map(|line| {
                let fields = line.split(' ').map(|field| match hex(field) {
                    true => format!("{:?}", unhex(field).unwrap_or_default()),
                    false => field.to_owned(),
                });
                fields.collect::<Vec<_>>().join(" ")
            })
            .collect(),
    }
}

#[test]
fn in_context_parsing_matches_libxml2() {
    let Some(helper) = std::env::var_os("YAXML_FRAGMENT_HELPER") else {
        eprintln!("YAXML_FRAGMENT_HELPER unset; skipping the libxml2 fragment differential");
        return;
    };
    let helper = Path::new(&helper);
    let cases: usize = std::env::var("YAXML_FRAGMENT_CASES")
        .ok()
        .and_then(|cases| cases.parse().ok())
        .unwrap_or(6000);
    let seed = std::env::var("YAXML_FRAGMENT_SEED")
        .ok()
        .and_then(|seed| seed.parse().ok());
    let mut random = Random(seed.unwrap_or(0xf7a9));
    let (mut requests, mut expected) = (String::new(), Vec::new());
    for _ in 0..cases {
        let xml = random.pick(CONTEXTS);
        let count = OwnedDocument::parse(xml).unwrap().nodes().count();
        let index = random.next(count);
        let balanced = random.next(8) == 0;
        let options = if balanced {
            0
        } else {
            OPTIONS[random.next(OPTIONS.len())]
        };
        let chunk = chunk(&mut random);
        let (outcome, insert) = ours(xml, index, options, &chunk, balanced);
        let mode = if balanced { 'B' } else { 'I' };
        writeln!(
            requests,
            "{}\t{index}\t{options}\t{}\t{mode}\t{}",
            hex(xml.as_bytes()),
            hex(chunk.as_bytes()),
            u8::from(insert)
        )
        .unwrap();
        expected.push((xml, index, options, chunk, balanced, outcome));
    }
    let (mut agree, mut codes, mut first_codes, mut failures, mut namespace) = (0, 0, 0, 0, 0);
    let (mut cached, mut candidates) = (0, Vec::new());
    for (block, case) in run(helper, &requests).iter().zip(&expected) {
        let (xml, index, options, chunk, balanced, outcome) = case;
        let theirs = parse_their(block);
        failures += usize::from(theirs.0.is_err());
        first_codes += usize::from(theirs.1 == outcome.1);
        if let (Err(their), Err(our)) = (&theirs.0, &outcome.0) {
            codes += usize::from(their == our);
        }
        if agrees(&theirs, outcome) {
            agree += 1;
        } else if namespace_only(&theirs, outcome) {
            namespace += 1;
        } else if cached_entity(chunk) {
            cached += 1;
        } else {
            candidates.push((case, theirs, whole(xml, *index, *options, chunk, *balanced)));
        }
    }
    // Divergences a whole document shows too are counted, not failed.
    let mut controls = String::new();
    for ((_, _, options, ..), _, whole) in &candidates {
        writeln!(controls, "-\t0\t{options}\t{}\tW\t0", hex(whole.as_bytes())).unwrap();
    }
    let (mut documented, mut mismatches) = (Vec::new(), Vec::new());
    for (block, (case, theirs, whole)) in run(helper, &controls).iter().zip(&candidates) {
        let (xml, index, options, chunk, balanced, outcome) = case;
        let report = format!(
            "context {xml:?} node {index} options {options} balanced {balanced}\nchunk {chunk:?}\n  yaxml:   {:?} first {:?}\n  libxml2: {:?} first {:?}",
            decode(&outcome.0),
            outcome.1,
            decode(&theirs.0),
            theirs.1
        );
        match agrees(&parse_their(block), &ours_whole(whole, *options)) {
            true => mismatches.push(report),
            false => documented.push(report),
        }
    }
    let failed = failures.max(1);
    eprintln!(
        "fragment differential: {cases} chunks, {agree} agree ({:.2}%); documented: {namespace} strict namespace errors, {cached} cached entities, {} as in a whole document; {} mismatches. {codes}/{failures} failures ({:.2}%) return libxml2's code; first error codes agree in {first_codes}/{cases} ({:.2}%)",
        100.0 * agree as f64 / cases as f64,
        documented.len(),
        mismatches.len(),
        100.0 * codes as f64 / failed as f64,
        100.0 * first_codes as f64 / cases as f64,
    );
    for report in documented.iter().take(3) {
        eprintln!("as in a whole document: {report}\n");
    }
    for mismatch in mismatches.iter().take(20) {
        eprintln!("{mismatch}\n");
    }
    assert!(
        mismatches.is_empty(),
        "{} mismatches; first:\n{}",
        mismatches.len(),
        mismatches[0]
    );
}
