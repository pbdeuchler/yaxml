//! XInclude over fuzz-provided resources. A testcase is NUL-separated: the
//! including document, then the resources served as `0`..`9` beside it. The
//! last piece is also evaluated as an XPointer over both DOMs. Default
//! options with the harness budgets reproduce every result that does not
//! hit the depth limit, and small input-derived budgets either fail or
//! reproduce the full result.

use std::sync::{Arc, Mutex};

use yaxml::xinclude::{self, ErrorKind, Options};
use yaxml::{Document, Limits, OwnedDocument, OwnedNodeKind, Request, Resolver, Writer, xpath};

const LIMITS: Limits = Limits {
    max_input_bytes: 1 << 20,
    max_token_bytes: 1 << 16,
    max_depth: 64,
    max_attributes: 64,
    max_events: 1 << 14,
    max_owned_bytes: 1 << 22,
    max_dtd_bytes: 1 << 16,
    max_entity_expansion_bytes: 1 << 16,
    max_entity_amplification: 5,
    max_entity_references: 1 << 12,
    max_entity_depth: 8,
    max_external_loads: 16,
    max_external_bytes: 1 << 16,
};

const XPATH: xpath::Limits = xpath::Limits {
    max_expression_bytes: 4096,
    max_depth: 64,
    max_operations: 100_000,
    max_nodes: 10_000,
};

const NAMES: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];

/// Requested URIs and response lengths.
type Requests = Vec<(String, usize)>;
type Log = Arc<Mutex<Requests>>;

fn resolver(resources: &[&[u8]], log: &Log) -> Box<dyn Resolver> {
    let resources: Vec<Vec<u8>> = resources.iter().map(|piece| piece.to_vec()).collect();
    let log = log.clone();
    Box::new(move |request: Request<'_>| {
        let name = request.uri.strip_prefix("d/").unwrap_or("");
        let response = NAMES
            .iter()
            .position(|candidate| *candidate == name)
            .and_then(|index| resources.get(index).cloned());
        let length = response.as_ref().map_or(0, Vec::len);
        log.lock().unwrap().push((request.uri.to_owned(), length));
        Ok(response)
    })
}

/// The result serialized, with the requests that produced it.
fn run(
    document: &OwnedDocument,
    resources: &[&[u8]],
    options: &Options,
) -> (Result<Vec<u8>, ErrorKind>, Requests) {
    let log = Log::default();
    let result = xinclude::process(document, "d/doc.xml", resolver(resources, &log), options)
        .map(|result| check(&result))
        .map_err(|error| {
            assert!(error.to_string().starts_with("XInclude error"));
            error.kind
        });
    let requests = log.lock().unwrap().clone();
    // Only the response that exhausts the byte budget may exceed it.
    let served: usize = requests
        .iter()
        .rev()
        .skip(1)
        .map(|(_, length)| length)
        .sum();
    assert!(
        requests.len() <= LIMITS.max_external_loads && served <= LIMITS.max_external_bytes,
        "budgets ignored: {requests:?}"
    );
    (result, requests)
}

/// Every inclusion is replaced, and the result writes and reparses.
fn check(result: &OwnedDocument) -> Vec<u8> {
    assert!(result.nodes().len() <= LIMITS.max_events + 1);
    for node in result.nodes() {
        if let Some(name) = node.name() {
            let xinclude = matches!(
                name.namespace(),
                Some("http://www.w3.org/2001/XInclude" | "http://www.w3.org/2003/XInclude")
            );
            assert!(
                !(xinclude && name.local() == "include"),
                "inclusion left in the result"
            );
        }
    }
    let mut writer = Writer::new(Vec::new());
    writer
        .write_owned_document(result)
        .expect("the result is writable");
    let bytes = writer.into_inner();
    let reparsed = OwnedDocument::parse_bytes(&bytes).expect("the written result reparses");
    let elements = |document: &OwnedDocument| {
        document
            .nodes()
            .filter(|node| node.kind() == OwnedNodeKind::Element)
            .count()
    };
    assert_eq!(elements(&reparsed), elements(result));
    let text = |document: &OwnedDocument| document.root().text_chunks().collect::<String>();
    assert_eq!(text(&reparsed), text(result));
    bytes
}

/// Small load, byte, node and depth budgets chosen by the input bytes.
fn tight(input: &[u8]) -> Options {
    let hash = input.iter().fold(0u32, |hash, &byte| {
        hash.wrapping_mul(31).wrapping_add(byte.into())
    });
    Options {
        base_fixup: true,
        lang_fixup: false,
        max_depth: (hash >> 20 & 3) as usize,
        limits: Limits {
            max_external_loads: (hash & 3) as usize,
            max_external_bytes: (hash >> 2 & 1023) as usize,
            max_events: 4 + (hash >> 12 & 63) as usize,
            ..LIMITS
        },
        xpath: XPATH,
        parse_flags: yaxml::options::LEGACY,
    }
}

pub fn exercise(input: &[u8]) {
    let mut pieces = input.split(|byte| *byte == 0);
    let main = pieces.next().unwrap_or_default();
    let resources: Vec<&[u8]> = pieces.collect();
    let log = Log::default();
    let Ok(document) = OwnedDocument::parse_with(
        main,
        loading(LIMITS, "d/doc.xml", resolver(&resources, &log)),
    ) else {
        return;
    };
    let options = |base_fixup, lang_fixup| Options {
        base_fixup,
        lang_fixup,
        max_depth: 8,
        limits: LIMITS,
        xpath: XPATH,
        parse_flags: yaxml::options::LEGACY,
    };
    let (result, requests) = run(&document, &resources, &options(true, false));
    // Processing is deterministic and never changes its input; the default
    // options differ only in a deeper nesting limit.
    let defaults = Options {
        limits: LIMITS,
        xpath: XPATH,
        ..Options::default()
    };
    if result != Err(ErrorKind::RecursionDepth) {
        assert_eq!(
            run(&document, &resources, &defaults),
            (result.clone(), requests)
        );
    }
    if let (Ok(small), Ok(full)) = (run(&document, &resources, &tight(input)).0, &result) {
        assert_eq!(&small, full, "small budgets changed the result");
    }
    // Fixups only add, change or remove xml:base and xml:lang attributes.
    let (other, _) = run(&document, &resources, &options(false, true));
    if result.is_ok() {
        assert!(
            other.is_ok() || matches!(other, Err(ErrorKind::Xml(_))),
            "{other:?}"
        );
    }

    let pointer = String::from_utf8_lossy(resources.last().copied().unwrap_or(main));
    let owned = yaxml::xpointer::evaluate(&document, &pointer, &XPATH).map(|nodes| nodes.len());
    if let Err(error) = &owned {
        assert!(error.to_string().starts_with("XPointer"));
    }
    let borrowed = Document::parse_with(
        main,
        loading(LIMITS, "d/doc.xml", resolver(&resources, &log)),
    );
    if let Ok(borrowed) = borrowed {
        let borrowed = yaxml::xpointer::evaluate(&borrowed, &pointer, &XPATH);
        assert_eq!(
            borrowed.map(|nodes| nodes.len()),
            owned,
            "DOMs disagree on {pointer:?}"
        );
    }
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
