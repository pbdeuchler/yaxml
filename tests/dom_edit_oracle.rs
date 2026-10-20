//! Differential test against `tooling/libxml2_domedit.c` (libxml2 2.15.4):
//! `path`, `base_uri`, `lang`, `space_preserve` and `content` on every node,
//! and the tree after random move scripts (`xmlAddChild` text merging,
//! sibling insertion, unlink, replace), dumped by `xmlDocDumpMemory`. Runs
//! only when `YAXML_DOMEDIT_HELPER` names the compiled helper; see
//! `docs/owned-dom.md`.
#![forbid(unsafe_code)]

#[path = "../examples/support/process.rs"]
mod process;

use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

use process::ProcessLimits;
use yaxml::{EditLimits, OwnedDocument, OwnedNodeId, ParseOptions};

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
            3 | 4 => xml.push_str(random.pick(&["x", " ", "\n ", "t&amp;", "y"])),
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
            "N {} {} {} {space} {}",
            field(node.path().as_deref()),
            field(node.base_uri().as_deref()),
            field(node.lang()),
            field(node.content().as_deref()),
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
    let mut operations = 0;
    for case in 0..cases {
        let edits = case % 2 == 1;
        let entities = random.next(3) == 0;
        let xml = document(&mut random, !edits, entities);
        let url = urls[random.next(urls.len())];
        let options = ParseOptions {
            base: url.map(str::to_owned),
            ..ParseOptions::default()
        };
        let mut owned = OwnedDocument::parse_with(xml.as_bytes(), options).unwrap();
        let ops = if edits {
            script(&mut random, &mut owned)
        } else {
            String::new()
        };
        operations += ops.split(' ').filter(|op| !op.is_empty()).count();
        let url_field = url.map_or("-".to_owned(), |url| hex(url.as_bytes()));
        writeln!(requests, "{}\t{url_field}\t{ops}", hex(xml.as_bytes())).unwrap();
        expected.push((xml, ops, report(&owned)));
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
        if xml.starts_with("<!DOCTYPE")
            && let Some(document) = theirs.get_mut(1)
            && let Some((fields, _)) = document.rsplit_once(' ')
        {
            *document = format!("{fields} {}", report[1].rsplit_once(' ').unwrap().1);
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
            mismatches.push(format!(
                "{xml}\nops: {ops}\nyaxml:   {:#?}\nlibxml2: {:#?}",
                decode(report),
                decode(&theirs)
            ));
        }
    }
    eprintln!(
        "domedit differential: {cases} documents, {operations} accepted operations, {} mismatches",
        mismatches.len()
    );
    assert!(
        mismatches.is_empty(),
        "{} mismatches; first:\n{}",
        mismatches.len(),
        mismatches[0]
    );
}
