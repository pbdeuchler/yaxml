//! Randomized differential testing against libxml2: random schemas built
//! from a small grammar and random instances over the same names, compared
//! by their exact error codes through the oracle helper.
//!
//! One libxml2 quirk is tolerated: its epsilon reduction lets a counted
//! repetition inside a nullable loop, such as `(x{2,3})*`, end below its
//! `minOccurs`, so libxml2 misses "Missing child element(s)" (1871) errors
//! that yaxml reports.
//!
//! ```sh
//! YAXML_XSD_HELPER=target/xsd-helper YAXML_XSD_CASES=5000 \
//!   cargo test --release --test xsd_random -- --ignored --nocapture
//! ```
//!
//! With `YAXML_XSD_STREAM` set, libxml2 validates with
//! `xmlSchemaValidateStream`; yaxml's stream validation always equals its
//! tree validation here.

use std::fmt::Write as _;
use std::io::Write;
use std::process::{Command, Stdio};

use yaxml::OwnedDocument;
use yaxml::xsd::{self, Schema};

/// xorshift64*.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }
}

const NAMES: [&str; 4] = ["a", "b", "c", "d"];
const SIMPLE: [&str; 9] = [
    "xs:string",
    "xs:int",
    "xs:boolean",
    "xs:date",
    "xs:token",
    "xs:decimal",
    "xs:NMTOKENS",
    "s1",
    "s2",
];
const VALUES: [&str; 12] = [
    "1",
    "x",
    "true",
    "2001-01-01",
    " a b ",
    "",
    "-5",
    "1.50",
    "abc",
    "0",
    "q:n",
    "9999999999",
];

fn occurs(rng: &mut Rng) -> String {
    let (min, max) = match rng.below(9) {
        0 => ("0", "1"),
        1 => ("0", "unbounded"),
        2 => ("1", "unbounded"),
        3 => ("2", "3"),
        4 => ("0", "2"),
        5 => ("2", "unbounded"),
        6 => ("1", "2"),
        7 => ("3", "3"),
        _ => ("1", "1"),
    };
    format!(" minOccurs='{min}' maxOccurs='{max}'")
}

fn particle(rng: &mut Rng, depth: usize, out: &mut String) {
    match rng.below(if depth > 2 { 3 } else { 6 }) {
        0 | 1 => {
            let name = rng.pick(&NAMES);
            let ty = rng.pick(&SIMPLE);
            let _ = write!(
                out,
                "<xs:element name='{name}' type='{ty}'{}/>",
                occurs(rng)
            );
        }
        2 => {
            let name = rng.pick(&NAMES);
            let _ = write!(out, "<xs:element ref='g{name}'{}/>", occurs(rng));
        }
        3 => {
            let ns = rng.pick(&["##any", "##other", "##local", "urn:x"]);
            let pc = rng.pick(&["lax", "skip", "strict"]);
            let _ = write!(
                out,
                "<xs:any namespace='{ns}' processContents='{pc}'{}/>",
                occurs(rng)
            );
        }
        _ => {
            let compositor = rng.pick(&["sequence", "choice"]);
            let _ = write!(out, "<xs:{compositor}{}>", occurs(rng));
            for _ in 0..1 + rng.below(3) {
                particle(rng, depth + 1, out);
            }
            let _ = write!(out, "</xs:{compositor}>");
        }
    }
}

fn attributes(rng: &mut Rng, out: &mut String) {
    for name in ["p", "q"] {
        if rng.chance(40) {
            let ty = rng.pick(&SIMPLE);
            let extra = match rng.below(5) {
                0 => " use='required'",
                1 => " default='1'",
                2 => " fixed='1'",
                _ => "",
            };
            let _ = write!(out, "<xs:attribute name='{name}' type='{ty}'{extra}/>");
        }
    }
    if rng.chance(25) {
        let pc = rng.pick(&["lax", "skip", "strict"]);
        let _ = write!(out, "<xs:anyAttribute processContents='{pc}'/>");
    }
}

fn complex_type(rng: &mut Rng, out: &mut String) {
    let mixed = if rng.chance(20) { " mixed='true'" } else { "" };
    let _ = write!(out, "<xs:complexType{mixed}>");
    match rng.below(10) {
        0 => {
            let _ = write!(out, "<xs:all>");
            for name in NAMES.iter().take(1 + rng.below(3)) {
                let min = if rng.chance(50) { "0" } else { "1" };
                let _ = write!(
                    out,
                    "<xs:element name='{name}' type='xs:int' minOccurs='{min}'/>"
                );
            }
            let _ = write!(out, "</xs:all>");
        }
        1 => {
            let _ = write!(
                out,
                "<xs:simpleContent><xs:extension base='{}'>",
                rng.pick(&SIMPLE)
            );
            attributes(rng, out);
            let _ = write!(out, "</xs:extension></xs:simpleContent>");
            let _ = write!(out, "</xs:complexType>");
            return;
        }
        2 => {}
        _ => {
            let compositor = rng.pick(&["sequence", "choice", "sequence"]);
            let _ = write!(out, "<xs:{compositor}{}>", occurs(rng));
            for _ in 0..1 + rng.below(3) {
                particle(rng, 0, out);
            }
            let _ = write!(out, "</xs:{compositor}>");
        }
    }
    attributes(rng, out);
    let _ = write!(out, "</xs:complexType>");
}

fn schema(rng: &mut Rng) -> String {
    let mut out = String::from("<xs:schema xmlns:xs='http://www.w3.org/2001/XMLSchema'>");
    let facets = [
        "<xs:maxInclusive value='5'/>",
        "<xs:minLength value='2'/>",
        "<xs:pattern value='[0-9a-z]+'/>",
        "<xs:enumeration value='1'/><xs:enumeration value='x'/>",
        "<xs:length value='3'/>",
    ];
    let base = rng.pick(&["xs:int", "xs:string", "xs:token"]);
    let facet = facets[rng.below(facets.len())];
    let _ = write!(
        out,
        "<xs:simpleType name='s1'><xs:restriction base='{base}'>{facet}</xs:restriction></xs:simpleType>"
    );
    let _ = match rng.below(2) {
        0 => write!(
            out,
            "<xs:simpleType name='s2'><xs:list itemType='xs:int'/></xs:simpleType>"
        ),
        _ => write!(
            out,
            "<xs:simpleType name='s2'><xs:union memberTypes='xs:int xs:boolean'/></xs:simpleType>"
        ),
    };
    for name in NAMES {
        let _ = write!(out, "<xs:element name='g{name}'");
        if rng.chance(50) {
            let _ = write!(out, " type='{}'/>", rng.pick(&SIMPLE));
        } else {
            out.push('>');
            complex_type(rng, &mut out);
            out.push_str("</xs:element>");
        }
    }
    out.push_str("<xs:element name='r'>");
    complex_type(rng, &mut out);
    if rng.chance(20) {
        out.push_str(
            "<xs:unique name='u'><xs:selector xpath='.//*'/><xs:field xpath='@p'/></xs:unique>",
        );
    }
    out.push_str("</xs:element></xs:schema>");
    out
}

fn element(rng: &mut Rng, depth: usize, out: &mut String) {
    let local = rng.pick(&NAMES);
    let name = match rng.below(4) {
        0 => format!("g{local}"),
        _ => local.to_owned(),
    };
    let _ = write!(out, "<{name}");
    for attr in ["p", "q", "z"] {
        if rng.chance(25) {
            let _ = write!(out, " {attr}='{}'", rng.pick(&VALUES));
        }
    }
    if rng.chance(5) {
        out.push_str(" xmlns:xsi='http://www.w3.org/2001/XMLSchema-instance' xsi:nil='true'");
    }
    out.push('>');
    match rng.below(if depth > 2 { 1 } else { 3 }) {
        0 => out.push_str(rng.pick(&VALUES)),
        _ => {
            for _ in 0..rng.below(5) {
                element(rng, depth + 1, out);
            }
        }
    }
    let _ = write!(out, "</{name}>");
}

fn instance(rng: &mut Rng) -> String {
    let mut out = String::from("<r>");
    for _ in 0..rng.below(6) {
        element(rng, 0, &mut out);
    }
    out.push_str("</r>");
    out
}

fn project<Id>(diagnostics: &[xsd::Diagnostic<Id>]) -> Vec<(xsd::Code, Option<usize>, String)> {
    let project = |d: &xsd::Diagnostic<Id>| (d.code, d.offset, d.message.clone());
    diagnostics.iter().map(project).collect()
}

fn ours(schema: &str, instance: &str) -> String {
    match Schema::parse(schema.as_bytes(), "schema.xsd") {
        Err(error) => {
            let codes: Vec<String> = error
                .diagnostics
                .iter()
                .filter(|d| !d.code.is_warning())
                .map(|d| (d.code as u16).to_string())
                .collect();
            format!("S {}", codes.join(" ")).trim_end().to_owned()
        }
        Ok(schema) => {
            let document = OwnedDocument::parse(instance).unwrap();
            let diagnostics = schema.validate(&document);
            let streamed = schema.validate_stream(instance.as_bytes(), Default::default());
            assert_eq!(project(&diagnostics), project(&streamed.unwrap()));
            let codes: Vec<String> = diagnostics
                .iter()
                .filter(|d| !d.code.is_warning())
                .map(|d| (d.code as u16).to_string())
                .collect();
            format!("V {}", codes.join(" ")).trim_end().to_owned()
        }
    }
}

fn hex(text: &str) -> String {
    text.bytes().map(|b| format!("{b:02x}")).collect()
}

#[test]
#[ignore = "requires YAXML_XSD_HELPER pointing at the compiled harness/tooling/libxml2_xsd.c"]
fn random_schemas_agree_with_libxml2() {
    let helper = std::env::var("YAXML_XSD_HELPER").unwrap();
    let cases: usize = std::env::var("YAXML_XSD_CASES")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(2000);
    let seed: u64 = std::env::var("YAXML_XSD_SEED")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(0x9e37_79b9_7f4a_7c15);
    let mut rng = Rng(seed);
    let pairs: Vec<(String, String)> = (0..cases)
        .map(|_| (schema(&mut rng), instance(&mut rng)))
        .collect();
    let mut child = Command::new(helper)
        // libxml2's stream validation agrees too: no IDs or ENTITY values here.
        .args(std::env::var_os("YAXML_XSD_STREAM").map(|_| "stream"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let input: String = pairs
        .iter()
        .map(|(s, i)| format!("{}\t{}\n", hex(s), hex(i)))
        .collect();
    let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()).unwrap());
    let output = String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap();
    writer.join().unwrap();
    let (mut differences, mut quirks) = (0, 0);
    let (mut valid, mut schema_errors) = (0, 0);
    for ((schema, instance), line) in pairs.iter().zip(output.lines()) {
        let theirs = match line.strip_prefix("V ") {
            Some(rest) => {
                let mut parts = rest.split(' ');
                parts.next();
                format!("V {}", parts.collect::<Vec<_>>().join(" "))
                    .trim_end()
                    .to_owned()
            }
            None => line.to_owned(),
        };
        valid += usize::from(theirs == "V");
        schema_errors += usize::from(theirs.starts_with('S'));
        let mine = ours(schema, instance);
        let missing = |codes: &str| codes.split(' ').filter(|&c| c == "1871").count();
        let without = |codes: &str| {
            codes
                .split(' ')
                .filter(|&c| c != "1871")
                .collect::<Vec<_>>()
                .join(" ")
        };
        if mine != theirs && without(&mine) == without(&theirs) && missing(&mine) > missing(&theirs)
        {
            quirks += 1;
        } else if mine != theirs {
            differences += 1;
            if differences <= 8 {
                println!("DIFF libxml2 [{theirs}] yaxml [{mine}]\n  {schema}\n  {instance}");
            }
        }
    }
    println!(
        "{differences} differences, {quirks} counted-loop quirks in {cases} cases ({valid} valid, {schema_errors} schema errors)"
    );
    assert_eq!(differences, 0);
}
