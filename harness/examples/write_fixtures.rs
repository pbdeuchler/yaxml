//! Writes every parse_benchmark fixture to `DIR/<name>.xml` (pgo.sh training input).
#![forbid(unsafe_code)]

#[allow(dead_code)]
#[path = "support/parse_benchmark.rs"]
mod benchmark;

fn main() -> Result<(), String> {
    let dir = std::env::args().nth(1).ok_or("usage: write_fixtures DIR")?;
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    benchmark::FIXTURES.iter().try_for_each(|name| {
        let bytes = benchmark::fixture(name)?;
        std::fs::write(format!("{dir}/{name}.xml"), bytes).map_err(|error| error.to_string())
    })
}
