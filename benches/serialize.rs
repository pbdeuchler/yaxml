//! Complete-document output through `Writer` sinks, `save::to_vec` and
//! `save::to_writer`; trees are constructed outside timed batches.

use std::hint::black_box;
use std::io::{self, BufWriter, Write};
use std::time::Instant;

use yaxml::{Document, Limits, OwnedDocument, WriteError, Writer, WriterLimits, save};

fn parameter(name: &str) -> Option<String> {
    std::env::var(name).ok()
}
fn number(name: &str, fallback: usize) -> usize {
    parameter(name).map_or(fallback, |value| {
        value.parse().expect("integer benchmark parameter")
    })
}
fn selected(name: &str, value: &str) -> bool {
    parameter(name).is_none_or(|choice| choice == value)
}

#[derive(Default)]
struct Counter {
    bytes: usize,
    writes: usize,
    flushes: usize,
}
impl Write for Counter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes += black_box(bytes).len();
        self.writes += 1;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.flushes += 1;
        Ok(())
    }
}

enum Sink {
    Counting(Counter),
    Buffered(BufWriter<Counter>),
}
impl Sink {
    fn new(buffered: bool) -> Self {
        if buffered {
            Self::Buffered(BufWriter::with_capacity(8192, Counter::default()))
        } else {
            Self::Counting(Counter::default())
        }
    }
    fn counter(&self) -> &Counter {
        match self {
            Self::Counting(counter) => counter,
            Self::Buffered(buffer) => {
                assert!(
                    buffer.buffer().is_empty(),
                    "document method did not flush buffered output"
                );
                buffer.get_ref()
            }
        }
    }
}
impl Write for Sink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match self {
            Self::Counting(sink) => sink.write(bytes),
            Self::Buffered(sink) => sink.write(bytes),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Counting(sink) => sink.flush(),
            Self::Buffered(sink) => sink.flush(),
        }
    }
}

#[derive(Clone, Copy)]
enum Tree<'a, 'input> {
    Borrowed(&'a Document<'input>),
    Owned(&'a OwnedDocument),
}
impl Tree<'_, '_> {
    fn write(self, writer: &mut Writer<Sink>) -> Result<(), WriteError> {
        match self {
            Self::Borrowed(tree) => writer.write_document(tree),
            Self::Owned(tree) => writer.write_owned_document(tree),
        }
    }
    fn save(self) -> Vec<u8> {
        let options = save::Options::default();
        match self {
            Self::Borrowed(tree) => save::to_vec(tree, &options),
            Self::Owned(tree) => save::to_vec(tree, &options),
        }
        .expect("saved")
    }
    /// `save::to_writer` into a counting sink: chunked output, no Vec.
    fn stream(self, sink: &mut Counter) {
        let options = save::Options::default();
        match self {
            Self::Borrowed(tree) => save::to_writer(tree, &options, sink),
            Self::Owned(tree) => save::to_writer(tree, &options, sink),
        }
        .expect("saved")
    }
    fn label(self) -> &'static str {
        match self {
            Self::Borrowed(_) => "borrowed",
            Self::Owned(_) => "owned",
        }
    }
}

struct Sample<'a> {
    shape: &'a str,
    siblings: usize,
    depth: usize,
    input_bytes: usize,
    limits: WriterLimits,
}

fn measure(tree: Tree<'_, '_>, sample: &Sample<'_>) {
    for sink in ["counting", "buffered", "save", "stream"] {
        if !selected("YAXML_SERIALIZE_SINK", sink) {
            continue;
        }
        if sink == "save" || sink == "stream" {
            measure_save(tree, sample, sink);
            continue;
        }
        let iterations = number(
            "YAXML_SERIALIZE_ITERATIONS",
            if cfg!(debug_assertions) {
                1
            } else {
                (1_048_576 / sample.input_bytes).clamp(1, 100)
            },
        );
        assert!(iterations > 0);
        let mut bytes = 0;
        let mut writes = 0;
        let mut flushes = 0;
        let mut expected = None;
        let mut rejected = None;
        let started = Instant::now();
        for _ in 0..iterations {
            let mut writer = Writer::with_limits(Sink::new(sink == "buffered"), sample.limits);
            if let Err(error) = tree.write(&mut writer) {
                rejected = Some(error);
                break;
            }
            let position = writer.position();
            let sink = writer.into_inner();
            let count = sink.counter();
            assert_eq!(position, count.bytes);
            assert_eq!(count.flushes, 1);
            assert_eq!(
                *expected.get_or_insert(position),
                position,
                "serialization output length changed between iterations"
            );
            bytes += position;
            writes += count.writes;
            flushes += count.flushes;
            black_box(sink);
        }
        let elapsed = started.elapsed();
        if let Some(error) = rejected {
            println!(
                "shape={} tree={} sink={sink} siblings={} depth={} input_bytes={} status=rejected error={error}",
                sample.shape,
                tree.label(),
                sample.siblings,
                sample.depth,
                sample.input_bytes
            );
        } else {
            println!(
                "shape={} tree={} sink={sink} siblings={} depth={} input_bytes={} status=complete iterations={iterations} output_bytes_total={bytes} sink_writes={writes} sink_flushes={flushes} elapsed_ns={} output_mib_per_s={:.2}",
                sample.shape,
                tree.label(),
                sample.siblings,
                sample.depth,
                sample.input_bytes,
                elapsed.as_nanos(),
                bytes as f64 / elapsed.as_secs_f64() / 1_048_576.0
            );
        }
    }
}

/// `save::to_vec` into a fresh vector per iteration (`save`), or
/// `save::to_writer` into a counting sink (`stream`).
fn measure_save(tree: Tree<'_, '_>, sample: &Sample<'_>, sink: &str) {
    let iterations = number(
        "YAXML_SERIALIZE_ITERATIONS",
        if cfg!(debug_assertions) {
            1
        } else {
            (1_048_576 / sample.input_bytes).clamp(1, 100)
        },
    );
    let mut bytes = 0;
    let started = Instant::now();
    for _ in 0..iterations {
        bytes += if sink == "save" {
            let output = tree.save();
            black_box(&output).len()
        } else {
            let mut counter = Counter::default();
            tree.stream(&mut counter);
            black_box(counter).bytes
        };
    }
    let elapsed = started.elapsed();
    println!(
        "shape={} tree={} sink={sink} siblings={} depth={} input_bytes={} status=complete iterations={iterations} output_bytes_total={bytes} elapsed_ns={} output_mib_per_s={:.2}",
        sample.shape,
        tree.label(),
        sample.siblings,
        sample.depth,
        sample.input_bytes,
        elapsed.as_nanos(),
        bytes as f64 / elapsed.as_secs_f64() / 1_048_576.0
    );
}

fn main() {
    for (name, choices) in [
        (
            "YAXML_SERIALIZE_SHAPE",
            &["flat", "text", "normalized", "lexical", "namespaces"][..],
        ),
        ("YAXML_SERIALIZE_TREE", &["borrowed", "owned"][..]),
        (
            "YAXML_SERIALIZE_SINK",
            &["counting", "buffered", "save", "stream"][..],
        ),
    ] {
        if let Some(choice) = parameter(name) {
            assert!(
                choices.contains(&choice.as_str()),
                "unknown serialization benchmark selector"
            );
        }
    }
    let sizes = if let Some(value) = parameter("YAXML_SERIALIZE_SIBLINGS") {
        vec![value.parse().expect("integer sibling count")]
    } else if cfg!(debug_assertions) {
        vec![1, 16]
    } else {
        vec![1, 256, 4096]
    };
    let depth = number("YAXML_SERIALIZE_DEPTH", 1);
    let attributes = number("YAXML_SERIALIZE_ATTRIBUTES", 1);
    let payload = number("YAXML_SERIALIZE_PAYLOAD_BYTES", 32);
    let uri = number("YAXML_SERIALIZE_URI_BYTES", 16);
    assert!(depth > 0);
    for shape in ["flat", "text", "normalized", "lexical", "namespaces"] {
        if !selected("YAXML_SERIALIZE_SHAPE", shape) {
            continue;
        }
        let item = match shape {
            "flat" => "<n a='v'/>".to_owned(),
            "text" => format!("<n>{}</n>", "x".repeat(payload)),
            "normalized" => format!("<n a='&#9;&#10;&#13;&quot;'>{}</n>", ">".repeat(payload)),
            "lexical" => {
                let text = "x".repeat(payload);
                format!("<n><!--{text}--><![CDATA[{text}]]><?p {text}?></n>")
            }
            "namespaces" => {
                let mut text = format!("<p:n xmlns:p='urn:{}'", "u".repeat(uri));
                for attribute in 0..attributes {
                    text.push_str(&format!(" p:a{attribute}='v'"));
                }
                text.push_str("/>");
                text
            }
            _ => unreachable!(),
        };
        for siblings in &sizes {
            assert!(*siblings > 0);
            let input = format!(
                "{}{}{}",
                "<r>".repeat(depth),
                item.repeat(*siblings),
                "</r>".repeat(depth)
            );
            let parse = Limits::default()
                .max_depth(depth + 1)
                .max_attributes(attributes.saturating_add(1).max(1024));
            let sample = Sample {
                shape,
                siblings: *siblings,
                depth,
                input_bytes: input.len(),
                limits: WriterLimits::default()
                    .max_depth(parse.max_depth)
                    .max_attributes(parse.max_attributes),
            };
            if selected("YAXML_SERIALIZE_TREE", "borrowed") {
                match Document::with_limits(input.as_bytes(), parse) {
                    Ok(document) => measure(Tree::Borrowed(&document), &sample),
                    Err(error) => println!(
                        "shape={shape} tree=borrowed siblings={siblings} depth={depth} status=parse_rejected error={error}"
                    ),
                }
            }
            if selected("YAXML_SERIALIZE_TREE", "owned") {
                match OwnedDocument::with_limits(input.as_bytes(), parse) {
                    Ok(document) => measure(Tree::Owned(&document), &sample),
                    Err(error) => println!(
                        "shape={shape} tree=owned siblings={siblings} depth={depth} status=parse_rejected error={error}"
                    ),
                }
            }
        }
    }
}
