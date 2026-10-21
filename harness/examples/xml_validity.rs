//! Scores DTD validation on the W3C XML Test Suite `valid` and `invalid` cases.
//!
//! `xml_validity --manifest <catalog.tsv> --suite-root <xmlconf> --expected-cases N
//! [--xmllint <path>]` prints one row per case: yaxml's verdict (`valid`,
//! `invalid` or the parse error) with its diagnostic codes and, with
//! `--xmllint`, the verdict of `xmllint --valid --noout` on the same file.
#![forbid(unsafe_code)]

#[path = "support/conformance_catalog.rs"]
#[allow(dead_code)]
mod catalog;

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use yaxml::{FileResolver, Limits, OwnedDocument, valid};

fn yaxml_verdict(root: &Path, path: &str) -> Result<(String, String), String> {
    let input = std::fs::read(root.join(path)).map_err(|error| error.to_string())?;
    let resolver = FileResolver::new(root).map_err(|error| error.to_string())?;
    let document = match OwnedDocument::parse_with(
        &input,
        loading(Limits::default(), path, Box::new(resolver)),
    ) {
        Ok(document) => document,
        Err(error) => return Ok((format!("{:?}", error.kind), String::new())),
    };
    let diagnostics = document.validate();
    let codes: Vec<_> = diagnostics
        .iter()
        .map(|diagnostic| (diagnostic.code as u16).to_string())
        .collect();
    let verdict = match valid::is_valid(&diagnostics) {
        true => "valid",
        false => "invalid",
    };
    Ok((verdict.into(), codes.join(",")))
}

fn xmllint_verdict(xmllint: &str, root: &Path, path: &str) -> String {
    let Ok(output) = Command::new(xmllint)
        .args(["--valid", "--noout", path])
        .current_dir(root)
        .output()
    else {
        return "unavailable".into();
    };
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("parser error") || stderr.contains("namespace error") {
        "error".into()
    } else if stderr.contains("validity error") {
        "invalid".into()
    } else {
        "valid".into()
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let option = |name: &str| {
        args.iter()
            .position(|arg| arg == name)
            .and_then(|index| args.get(index + 1))
            .cloned()
    };
    let manifest = option("--manifest").ok_or("--manifest is required")?;
    let root = option("--suite-root").ok_or("--suite-root is required")?;
    let expected = option("--expected-cases")
        .ok_or("--expected-cases is required")?
        .parse()?;
    let xmllint = option("--xmllint");
    let manifest = std::fs::read(manifest)?;
    let root = Path::new(&root).canonicalize()?;
    let mut counts = BTreeMap::new();
    println!("id\ttype\tyaxml\tcodes\txmllint");
    for case in catalog::parse(&manifest, expected)? {
        if !matches!(case.kind, "valid" | "invalid") {
            continue;
        }
        let (path, _) = case.run_path();
        let (verdict, codes) = yaxml_verdict(&root, path)?;
        let oracle = xmllint.as_deref().map_or(String::new(), |xmllint| {
            xmllint_verdict(xmllint, &root, path)
        });
        println!("{}\t{}\t{verdict}\t{codes}\t{oracle}", case.id, case.kind);
        let verdict = match verdict.as_str() {
            "valid" | "invalid" => verdict.as_str(),
            _ => "error",
        };
        *counts
            .entry(("yaxml", case.kind, verdict.to_owned()))
            .or_insert(0usize) += 1;
        if xmllint.is_some() {
            *counts.entry(("xmllint", case.kind, oracle)).or_insert(0) += 1;
        }
    }
    for ((tool, kind, verdict), count) in counts {
        eprintln!("{tool}\t{kind}\t{verdict}\t{count}");
    }
    Ok(())
}

/// Complete DTD processing (`NOENT | DTDATTR | DTDLOAD`) through `resolver`.
fn loading(
    limits: yaxml::Limits,
    base: &str,
    resolver: Box<dyn yaxml::Resolver>,
) -> yaxml::ParseOptions {
    let flags = yaxml::options::LEGACY | yaxml::options::DTDLOAD;
    yaxml::ParseOptions::new(flags)
        .with_limits(limits)
        .with_resolver(base, resolver)
}
