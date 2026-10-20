//! XML Schema construction and validation. The input is NUL-separated: a
//! schema document, an instance, then resources served as
//! `file:///f/0.xsd` .. `9.xsd` to `include`, `import` and `redefine`
//! (a missing numbered resource is an I/O error, any other URI is absent).
//! An optional leading control byte in `0x01..=0x08` or `0x0e..=0x1f`, which
//! never starts a well-formed schema, selects tighter limits: bit 0 one
//! document, bit 1 256 resolver bytes and one counter vector, bit 2 depth 2,
//! bit 3 16-1039 model work units (from the schema length), bit 4 no resolver (`0x10` alone is
//! `Schema::parse`). An instance with `schemaLocation` hints also builds a
//! schema with `Schema::from_instance`.
//! Checks: construction always ends with an error or a schema; validation
//! agrees exactly over both DOMs and stream validation (buffered, and pushed
//! in two chunks), repeats identically, and `is_valid` matches the error
//! codes; budgets hold. A schema ID that a later DTD ID or `xml:id` repeats
//! is reported at the later attribute in a stream, so ID errors agree in
//! number only.

use std::sync::{Arc, Mutex};

use std::ops::ControlFlow;

use yaxml::xsd::{self, Limits, Schema, StreamValidator};
use yaxml::{Document, OwnedDocument, ParseOptions, PushParser, Request, Resolver};

fn limits(control: u8, schema: usize) -> Limits {
    let pick = |bit: u8, tight, roomy| if control & bit != 0 { tight } else { roomy };
    let mut limits = Limits::default()
        .max_documents(pick(1, 1, 4))
        .max_bytes(pick(2, 256, 1 << 16))
        .max_depth(pick(4, 2, 32))
        .max_model_work(pick(8, 16 + schema * 7 % 1024, 1 << 16))
        .max_counter_vectors(pick(2, 1, 16));
    limits.xml.max_depth = pick(4, 8, limits.xml.max_depth);
    limits
}

/// Code, offset and message: comparable across DOMs.
fn project<Id>(diagnostics: &[xsd::Diagnostic<Id>]) -> Vec<(xsd::Code, Option<usize>, String)> {
    diagnostics
        .iter()
        .map(|d| (d.code, d.offset, d.message.clone()))
        .collect()
}

fn resolver(files: &[Vec<u8>], requests: &Arc<Mutex<usize>>) -> Box<dyn Resolver> {
    let files = files.to_vec();
    let counter = Arc::clone(requests);
    Box::new(move |request: Request<'_>| {
        *counter.lock().unwrap() += 1;
        let index = request
            .uri
            .strip_prefix("file:///f/")
            .and_then(|rest| rest.strip_suffix(".xsd"))
            .and_then(|n| n.parse::<usize>().ok());
        match index {
            Some(i) if i >= files.len() => Err(std::io::Error::other("no such resource")),
            _ => Ok(index.and_then(|i| files.get(i)).cloned()),
        }
    })
}

/// Construction invariants; the schema when one was built.
fn built(result: Result<Schema, xsd::Error>) -> Option<Schema> {
    match result {
        Ok(schema) => {
            for warning in schema.warnings() {
                assert!(warning.code.is_warning());
                assert!(warning.to_string().ends_with(&warning.message));
                super::check_display(warning);
            }
            Some(schema)
        }
        Err(error) => {
            assert!(error.diagnostics.iter().any(|d| !d.code.is_warning()));
            assert!(error.to_string().starts_with("invalid schema: "));
            super::check_display(&error);
            for diagnostic in &error.diagnostics {
                assert!(diagnostic.to_string().starts_with(&diagnostic.uri));
                super::check_display(diagnostic);
            }
            // Only warnings: the public fields allow it, the parser never builds it.
            let warnings = error
                .diagnostics
                .into_iter()
                .filter(|d| d.code.is_warning());
            let error = xsd::Error {
                diagnostics: warnings.collect(),
            };
            assert_eq!(error.to_string(), "invalid schema");
            None
        }
    }
}

/// Validation invariants over both DOMs.
fn check(schema: &Schema, owned: &OwnedDocument, instance: &[u8]) {
    let diagnostics = schema.validate(owned);
    let streamed = schema
        .validate_stream(instance, ParseOptions::default())
        .expect("the tree parsed");
    let mut validator = StreamValidator::new(schema);
    // Options a reader rejects, and input it cannot read, fail the stream.
    let tree_only = ParseOptions::new(yaxml::options::DTDVALID);
    assert!(schema.validate_stream(instance, tree_only).is_err());
    let truncated = &instance[..instance.len() / 2];
    let _ = schema.validate_stream(truncated, ParseOptions::default());
    let mut parser = PushParser::new();
    let mut emit = |event: &yaxml::Event<'_>, origin| {
        validator.event(event, origin);
        ControlFlow::Continue(())
    };
    let (head, tail) = instance.split_at(instance.len() / 2);
    parser.feed(head, &mut emit).expect("the tree parsed");
    parser.feed(tail, &mut emit).expect("the tree parsed");
    parser.finish(&mut emit).expect("the tree parsed");
    assert_eq!(streamed, validator.finish(), "push");
    let id = |d: &(xsd::Code, Option<usize>, String)| d.2.ends_with(" atomic type 'xs:ID'.");
    let (tree_ids, tree_rest): (Vec<_>, Vec<_>) = project(&diagnostics).into_iter().partition(id);
    let (stream_ids, stream_rest): (Vec<_>, Vec<_>) = project(&streamed).into_iter().partition(id);
    assert_eq!(tree_rest, stream_rest, "stream");
    assert_eq!(tree_ids.len(), stream_ids.len(), "stream IDs");
    assert_eq!(
        project(&diagnostics),
        project(&schema.validate(owned)),
        "repeatable"
    );
    assert!(diagnostics.iter().all(|d| !d.message.is_empty()));
    assert_eq!(
        xsd::is_valid(&diagnostics),
        diagnostics.iter().all(|d| d.code.is_warning())
    );
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

pub fn exercise(input: &[u8]) {
    let (control, input) = match input.split_first() {
        Some((&c, rest)) if matches!(c, 0x01..=0x08 | 0x0e..=0x1f) => (c, rest),
        _ => (0, input),
    };
    let mut pieces = input.split(|&b| b == 0);
    let schema_text = pieces.next().unwrap_or_default();
    let limits = limits(control, schema_text.len());
    let instance = pieces.next().unwrap_or_default();
    let files: Vec<Vec<u8>> = pieces.take(10).map(<[u8]>::to_vec).collect();
    let requests = Arc::new(Mutex::new(0usize));
    let result = match control {
        0x10 => Schema::parse(schema_text, "file:///f/main.xsd"),
        _ => Schema::with_resolver(
            schema_text,
            "file:///f/main.xsd",
            (control & 0x10 == 0).then(|| resolver(&files, &requests)),
            limits,
        ),
    };
    assert!(*requests.lock().unwrap() <= limits.max_documents);
    let schema = built(result);
    // A recovered instance without an element reports its missing root.
    let recover = yaxml::ParseOptions::new(yaxml::options::RECOVER);
    if let (Some(schema), Ok(rootless)) = (&schema, OwnedDocument::parse_with(instance, recover))
        && rootless.document_element().is_none()
    {
        assert!(!xsd::is_valid(&schema.validate(&rootless)));
    }
    let Ok(owned) = OwnedDocument::parse_bytes(instance) else {
        return;
    };
    if let Some(schema) = &schema {
        check(schema, &owned, instance);
    }
    if instance.windows(14).any(|w| w == b"schemaLocation") {
        let requests = Arc::new(Mutex::new(0usize));
        let hinted = Schema::from_instance(
            &owned,
            "file:///f/instance.xml",
            resolver(&files, &requests),
            limits,
        );
        assert!(*requests.lock().unwrap() <= limits.max_documents);
        if let Some(schema) = built(hinted) {
            check(&schema, &owned, instance);
        }
    }
}
