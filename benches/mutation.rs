//! Setup, mutation and final validation are timed separately. Every iteration
//! reparses a fresh tree before the mutation timer, since edits spend capacity.
//! The debug defaults form the all-target smoke run; release defaults are a
//! workload matrix, not a comparison with libxml2 or an allocation benchmark.
//!
//! Select rows with YAXML_MUTATION_CASE / ENCODING. WIDTH / DEPTH select one
//! positive size; ATTRIBUTES controls the attribute fixtures. ITERATIONS and
//! OPERATIONS are positive batch counts. PAYLOAD_BYTES is the ordinary/short
//! value size; GROWTH_BYTES is the growth case's large value. MAX_VALUE_BYTES
//! and MAX_OWNED_BYTES select EditLimits, including zero for rejection studies.
//! Every parameter above has the YAXML_MUTATION_ prefix.

use std::hint::black_box;
use std::time::{Duration, Instant};

use yaxml::{
    EditError, EditLimits, Limits, OwnedDocument, OwnedNodeId, OwnedNodeKind, Reader, Writer,
    WriterLimits,
};

const CASES: &[&str] = &[
    "same_leaf",
    "many_leaves",
    "attribute_first",
    "attribute_middle",
    "attribute_last",
    "growth_shrink",
];
const ENCODINGS: &[&str] = &["utf8", "utf16le", "utf16be"];

fn parameter(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

fn number(name: &str, fallback: usize) -> usize {
    parameter(name).map_or(fallback, |value| {
        value.parse().expect("integer benchmark parameter")
    })
}

fn numbers(name: &str, defaults: &[usize]) -> Vec<usize> {
    parameter(name).map_or_else(
        || defaults.to_vec(),
        |value| vec![value.parse().expect("integer benchmark parameter")],
    )
}

fn selected(name: &str, value: &str) -> bool {
    parameter(name).is_none_or(|choice| choice == value)
}

fn encode(xml: &str, encoding: &str) -> Vec<u8> {
    if encoding == "utf8" {
        return xml.as_bytes().to_vec();
    }
    let little = encoding == "utf16le";
    let mut bytes = if little {
        vec![0xff, 0xfe]
    } else {
        vec![0xfe, 0xff]
    };
    for unit in xml.encode_utf16() {
        bytes.extend_from_slice(&if little {
            unit.to_le_bytes()
        } else {
            unit.to_be_bytes()
        });
    }
    bytes
}

struct Case {
    label: &'static str,
    width: usize,
    depth: usize,
    attributes: usize,
    attribute: Option<usize>,
    payload: String,
    alternate: String,
}

impl Case {
    fn xml(&self) -> String {
        let mut item = String::from("<é");
        for index in 0..self.attributes {
            item.push_str(&format!(" a{index:04}='v'"));
        }
        item.push_str(">x</é>");
        format!(
            "{}{}{}",
            "<r>".repeat(self.depth),
            item.repeat(self.width),
            "</r>".repeat(self.depth)
        )
    }

    fn value(&self, operation: usize) -> &str {
        if operation.is_multiple_of(2) {
            &self.payload
        } else {
            &self.alternate
        }
    }

    fn check(&self, document: &OwnedDocument, leaves: &[OwnedNodeId], completed: usize) {
        assert_eq!(leaves.len(), self.width);
        assert_eq!(document.nodes().len(), self.depth + self.width * 2 + 1);
        for (index, id) in leaves.iter().copied().enumerate() {
            let last = if self.attribute.is_some() {
                None
            } else if self.label == "many_leaves" {
                (index < completed)
                    .then(|| index + (completed - 1 - index) / leaves.len() * leaves.len())
            } else {
                (index == 0 && completed > 0).then(|| completed - 1)
            };
            let expected = last.map_or("x", |operation| self.value(operation));
            assert_eq!(document.node(id).unwrap().text(), Some(expected));
            let element = document.node(id).unwrap().parent().unwrap();
            assert_eq!(element.attributes().len(), self.attributes);
            for (attribute_index, attribute) in element.attributes().enumerate() {
                assert_eq!(attribute.name.qualified(), format!("a{attribute_index:04}"));
                let expected =
                    if index == 0 && self.attribute == Some(attribute_index) && completed > 0 {
                        self.value(completed - 1)
                    } else {
                        "v"
                    };
                assert_eq!(attribute.value, expected);
            }
        }
    }
}

struct Config {
    iterations: usize,
    operations: usize,
    edit: EditLimits,
}

fn measure(case: &Case, encoding: &str, config: &Config) {
    // Fixture generation and transport encoding are outside all three phases.
    let input = encode(&case.xml(), encoding);
    let depth = case.depth.checked_add(1).expect("depth overflow");
    let parse = Limits::default()
        .max_depth(depth)
        .max_attributes(case.attributes.max(1024));
    let output = WriterLimits::default()
        .max_depth(depth)
        .max_attributes(parse.max_attributes);
    let selector = case.attribute.map(|index| format!("a{index:04}"));
    let mut setup = Duration::ZERO;
    let mut elapsed = Duration::ZERO;
    let mut validation = Duration::ZERO;
    let mut destruction = Duration::ZERO;
    let mut completed_total = 0_u128;
    let mut output_total = 0_u128;
    let mut checksum = 0xcbf29ce484222325_u64;
    let mut disposition: Option<(usize, Option<EditError>)> = None;
    for _ in 0..config.iterations {
        let started = Instant::now();
        let mut document = match OwnedDocument::with_limits(black_box(&input), parse) {
            Ok(document) => document,
            Err(error) => {
                println!(
                    "case={} encoding={encoding} width={} depth={} attributes={} input_bytes={} status=parse_rejected error={error}",
                    case.label,
                    case.width,
                    case.depth,
                    case.attributes,
                    input.len()
                );
                return;
            }
        };
        let leaves: Vec<_> = document
            .nodes()
            .filter(|node| node.kind() == OwnedNodeKind::Text)
            .map(|node| node.id())
            .collect();
        assert_eq!(leaves.len(), case.width);
        let element = document.node(leaves[0]).unwrap().parent().unwrap().id();
        setup += started.elapsed();

        let started = Instant::now();
        let mut completed = 0;
        let mut failure = None;
        {
            let mut editor = document.edit(config.edit).unwrap();
            for operation in 0..config.operations {
                let value = black_box(case.value(operation));
                let result = if let Some(local) = &selector {
                    editor.set_attribute_value(black_box(element), None, black_box(local), value)
                } else {
                    let index = if case.label == "many_leaves" {
                        operation % leaves.len()
                    } else {
                        0
                    };
                    editor.set_node_value(black_box(leaves[index]), value)
                };
                if let Err(error) = black_box(result) {
                    failure = Some(error);
                    break;
                }
                completed += 1;
            }
        }
        elapsed += started.elapsed();
        completed_total += completed as u128;
        let current = (completed, failure);
        assert_eq!(
            *disposition.get_or_insert(current),
            current,
            "iteration dispositions differ"
        );

        let started = Instant::now();
        case.check(&document, &leaves, completed);
        let mut writer = Writer::with_limits(Vec::new(), output);
        writer
            .write_owned_document(&document)
            .expect("edited tree must serialize");
        let bytes = writer.into_inner();
        let mut reader = Reader::with_limits(&bytes, parse).expect("serialized tree must parse");
        while let Some(event) = reader.read_event().expect("serialized tree must finish") {
            black_box(event);
        }
        assert_eq!(reader.position(), bytes.len());
        for byte in &bytes {
            checksum = (checksum ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
        }
        output_total += bytes.len() as u128;
        black_box((&document, checksum));
        validation += started.elapsed();
        // This phase also drops validation buffers and the selected ID vector.
        let started = Instant::now();
        drop(reader);
        drop(bytes);
        drop(leaves);
        drop(document);
        destruction += started.elapsed();
    }
    let (completed, failure) = disposition.unwrap();
    let status = if failure.is_some() {
        "edit_rejected"
    } else {
        "complete"
    };
    println!(
        "case={} encoding={encoding} width={} depth={} attributes={} input_bytes={} payload_bytes={} alternate_bytes={} max_value_bytes={} max_owned_bytes={} iterations={} requested_ops_per_iteration={} completed_ops_per_iteration={completed} completed_ops_total={completed_total} status={status} error={:?} setup_ns={} elapsed_ns={} validation_ns={} destruction_ns={} output_bytes_total={output_total} checksum={checksum:016x} ops_per_s={:.2}",
        case.label,
        case.width,
        case.depth,
        case.attributes,
        input.len(),
        case.payload.len(),
        case.alternate.len(),
        config.edit.max_value_bytes,
        config.edit.max_owned_bytes,
        config.iterations,
        config.operations,
        failure.map(|error| error.kind),
        setup.as_nanos(),
        elapsed.as_nanos(),
        validation.as_nanos(),
        destruction.as_nanos(),
        completed_total as f64 / elapsed.as_secs_f64()
    );
}

fn main() {
    for (name, choices) in [
        ("YAXML_MUTATION_CASE", CASES),
        ("YAXML_MUTATION_ENCODING", ENCODINGS),
    ] {
        if let Some(choice) = parameter(name) {
            assert!(
                choices.contains(&choice.as_str()),
                "unknown mutation benchmark selector"
            );
        }
    }
    let debug = cfg!(debug_assertions);
    let widths = numbers(
        "YAXML_MUTATION_WIDTH",
        if debug { &[1, 16] } else { &[1, 256] },
    );
    let depths = numbers(
        "YAXML_MUTATION_DEPTH",
        if debug { &[1, 4] } else { &[1, 64] },
    );
    let attributes = number("YAXML_MUTATION_ATTRIBUTES", if debug { 4 } else { 32 });
    let payload = number("YAXML_MUTATION_PAYLOAD_BYTES", 32);
    let growth = number("YAXML_MUTATION_GROWTH_BYTES", 1024);
    let config = Config {
        iterations: number("YAXML_MUTATION_ITERATIONS", if debug { 1 } else { 5 }),
        operations: number("YAXML_MUTATION_OPERATIONS", if debug { 32 } else { 10_000 }),
        edit: EditLimits::default()
            .max_value_bytes(number(
                "YAXML_MUTATION_MAX_VALUE_BYTES",
                EditLimits::default().max_value_bytes,
            ))
            .max_owned_bytes(number(
                "YAXML_MUTATION_MAX_OWNED_BYTES",
                EditLimits::default().max_owned_bytes,
            )),
    };
    assert!(config.iterations > 0 && config.operations > 0 && attributes > 0);
    if selected("YAXML_MUTATION_CASE", "growth_shrink") {
        assert!(
            growth >= payload,
            "growth payload must be at least the short payload"
        );
    }
    for label in CASES {
        if !selected("YAXML_MUTATION_CASE", label) {
            continue;
        }
        let attribute = match *label {
            "attribute_first" => Some(0),
            "attribute_middle" => Some(attributes / 2),
            "attribute_last" => Some(attributes - 1),
            _ => None,
        };
        for width in &widths {
            assert!(*width > 0);
            for depth in &depths {
                assert!(*depth > 0);
                let case = Case {
                    label,
                    width: *width,
                    depth: *depth,
                    attributes: if attribute.is_some() { attributes } else { 0 },
                    attribute,
                    payload: "a".repeat(if *label == "growth_shrink" {
                        growth
                    } else {
                        payload
                    }),
                    alternate: "b".repeat(payload),
                };
                for encoding in ENCODINGS {
                    if selected("YAXML_MUTATION_ENCODING", encoding) {
                        measure(&case, encoding, &config);
                    }
                }
            }
        }
    }
}
