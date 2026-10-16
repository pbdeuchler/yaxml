use std::{hint::black_box, time::Instant};

use yaxml::{Document, Reader};

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
        for mode in ["dom", "events"] {
            let start = Instant::now();
            for _ in 0..iterations {
                match mode {
                    "dom" => {
                        black_box(Document::parse(black_box(&xml)).unwrap());
                    }
                    _ => {
                        let mut reader = Reader::new(black_box(xml.as_bytes())).unwrap();
                        while let Some(event) = reader.read_event().unwrap() {
                            black_box(event);
                        }
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
