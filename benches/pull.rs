//! Observable pull latency/throughput, suitable for one-sample RSS subprocesses.

use std::hint::black_box;
use std::io::{BufReader, Cursor};
use std::time::Instant;

use yaxml::{BufferedReader, Event};

fn parameter(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

fn numbers(name: &str, defaults: &[usize]) -> Vec<usize> {
    match parameter(name) {
        Some(value) => vec![
            value
                .parse()
                .expect("benchmark parameter must be an integer"),
        ],
        None => defaults.to_vec(),
    }
}

fn observe(event: Event<'_>) -> usize {
    let name = |name: yaxml::Name<'_>| {
        black_box(name.qualified);
        black_box(name.prefix);
        black_box(name.local);
        black_box(name.namespace.as_deref());
        name.qualified.len() + name.namespace.as_deref().map_or(0, str::len)
    };
    match event {
        Event::Start(element) | Event::Empty(element) => {
            let mut total = name(element.name);
            for attribute in element.attributes {
                total += name(attribute.name) + black_box(attribute.value.as_bytes()).len();
            }
            total
        }
        Event::End(value) => name(value),
        Event::Declaration(value) => {
            black_box(value);
            0
        }
        Event::Text(value) | Event::Comment(value) | Event::Cdata(value) => {
            black_box(value.as_bytes()).len()
        }
        Event::ProcessingInstruction { target, data } => {
            black_box(target).len() + black_box(data.as_bytes()).len()
        }
    }
}

fn encode(input: &str, encoding: &str) -> Vec<u8> {
    if encoding == "utf8" {
        return input.as_bytes().to_vec();
    }
    assert!(matches!(encoding, "utf16le" | "utf16be"));
    let little = encoding == "utf16le";
    let mut bytes = if little {
        vec![0xff, 0xfe]
    } else {
        vec![0xfe, 0xff]
    };
    for unit in input.encode_utf16() {
        bytes.extend_from_slice(&if little {
            unit.to_le_bytes()
        } else {
            unit.to_be_bytes()
        });
    }
    bytes
}

fn main() {
    let sizes = if cfg!(debug_assertions) {
        &[1, 16][..]
    } else {
        &[1, 256, 4096][..]
    };
    let fixtures = [
        ("tiny", "<n/>"),
        ("attributes", "<n a='é中🙂' b='value'/>"),
        ("text", "<n>text text text text</n>"),
        (
            "lexical",
            "<n><!--comment--><![CDATA[cdata]]><?pi data?></n>",
        ),
        ("normalized", "<n a='&#9;'>é&amp;中&#13;</n>"),
        (
            "namespaces",
            "<p:n xmlns:p='urn:p' p:a='v'><n xmlns='urn:n'/></p:n>",
        ),
    ];
    for (fixture, item) in fixtures {
        if parameter("YAXML_PULL_FIXTURE").is_some_and(|selected| selected != fixture) {
            continue;
        }
        for siblings in numbers("YAXML_PULL_SIBLINGS", sizes) {
            assert!(siblings > 0);
            let xml = format!("<r>{}</r>", item.repeat(siblings));
            for encoding in ["utf8", "utf16le", "utf16be"] {
                if parameter("YAXML_PULL_ENCODING").is_some_and(|selected| selected != encoding) {
                    continue;
                }
                let input = encode(&xml, encoding);
                for capacity in numbers("YAXML_PULL_CAPACITY", &[1, 2, 3, 7, 64, 8192, 65536]) {
                    assert!(capacity > 0);
                    let iterations = numbers(
                        "YAXML_PULL_ITERATIONS",
                        &[if cfg!(debug_assertions) {
                            1
                        } else {
                            (1_048_576 / input.len()).clamp(1, 50)
                        }],
                    )[0];
                    assert!(iterations > 0);
                    let mut first_ns = 0;
                    let mut early_bytes = 0;
                    let mut events = 0;
                    let mut logical_bytes = 0;
                    let started = Instant::now();
                    for _ in 0..iterations {
                        let before_first = Instant::now();
                        let source = BufReader::with_capacity(
                            capacity,
                            Cursor::new(black_box(input.as_slice())),
                        );
                        let mut reader = BufferedReader::new(source);
                        let (event, range) = reader.read_event().unwrap().expect("root event");
                        first_ns += before_first.elapsed().as_nanos();
                        logical_bytes += observe(event);
                        black_box(range);
                        early_bytes += reader.position();
                        events += 1;
                        while let Some((event, range)) = reader.read_event().unwrap() {
                            logical_bytes += observe(event);
                            black_box(range);
                            events += 1;
                        }
                        assert_eq!(
                            reader.position(),
                            input.len(),
                            "benchmark did not consume full input"
                        );
                        black_box(reader.into_inner());
                    }
                    let elapsed = started.elapsed();
                    println!(
                        "fixture={fixture} encoding={encoding} siblings={siblings} capacity={capacity} bytes={} iterations={iterations} events={events} logical_bytes={logical_bytes} elapsed_ns={} first_event_ns_total={first_ns} first_event_consumed_bytes_total={early_bytes} mib_per_s={:.2}",
                        input.len(),
                        elapsed.as_nanos(),
                        input.len() as f64 * iterations as f64
                            / elapsed.as_secs_f64()
                            / 1_048_576.0
                    );
                }
            }
        }
    }
}
