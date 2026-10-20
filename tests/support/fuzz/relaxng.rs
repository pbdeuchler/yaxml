//! RELAX NG schemas and instances. A testcase is NUL-separated: the schema
//! (`d/main.rng`), the instance, then files served as `d/0`..`d/9` to
//! `include` and `externalRef`. A schema piece that is not markup is wrapped
//! as the content of `<element name="r">`. The resolver fails with an I/O
//! error for `d/err`. A rebuild under small limits derived from the schema
//! bytes either fails or validates exactly as the full build, unless its
//! validation runs out of work; `Schema::from_tree` over the borrowed DOM
//! reports the same build errors as `Schema::parse`.

use std::sync::{Arc, Mutex};

use std::ops::ControlFlow;
use yaxml::relaxng::{Code, Diagnostic, Limits, Schema, SchemaCode, SchemaErrors};

use yaxml::{BufferedReader, Document, Event, OwnedDocument, PushParser, Request, Resolver};

const NAMES: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];
const MAX_LOADS: usize = 16;
const MAX_WORK: usize = 200_000;

fn limits(max_work: usize) -> Limits {
    let mut limits = Limits::default().max_work(max_work).max_include_depth(8);
    limits.xml.max_external_loads = MAX_LOADS;
    limits.xml.max_external_bytes = 1 << 16;
    limits
}

/// Small load, byte, depth and work ceilings chosen by the schema bytes.
fn tight(schema: &[u8]) -> Limits {
    let hash = schema.iter().fold(0u32, |hash, &byte| {
        hash.wrapping_mul(31).wrapping_add(byte.into())
    });
    let mut limits = Limits::default()
        .max_work(1 + (hash & 1023) as usize * 8)
        .max_include_depth((hash >> 10 & 3) as usize);
    limits.xml.max_external_loads = (hash >> 12 & 3) as usize;
    limits.xml.max_external_bytes = (hash >> 14 & 511) as usize;
    limits
}

fn resolver(resources: &[&[u8]], log: &Arc<Mutex<usize>>) -> Box<dyn Resolver> {
    let resources: Vec<Vec<u8>> = resources.iter().map(|piece| piece.to_vec()).collect();
    let log = log.clone();
    Box::new(move |request: Request<'_>| {
        *log.lock().unwrap() += 1;
        let name = request.uri.strip_prefix("d/").unwrap_or("");
        if name == "err" {
            return Err(std::io::Error::other("fuzz resolver failure"));
        }
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

fn build(schema: &[u8], resources: &[&[u8]], limits: &Limits) -> Result<Schema, SchemaErrors> {
    let log = Arc::new(Mutex::new(0));
    let parsed = Schema::parse(schema, "d/main.rng", resolver(resources, &log), limits);
    assert!(
        *log.lock().unwrap() <= limits.xml.max_external_loads,
        "load budget ignored"
    );
    if let Err(errors) = &parsed {
        assert!(!errors.0.is_empty() && !errors.to_string().is_empty());
    }
    parsed
}

fn parse(schema: &[u8], resources: &[&[u8]], max_work: usize) -> Option<Schema> {
    build(schema, resources, &limits(max_work)).ok()
}

/// Error codes, or None for a schema.
fn codes(result: &Result<Schema, SchemaErrors>) -> Option<Vec<SchemaCode>> {
    result
        .as_ref()
        .err()
        .map(|errors| errors.0.iter().map(|error| error.code).collect())
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
    let full = build(schema, &resources, &limits(MAX_WORK));
    // Schema::parse reads schema documents with entities substituted.
    let legacy = yaxml::ParseOptions::new(yaxml::options::LEGACY).with_limits(limits(MAX_WORK).xml);
    if let Ok(tree) = Document::parse_with(schema, legacy) {
        let log = Arc::new(Mutex::new(0));
        let built = Schema::from_tree(
            &tree,
            "d/main.rng",
            resolver(&resources, &log),
            &limits(MAX_WORK),
        );
        assert_eq!(codes(&built), codes(&full), "from_tree differs from parse");
    }
    let small = build(schema, &resources, &tight(schema));
    let Ok(compiled) = full else {
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
    let lines = yaxml::LineIndex::new(instance);
    for diagnostic in &borrowed {
        super::check_described(diagnostic.describe(&lines), &lines);
    }
    let found = summary(&borrowed);
    assert_eq!(found, summary(&compiled.validate(&owned)), "DOMs disagree");
    assert_eq!(
        found,
        summary(&compiled.validate(&document)),
        "reuse changed the result"
    );
    assert_eq!(
        Some(&found),
        streamed(&compiled, instance).as_ref(),
        "stream differs"
    );
    assert_eq!(
        Some(found.clone()),
        pushed(&compiled, instance, document.dtd()),
        "push differs"
    );
    for diagnostic in &borrowed {
        let node = diagnostic.node.and_then(|id| document.node(id));
        assert_eq!(
            diagnostic.offset,
            node.and_then(|n| n.range()).map(|r| r.start)
        );
    }
    if let Ok(small) = &small {
        let diagnostics = summary(&small.validate(&document));
        if diagnostics.iter().all(|(code, ..)| *code != Code::Internal) {
            assert_eq!(found, diagnostics, "small limits changed the verdict");
        }
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

/// Streaming validation of `BufferedReader` events; `None` if not well-formed.
fn streamed(schema: &Schema, instance: &[u8]) -> Option<Vec<(Code, Option<usize>, String)>> {
    let mut reader = BufferedReader::new(instance);
    let mut stream = schema.stream();
    while let Some((event, origin)) = reader.read_event().ok()? {
        let doctype = matches!(event, Event::Doctype(_));
        stream.event(&event, origin);
        if let Some(dtd) = reader.dtd().filter(|_| doctype) {
            stream.set_dtd(dtd);
        }
    }
    Some(summary(stream.diagnostics()))
}

/// Streaming validation of `PushParser` events fed in chunks the input
/// length chooses, with the DOM's DTD.
fn pushed(
    schema: &Schema,
    instance: &[u8],
    dtd: Option<&yaxml::valid::Dtd>,
) -> Option<Vec<(Code, Option<usize>, String)>> {
    let mut stream = schema.stream();
    dtd.inspect(|dtd| stream.set_dtd(dtd));
    let mut parser = PushParser::new();
    let mut feed = |event: &Event<'_>, origin| {
        stream.event(event, origin);
        ControlFlow::Continue(())
    };
    for chunk in instance.chunks(1 + instance.len() % 7) {
        parser.feed(chunk, &mut feed).ok()?;
    }
    parser.finish(&mut feed).ok()?;
    Some(summary(stream.diagnostics()))
}
