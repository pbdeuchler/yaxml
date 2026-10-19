//! Replays libxml2 2.15.4's `test/schemas` suite as `runtest.c`'s
//! `schemasTest` does, comparing verdicts with `result/schemas/*.err`:
//!
//! ```sh
//! YAXML_LIBXML2_SOURCE=../yaxml-oracle/libxml2-2.15.4 \
//!   cargo test --release --test xsd_suite -- --ignored --nocapture
//! ```

use std::path::{Path, PathBuf};

use yaxml::xsd::{self, Schema};
use yaxml::{Document, FileResolver, OwnedDocument};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Verdict {
    SchemaError,
    Valid,
    Invalid,
    Internal,
    Unparsed,
}

fn expected(err: &str) -> Verdict {
    if err.contains(" validates\n") {
        Verdict::Valid
    } else if err.contains(" fails to validate\n") {
        Verdict::Invalid
    } else if err.contains("internal error") {
        Verdict::Internal
    } else {
        Verdict::SchemaError
    }
}

fn validity_errors(err: &str) -> usize {
    err.lines()
        .filter(|l| l.contains("Schemas validity error"))
        .count()
}

fn project<Id>(diagnostics: &[xsd::Diagnostic<Id>]) -> Vec<(xsd::Code, Option<usize>, String)> {
    diagnostics
        .iter()
        .map(|d| (d.code, d.offset, d.message.clone()))
        .collect()
}

/// The instance verdict and error count over both DOMs, which must agree.
fn check(schema: &Schema, instance: &Path) -> (Verdict, usize) {
    let bytes = std::fs::read(instance).unwrap();
    // The borrowing DOM is UTF-8 only; other encodings use the owned DOM.
    let Ok(owned) = OwnedDocument::parse_bytes(&bytes) else {
        return (Verdict::Unparsed, 0);
    };
    let a = schema.validate(&owned);
    if let Ok(borrowed) = Document::parse_bytes(&bytes) {
        assert_eq!(
            project(&a),
            project(&schema.validate(&borrowed)),
            "{}",
            instance.display()
        );
    }
    let errors = a.iter().filter(|d| !d.code.is_warning()).count();
    match xsd::is_valid(&a) {
        true => (Verdict::Valid, 0),
        false if a.iter().any(|d| d.code == xsd::Code::VInternal) => (Verdict::Internal, errors),
        false => (Verdict::Invalid, errors),
    }
}

#[test]
#[ignore = "requires YAXML_LIBXML2_SOURCE pointing at the pinned libxml2 2.15.4 source tree"]
fn libxml2_schema_suite_verdicts() {
    let source = PathBuf::from(std::env::var_os("YAXML_LIBXML2_SOURCE").unwrap());
    let dir = source.join("test/schemas");
    let mut schemas: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|name| name.ends_with(".xsd") && name.contains('_'))
        .collect();
    schemas.sort();
    let all: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .collect();
    let (mut total, mut agree, mut counts) = (0, 0, 0);
    let mut differences = Vec::new();
    for base in &schemas {
        // runtest.c: strip ".xsd" and up to two "_N" suffixes for the
        // instance prefix; the error file keeps the schema's number.
        let stem = &base[..base.len() - 4];
        let mut len = stem.len();
        for _ in 0..2 {
            if len >= 2 && stem.as_bytes()[len - 2] == b'_' {
                len -= 2;
            }
        }
        let prefix = &stem[..len];
        let err_prefix = if stem.as_bytes().get(len) == Some(&b'_') {
            &stem[..len + 2]
        } else {
            prefix
        };
        let uri = format!("./test/schemas/{base}");
        let resolver = FileResolver::new(&source).unwrap();
        let built = Schema::with_resolver(
            &std::fs::read(dir.join(base)).unwrap(),
            &uri,
            Some(Box::new(resolver)),
            Default::default(),
        );
        let mut instances: Vec<&String> = all
            .iter()
            .filter(|name| name.starts_with(&format!("{prefix}_")) && name.ends_with(".xml"))
            .collect();
        instances.sort();
        for instance in instances {
            let count = instance.as_bytes()[instance.len() - 5] as char;
            if instance.as_bytes()[instance.len() - 6] != b'_' {
                continue;
            }
            let err_path = source.join(format!("result/schemas/{err_prefix}_{count}.err"));
            let Ok(err) = std::fs::read_to_string(&err_path) else {
                continue;
            };
            total += 1;
            let want = expected(&err);
            let (got, errors) = match &built {
                Err(_) => (Verdict::SchemaError, 0),
                Ok(schema) => check(schema, &dir.join(instance)),
            };
            if got == want {
                agree += 1;
                if errors == validity_errors(&err) {
                    counts += 1;
                }
            } else {
                let detail = match &built {
                    Err(e) => e.to_string(),
                    Ok(schema) => {
                        let bytes = std::fs::read(dir.join(instance)).unwrap();
                        OwnedDocument::parse_bytes(&bytes)
                            .map(|d| {
                                format!(
                                    "{:?}",
                                    schema
                                        .validate(&d)
                                        .iter()
                                        .map(|d| &d.message)
                                        .collect::<Vec<_>>()
                                )
                            })
                            .unwrap_or_default()
                    }
                };
                differences.push(format!(
                    "{base} {instance}: libxml2 {want:?}, yaxml {got:?} {detail}"
                ));
            }
        }
    }
    // runtest.c's "Schemas XSI assembly tests".
    {
        total += 1;
        let instance = dir.join("any1_0.xml");
        let bytes = std::fs::read(&instance).unwrap();
        let resolver = FileResolver::new(&source).unwrap();
        let got = match OwnedDocument::parse_bytes(&bytes) {
            Err(_) => Verdict::Unparsed,
            Ok(document) => match Schema::from_instance(
                &document,
                "./test/schemas/any1_0.xml",
                Box::new(resolver),
                Default::default(),
            ) {
                Ok(schema) => match xsd::is_valid(&schema.validate(&document)) {
                    true => Verdict::Valid,
                    false => Verdict::Invalid,
                },
                Err(_) => Verdict::SchemaError,
            },
        };
        if got == Verdict::Valid {
            agree += 1;
            counts += 1;
        } else {
            differences.push(format!("XSI any1_0.xml: libxml2 Valid, yaxml {got:?}"));
        }
    }
    for difference in &differences {
        println!("{difference}");
    }
    println!("{agree}/{total} verdicts agree ({counts} with equal validity error counts)");
}
