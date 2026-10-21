//! DTD validation throughput: `cargo bench --bench valid`.
//!
//! `tree` times `Document::validate` on a document parsed once; `events`
//! reads the document's `BufferedReader` events and `stream` validates them
//! too, so `stream` minus `events` is the streaming validator's cost.
//! `YAXML_BENCH_ITERATIONS` fixes the iteration count, `YAXML_BENCH_FIXTURE`
//! and `YAXML_BENCH_MODE` select one fixture and mode (for `/usr/bin/time -l`
//! instruction counts).

use std::hint::black_box;
use std::time::Instant;

use yaxml::valid::StreamValidator;
use yaxml::{BufferedReader, Document};

const DTD: &str = "<!DOCTYPE r [\
    <!ELEMENT r (head, (item | note)*)>\
    <!ELEMENT head (#PCDATA)>\
    <!ELEMENT item (name, value?, tag*)>\
    <!ELEMENT note (#PCDATA | em)*>\
    <!ELEMENT name (#PCDATA)><!ELEMENT value (#PCDATA)>\
    <!ELEMENT tag EMPTY><!ELEMENT em (#PCDATA)>\
    <!ATTLIST item id ID #REQUIRED ref IDREF #IMPLIED kind (a|b|c) 'a' n NMTOKENS #IMPLIED>\
    <!ATTLIST tag k CDATA #REQUIRED>]>";

/// Reads `xml`'s events, validating them when `validate`; the diagnostics.
fn stream(xml: &str, validate: bool) -> usize {
    let mut reader = BufferedReader::new(xml.as_bytes());
    let mut validator = StreamValidator::new();
    while let Some((event, origin)) = reader.read_event().unwrap() {
        if validate {
            validator.event(&event, origin);
            if let Some(dtd) = reader.dtd() {
                validator.set_dtd(dtd);
            }
        } else {
            black_box(event);
        }
    }
    validator.finish().len()
}

fn main() {
    let items: String = (0..10_000)
        .map(|i| {
            format!(
                "<item id='i{i}' ref='i{}' kind='b' n='x y'>\n <name>n{i}</name>\n <value>v</value><tag k='t'/><tag k='u'/></item>\
                 <note>some <em>text</em> here</note>\n",
                i / 2
            )
        })
        .collect();
    let fixtures = [
        ("flat", format!("{DTD}<r><head>h</head>{items}</r>")),
        (
            "deep",
            format!(
                "<!DOCTYPE r [<!ELEMENT r (r?)>]>{}{}",
                "<r>".repeat(200),
                "</r>".repeat(200)
            ),
        ),
    ];
    let only = std::env::var("YAXML_BENCH_FIXTURE").ok();
    let mode = std::env::var("YAXML_BENCH_MODE").ok();
    let fixed: Option<usize> = std::env::var("YAXML_BENCH_ITERATIONS")
        .ok()
        .map(|s| s.parse().expect("positive iteration count"));
    for (name, xml) in fixtures {
        if only.as_deref().is_some_and(|only| only != name) {
            continue;
        }
        let document = Document::parse(&xml).unwrap();
        assert!(yaxml::valid::is_valid(&document.validate()), "{name}");
        assert_eq!(stream(&xml, true), 0, "{name}");
        for label in ["tree", "events", "stream"] {
            if mode.as_deref().is_some_and(|mode| mode != label) {
                continue;
            }
            let start = Instant::now();
            let mut iterations = 0;
            let more = |iterations| {
                fixed.map_or(iterations < 3 || start.elapsed().as_secs_f64() < 1.0, |n| {
                    iterations < n
                })
            };
            while more(iterations) {
                match label {
                    "tree" => drop(black_box(black_box(&document).validate())),
                    _ => drop(black_box(stream(black_box(&xml), label == "stream"))),
                }
                iterations += 1;
            }
            let seconds = start.elapsed().as_secs_f64();
            let mb = (xml.len() * iterations) as f64 / 1e6;
            println!("{name:8} {label:7} {:8.1} MB/s", mb / seconds);
        }
    }
}
