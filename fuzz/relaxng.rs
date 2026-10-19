//! RELAX NG schemas and instances. A testcase is NUL-separated: the schema
//! (`d/main.rng`), the instance, then files served as `d/0`..`d/9` to
//! `include` and `externalRef`. A schema piece that is not markup is wrapped
//! as the content of `<element name="r">`.

use std::sync::{Arc, Mutex};

use yaxml::relaxng::{Code, Diagnostic, Limits, Schema};
use yaxml::{Document, OwnedDocument, Request, Resolver};

const NAMES: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];
const MAX_LOADS: usize = 16;
const MAX_WORK: usize = 200_000;

fn limits(max_work: usize) -> Limits {
    let mut limits = Limits {
        max_work,
        max_include_depth: 8,
        ..Limits::default()
    };
    limits.xml.max_external_loads = MAX_LOADS;
    limits.xml.max_external_bytes = 1 << 16;
    limits
}

fn resolver(resources: &[&[u8]], log: &Arc<Mutex<usize>>) -> Box<dyn Resolver> {
    let resources: Vec<Vec<u8>> = resources.iter().map(|piece| piece.to_vec()).collect();
    let log = log.clone();
    Box::new(move |request: Request<'_>| {
        *log.lock().unwrap() += 1;
        let name = request.uri.strip_prefix("d/").unwrap_or("");
        Ok((NAMES.iter().position(|candidate| *candidate == name))
            .and_then(|index| resources.get(index).cloned()))
    })
}

/// Code, offset and message: what both DOMs must agree on.
fn summary<I>(diagnostics: &[Diagnostic<I>]) -> Vec<(Code, Option<usize>, String)> {
    (diagnostics.iter())
        .map(|d| (d.code, d.offset, d.message.clone()))
        .collect()
}

fn parse(schema: &[u8], resources: &[&[u8]], max_work: usize) -> Option<Schema> {
    let log = Arc::new(Mutex::new(0));
    let parsed = Schema::parse(
        schema,
        "d/main.rng",
        resolver(resources, &log),
        &limits(max_work),
    );
    assert!(*log.lock().unwrap() <= MAX_LOADS, "load budget ignored");
    if let Err(errors) = &parsed {
        assert!(!errors.0.is_empty() && !errors.to_string().is_empty());
    }
    parsed.ok()
}

pub fn exercise(input: &[u8]) {
    let mut pieces = input.split(|byte| *byte == 0);
    let schema = pieces.next().unwrap_or_default();
    let instance = pieces.next().unwrap_or_default();
    let resources: Vec<&[u8]> = pieces.collect();
    let wrapped;
    let schema = match schema.trim_ascii_start().first() {
        Some(b'<') => schema,
        _ => {
            wrapped = [
                br#"<element name="r" xmlns="http://relaxng.org/ns/structure/1.0" datatypeLibrary="http://www.w3.org/2001/XMLSchema-datatypes">"#.as_slice(),
                schema,
                b"</element>",
            ]
            .concat();
            &wrapped
        }
    };
    let Some(compiled) = parse(schema, &resources, MAX_WORK) else {
        return;
    };
    // Building is deterministic.
    assert!(parse(schema, &resources, MAX_WORK).is_some());

    let (Ok(text), Ok(owned)) = (
        std::str::from_utf8(instance),
        OwnedDocument::with_limits(instance, yaxml::Limits::default()),
    ) else {
        return;
    };
    let Ok(document) = Document::parse(text) else {
        return;
    };
    let borrowed = compiled.validate(&document);
    let found = summary(&borrowed);
    assert_eq!(found, summary(&compiled.validate(&owned)), "DOMs disagree");
    assert_eq!(
        found,
        summary(&compiled.validate(&document)),
        "reuse changed the result"
    );
    for diagnostic in &borrowed {
        let node = diagnostic.node.and_then(|id| document.node(id));
        assert_eq!(
            diagnostic.offset,
            node.and_then(|n| n.range()).map(|r| r.start)
        );
    }
    let internal = found
        .iter()
        .filter(|(code, ..)| *code == Code::Internal)
        .count();
    assert!(internal <= 1 && (internal == 0 || found.last().unwrap().0 == Code::Internal));
    // A larger budget changes nothing unless the smaller one ran out.
    if internal == 0
        && let Some(generous) = parse(schema, &resources, MAX_WORK * 4)
    {
        assert_eq!(
            found,
            summary(&generous.validate(&document)),
            "budget changed the verdict"
        );
    }
}
