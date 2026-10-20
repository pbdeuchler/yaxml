//! Differential test against `tooling/libxml2_domedit.c` (libxml2 2.15.4):
//! `path`, `base_uri`, `lang`, `space_preserve` and `content` on every node,
//! `list_string` (`xmlNodeListGetString`) both ways, and the tree after
//! random move scripts (`xmlAddChild` text merging, sibling insertion,
//! unlink, replace) or copy scripts (`xmlDocCopyNode` from another document
//! or the same one, then `xmlAddChild`; `xmlCopyDoc`), dumped by
//! `xmlDocDumpMemory`. Runs
//! only when `YAXML_DOMEDIT_HELPER` names the compiled helper; see
//! `docs/owned-dom.md`.
#![forbid(unsafe_code)]

#[path = "../examples/support/process.rs"]
mod process;

use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

use process::ProcessLimits;
use yaxml::{EditLimits, OwnedDocument, OwnedNode, OwnedNodeId, OwnedNodeKind, ParseOptions};

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

const DTD: &str =
    "<!DOCTYPE r [<!ENTITY e '<i xml:lang=\"de\" xml:base=\"ent/\">z</i> w'><!ENTITY f 'plain'>]>";

/// A random document; `namespaces` adds prefixed and default namespaces.
fn document(random: &mut Random, namespaces: bool, entities: bool) -> String {
    let mut xml = String::new();
    if entities {
        xml.push_str(DTD);
    }
    if random.next(3) == 0 {
        xml.push_str("<!--lead-->");
    }
    let root_ns = if namespaces {
        random.pick(&[
            " xmlns:p='urn:p' xmlns:q='urn:q'",
            " xmlns='urn:d' xmlns:p='urn:p' xmlns:q='urn:q'",
            " xmlns:p='urn:p' xmlns:q='urn:p'",
        ])
    } else {
        ""
    };
    write!(xml, "<r{root_ns}{}>", attributes(random)).unwrap();
    content(random, &mut xml, 0, namespaces, entities);
    xml.push_str("</r>");
    if random.next(3) == 0 {
        xml.push_str("<?tail x?>");
    }
    xml
}

fn attributes(random: &mut Random) -> String {
    let mut out = String::new();
    if random.next(2) == 0 {
        let base = [
            "sub/",
            "/abs/",
            "http://h/x/",
            "urn:u",
            "../up/",
            "f.xml",
            "",
            "a b/",
        ];
        write!(out, " xml:base='{}'", random.pick(&base)).unwrap();
    }
    if random.next(3) == 0 {
        write!(out, " xml:lang='{}'", random.pick(&["en", "fr-CA", ""])).unwrap();
    }
    if random.next(3) == 0 {
        let space = ["preserve", "default", "bogus"];
        write!(out, " xml:space='{}'", random.pick(&space)).unwrap();
    }
    out
}

fn content(random: &mut Random, xml: &mut String, depth: usize, namespaces: bool, entities: bool) {
    for _ in 0..random.next(5) {
        match random.next(if entities { 9 } else { 8 }) {
            0..=2 if depth < 4 => {
                let names: &[&str] = if namespaces {
                    &["a", "b", "p:a", "p:b", "q:a"]
                } else {
                    &["a", "b", "c"]
                };
                let name = random.pick(names);
                let local = match (namespaces, random.next(6)) {
                    (true, 0) => " xmlns=''",
                    (true, 1) => " xmlns='urn:e'",
                    _ => "",
                };
                write!(xml, "<{name}{local}{}>", attributes(random)).unwrap();
                content(random, xml, depth + 1, namespaces, entities);
                write!(xml, "</{name}>").unwrap();
            }
            3 | 4 => xml.push_str(random.pick(&["x", " ", "\n ", "t&amp;", "y", "\u{e9}&lt;"])),
            5 => xml.push_str("<![CDATA[c]]>"),
            6 => xml.push_str(random.pick(&["<!--k-->", "<!---->"])),
            7 => xml.push_str(random.pick(&["<?p d?>", "<?q?>", "<?p?>"])),
            8 if entities => xml.push_str(random.pick(&["&e;", "&f;"])),
            _ => {}
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(field: &str) -> Option<String> {
    match field {
        "-" => None,
        "0" => Some(String::new()),
        field => {
            let bytes = (0..field.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&field[i..i + 2], 16).unwrap())
                .collect();
            Some(String::from_utf8(bytes).unwrap())
        }
    }
}

/// Applies a random script, keeping the operations yaxml accepts in the
/// helper's notation.
fn script(random: &mut Random, document: &mut OwnedDocument) -> String {
    let ids: Vec<OwnedNodeId> = document.nodes().map(|node| node.id()).collect();
    let mut accepted = Vec::new();
    for _ in 0..random.next(9) {
        let (i, j) = (random.next(ids.len()), random.next(ids.len()));
        let (first, second) = (ids[i], ids[j]);
        let mut editor = document.edit(EditLimits::default()).unwrap();
        let (token, result) = match random.next(6) {
            0 => (
                format!("A,{i},{j}"),
                editor.append_child(first, second).map(drop),
            ),
            1 => (
                format!("B,{i},{j}"),
                editor.insert_before(first, second).map(drop),
            ),
            2 => (
                format!("N,{i},{j}"),
                editor.insert_after(first, second).map(drop),
            ),
            3 => (format!("U,{i}"), editor.unlink(first)),
            4 => (format!("R,{i},{j}"), editor.replace(first, second)),
            _ => {
                let text = random.pick(&["x", " ", "", "z&"]);
                // Create, then move with xmlAddChild semantics; a node that
                // cannot move (entity content) is removed again.
                let created = editor.append_text(first, text).and_then(|node| {
                    let moved = editor.unlink(node);
                    let moved = moved.and_then(|()| editor.append_child(first, node));
                    if moved.is_err() {
                        editor.remove_subtree(node)?;
                    }
                    moved.map(drop)
                });
                (format!("T,{i},{}", hex(text.as_bytes())), created)
            }
        };
        if result.is_ok() {
            accepted.push(token);
        }
    }
    accepted.join(" ")
}

fn in_entity(node: OwnedNode<'_>) -> bool {
    std::iter::successors(node.parent(), |node| node.parent())
        .any(|node| node.kind() == OwnedNodeKind::EntityRef)
}

/// Whether the detached `copy`, about to go under `parent`, would show a
/// documented divergence instead of a bug:
/// - an unnamespaced element relying on the default namespace being unbound
///   gets `xmlns=""` here; libxml2's `xmlAddChild` puts it in the parent's
///   default namespace;
/// - libxml2 parses an entity's content at its first reference, so a copied
///   reference to an entity the document never referenced has none there;
/// - a copy from entity content (`scoped`) keeps the namespaces of its
///   reference's scope; libxml2 parses the entity once, outside any.
fn diverges(
    document: &OwnedDocument,
    copy: OwnedNodeId,
    parent: OwnedNodeId,
    referenced: &[String],
    scoped: bool,
) -> bool {
    let default = document
        .node(parent)
        .and_then(|parent| parent.search_ns(None));
    let default = default.is_some_and(|uri| !uri.is_empty());
    document.node(copy).unwrap().descendants().any(|node| {
        let name = node.name();
        let unnamespaced = name
            .as_ref()
            .is_some_and(|name| name.prefix().is_none() && name.namespace().is_none());
        let undeclared =
            default && unnamespaced && node.search_ns(None).is_none() && !in_entity(node);
        let unparsed = node
            .entity_name()
            .is_some_and(|name| !referenced.iter().any(|seen| seen == name));
        let namespaced = scoped && name.is_some_and(|name| name.namespace().is_some());
        undeclared || unparsed || namespaced
    })
}

/// Applies a random copy script: `import` from `source` or `copy` within,
/// then `append_child`, and sometimes `try_clone` at the end (`D`).
fn copy_script(
    random: &mut Random,
    document: &mut OwnedDocument,
    source: &OwnedDocument,
    skipped: &mut usize,
) -> String {
    let ids: Vec<OwnedNodeId> = document.nodes().map(|node| node.id()).collect();
    let sources: Vec<OwnedNodeId> = source.nodes().map(|node| node.id()).collect();
    let referenced: Vec<String> = document
        .nodes()
        .filter_map(|node| node.entity_name().map(str::to_owned))
        .collect();
    let mut accepted = Vec::new();
    for _ in 0..random.next(7) {
        let (imported, deep) = (random.next(3) != 0, random.next(3) != 0);
        let i = random.next(if imported { sources.len() } else { ids.len() });
        let j = random.next(ids.len());
        let scoped = match imported {
            true => in_entity(source.node(sources[i]).unwrap()),
            false => in_entity(document.node(ids[i]).unwrap()),
        };
        let mut editor = document.edit(EditLimits::default()).unwrap();
        let copy = match imported {
            true => editor.import(source, sources[i], deep),
            false => editor.copy(ids[i], deep),
        };
        let Ok(copy) = copy else {
            continue;
        };
        let diverging = diverges(editor.document(), copy, ids[j], &referenced, scoped);
        *skipped += usize::from(diverging);
        if diverging || editor.append_child(ids[j], copy).is_err() {
            editor.remove_subtree(copy).unwrap();
            continue;
        }
        let kind = if imported { 'C' } else { 'c' };
        accepted.push(format!("{kind},{i},{j},{}", u8::from(deep)));
    }
    if random.next(3) == 0 {
        accepted.push("D".to_owned());
    }
    accepted.join(" ")
}

/// The helper's report for a parsed document: dump, then one line per node.
fn report(document: &OwnedDocument) -> Vec<String> {
    let dump = yaxml::save::to_vec(document, &yaxml::save::Options::default()).unwrap();
    let field = |value: Option<&str>| match value {
        None => "-".to_owned(),
        Some("") => "0".to_owned(),
        Some(value) => hex(value.as_bytes()),
    };
    let mut lines = vec![format!(
        "DOC {}",
        field(Some(std::str::from_utf8(&dump).unwrap()))
    )];
    for node in document.nodes() {
        let space = node.space_preserve().map_or(-1, i32::from);
        lines.push(format!(
            "N {} {} {} {space} {} {} {}",
            field(node.path().as_deref()),
            field(node.base_uri().as_deref()),
            field(node.lang()),
            field(node.content().as_deref()),
            field(Some(&node.list_string(true))),
            field(Some(&node.list_string(false))),
        ));
    }
    lines.push("END".to_owned());
    lines
}

#[test]
fn accessors_and_moves_match_libxml2() {
    let Some(helper) = std::env::var_os("YAXML_DOMEDIT_HELPER") else {
        eprintln!("YAXML_DOMEDIT_HELPER unset; skipping the libxml2 differential");
        return;
    };
    let cases: usize = std::env::var("YAXML_DOMEDIT_CASES")
        .ok()
        .and_then(|cases| cases.parse().ok())
        .unwrap_or(4000);
    let urls = [
        None,
        Some("/doc/main.xml"),
        Some("http://example.com/d/doc.xml"),
    ];
    let seed = std::env::var("YAXML_DOMEDIT_SEED")
        .ok()
        .and_then(|seed| seed.parse().ok());
    let mut random = Random(seed.unwrap_or(0x5eed));
    let (mut requests, mut expected) = (String::new(), Vec::new());
    let (mut operations, mut copied, mut skipped) = (0, 0, 0);
    for case in 0..cases {
        // Namespaced documents take no moves (libxml2 does not reconcile
        // them) but do take copies, which it declares on the copy's root.
        let (moves, copies) = (case % 3 == 1, case % 3 == 2);
        let entities = random.next(3) == 0;
        let namespaces = !moves && (!copies || random.next(2) == 0);
        let xml = document(&mut random, namespaces, entities);
        let url = urls[random.next(urls.len())];
        let options = ParseOptions::default().base(url.map(str::to_owned));
        let mut owned = OwnedDocument::parse_with(xml.as_bytes(), options).unwrap();
        let source_namespaces = random.next(2) == 0;
        let source = copies.then(|| document(&mut random, source_namespaces, entities));
        let ops = match &source {
            Some(source) => {
                let source = OwnedDocument::parse(source).unwrap();
                copy_script(&mut random, &mut owned, &source, &mut skipped)
            }
            None if moves => script(&mut random, &mut owned),
            None => String::new(),
        };
        operations += ops.split(' ').filter(|op| !op.is_empty()).count();
        copied += ops
            .split(' ')
            .filter(|op| op.starts_with(['C', 'c', 'D']))
            .count();
        let url_field = url.map_or("-".to_owned(), |url| hex(url.as_bytes()));
        let source_field = source
            .as_ref()
            .map_or("-".to_owned(), |xml| hex(xml.as_bytes()));
        let xml_field = hex(xml.as_bytes());
        writeln!(requests, "{xml_field}\t{url_field}\t{ops}\t{source_field}").unwrap();
        let report = match ops.ends_with('D') {
            true => report(&owned.try_clone().unwrap()),
            false => report(&owned),
        };
        expected.push((xml, ops, report));
    }
    let output = process::run(
        Path::new(&helper),
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
    let mut lines = stdout.lines();
    let mut mismatches = Vec::new();
    for (xml, ops, report) in &expected {
        let mut theirs = Vec::new();
        for line in lines.by_ref() {
            theirs.push(line.to_owned());
            if line == "END" || line == "ERR" {
                break;
            }
        }
        // Documented divergence: libxml2's document content also descends
        // into the DTD's parsed entity declarations.
        // The content is the sixth field of the document node's line.
        if xml.starts_with("<!DOCTYPE")
            && let Some(document) = theirs.get_mut(1)
        {
            let mut fields: Vec<&str> = document.split(' ').collect();
            fields[5] = report[1].split(' ').nth(5).unwrap();
            *document = fields.join(" ");
        }
        if theirs != *report {
            let decode = |lines: &[String]| -> Vec<String> {
                lines
                    .iter()
                    .map(|line| {
                        line.split(' ')
                            .map(|field| {
                                match field.len() % 2 == 0
                                    && field.bytes().all(|b| b.is_ascii_hexdigit())
                                {
                                    true => format!("{:?}", unhex(field)),
                                    false => field.to_owned(),
                                }
                            })
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .collect()
            };
            // Only the lines that differ, by position.
            let (ours, theirs) = (decode(report), decode(&theirs));
            let differing: Vec<_> = (0..ours.len().max(theirs.len()))
                .filter(|&line| ours.get(line) != theirs.get(line))
                .map(|line| {
                    format!(
                        "line {line}\n  yaxml:   {:?}\n  libxml2: {:?}",
                        ours.get(line),
                        theirs.get(line)
                    )
                })
                .collect();
            mismatches.push(format!("{xml}\nops: {ops}\n{}", differing.join("\n")));
        }
    }
    eprintln!(
        "domedit differential: {cases} documents, {operations} accepted operations ({copied} copies; {skipped} divergent copies not sent), {} mismatches",
        mismatches.len()
    );
    assert!(
        mismatches.is_empty(),
        "{} mismatches; first:\n{}",
        mismatches.len(),
        mismatches[0]
    );
}
