use std::{hint::black_box, ops::ControlFlow, time::Instant};

use yaxml::{Document, PushParser, Reader};

fn main() {
    let fixtures = [
        ("tiny", "<root a='v'>text</root>".to_owned()),
        (
            "flat",
            format!(
                "<root>{}</root>",
                "<item a='value'>text</item>".repeat(10_000)
            ),
        ),
        ("text", format!("<root>{}</root>", "text ".repeat(100_000))),
        (
            "normalized",
            format!(
                "<root>{}</root>",
                "<item a='&#x9;'>a&amp;b</item>".repeat(10_000)
            ),
        ),
        (
            "namespaces",
            format!(
                "<r xmlns:p='urn:p'>{}</r>",
                "<p:item p:a='v'/>".repeat(10_000)
            ),
        ),
    ];
    let iterations: usize = std::env::var("YAXML_BENCH_ITERATIONS")
        .ok()
        .map(|s| s.parse().expect("positive iteration count"))
        .unwrap_or(if cfg!(debug_assertions) { 1 } else { 50 });
    assert!(iterations > 0, "iterations must be positive");
    for (name, xml) in fixtures {
        for mode in ["dom", "events", "push"] {
            let start = Instant::now();
            for _ in 0..iterations {
                match mode {
                    "dom" => {
                        black_box(Document::parse(black_box(&xml)).unwrap());
                    }
                    "events" => {
                        let mut reader = Reader::new(black_box(xml.as_bytes())).unwrap();
                        while let Some(event) = reader.read_event().unwrap() {
                            black_box(event);
                        }
                    }
                    _ => {
                        let mut parser = PushParser::new();
                        for chunk in black_box(xml.as_bytes()).chunks(8192) {
                            parser
                                .feed(chunk, |event, _| {
                                    black_box(event);
                                    ControlFlow::Continue(())
                                })
                                .unwrap();
                        }
                        parser
                            .finish(|event, _| {
                                black_box(event);
                                ControlFlow::Continue(())
                            })
                            .unwrap();
                    }
                }
            }
            let elapsed = start.elapsed();
            println!(
                "fixture={name} mode={mode} bytes={} iterations={iterations} elapsed_ns={} mib_per_s={:.2}",
                xml.len(),
                elapsed.as_nanos(),
                xml.len() as f64 * iterations as f64 / elapsed.as_secs_f64() / (1024.0 * 1024.0)
            );
        }
    }
}
