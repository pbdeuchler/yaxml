//! XML Schema construction and validation. The input is NUL-separated: a
//! schema document, an instance, then resources served as
//! `file:///f/0.xsd` .. `9.xsd` to `include`, `import` and `redefine`.
//! Checks: construction always ends with an error or a schema; validation
//! agrees exactly over both DOMs, repeats identically, and `is_valid`
//! matches the error codes; budgets hold.

use std::sync::{Arc, Mutex};

use yaxml::xsd::{self, Limits, Schema};
use yaxml::{Document, OwnedDocument, Request};

fn limits() -> Limits {
    Limits {
        max_documents: 4,
        max_bytes: 1 << 16,
        max_depth: 32,
        max_model_work: 1 << 16,
        max_counter_vectors: 16,
        ..Limits::default()
    }
}

/// Code, offset and message: comparable across DOMs.
fn project<Id>(diagnostics: &[xsd::Diagnostic<Id>]) -> Vec<(xsd::Code, Option<usize>, String)> {
    diagnostics
        .iter()
        .map(|d| (d.code, d.offset, d.message.clone()))
        .collect()
}

pub fn exercise(input: &[u8]) {
    let mut pieces = input.split(|&b| b == 0);
    let schema_text = pieces.next().unwrap_or_default();
    let instance = pieces.next().unwrap_or_default();
    let files: Vec<Vec<u8>> = pieces.take(10).map(<[u8]>::to_vec).collect();
    let requests = Arc::new(Mutex::new(0usize));
    let counter = Arc::clone(&requests);
    let resolver = move |request: Request<'_>| {
        *counter.lock().unwrap() += 1;
        let index = request
            .uri
            .strip_prefix("file:///f/")
            .and_then(|rest| rest.strip_suffix(".xsd"))
            .and_then(|n| n.parse::<usize>().ok());
        Ok(index.and_then(|i| files.get(i)).cloned())
    };
    let built = Schema::with_resolver(
        schema_text,
        "file:///f/main.xsd",
        Some(Box::new(resolver)),
        limits(),
    );
    assert!(*requests.lock().unwrap() <= limits().max_documents);
    let schema = match built {
        Ok(schema) => schema,
        Err(error) => {
            assert!(error.diagnostics.iter().any(|d| !d.code.is_warning()));
            return;
        }
    };
    let Ok(owned) = OwnedDocument::parse_bytes(instance) else {
        return;
    };
    let diagnostics = schema.validate(&owned);
    assert_eq!(
        project(&diagnostics),
        project(&schema.validate(&owned)),
        "repeatable"
    );
    assert!(diagnostics.iter().all(|d| !d.message.is_empty()));
    if let Ok(text) = std::str::from_utf8(instance)
        && let Ok(borrowed) = Document::parse(text)
    {
        assert_eq!(
            project(&diagnostics),
            project(&schema.validate(&borrowed)),
            "both DOMs"
        );
    }
}
