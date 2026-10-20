//! Allocation failure. yaxml-harness's `allocfail` target runs [`run`] under a
//! global allocator that fails one chosen allocation (this crate forbids
//! unsafe code, so the allocator lives there) and compares the result with an
//! unarmed run through [`check`].
//!
//! Format: a surface byte, a flags byte (bits 0-6 as the options selector,
//! bit 7 adds `DTDLOAD`), then the document; NUL-separated pieces after it
//! are served as resources `0`..`9` to every parse. Surfaces: the borrowed
//! and owned DOMs (with `try_clone` and `save`), the slice, push, buffered
//! and text readers, owned editing and construction, `Writer`, `XINCLUDE`,
//! and HTML parsing and saving.

use std::fmt::Write as _;
use std::hash::{DefaultHasher, Hasher};
use std::io::BufReader;
use std::ops::ControlFlow;

use yaxml::options::{DTDLOAD, XINCLUDE};
use yaxml::{
    AttributeSpec, BufferedReader, Document, EditError, EditErrorKind, EditLimits, ElementItem,
    ElementSpec, NameSpec, NamespaceSpec, OwnedDocument, ParseOptions, PrefixPolicy, PushParser,
    Reader, Request, TextReader, Writer, save,
};

/// What one surface produced: a hash of its output, or its error.
pub type Run = Result<u64, String>;

/// Hashes what is written to it: the digest itself never allocates, so the
/// failures injected land in yaxml.
#[derive(Default)]
struct Digest(DefaultHasher);

impl std::fmt::Write for Digest {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        self.0.write(text.as_bytes());
        Ok(())
    }
}

/// Runs the surface the first byte selects. Deterministic, so an unarmed run
/// is the baseline for an armed one.
pub fn run(input: &[u8]) -> Run {
    let [surface, selector, rest @ ..] = input else {
        return Ok(0);
    };
    let mut pieces = rest.split(|byte| *byte == 0);
    let document = pieces.next().unwrap_or_default();
    let resources: Vec<Vec<u8>> = pieces.take(10).map(<[u8]>::to_vec).collect();
    let flags = super::options::split(&[selector | 0x80]).0 | (u32::from(selector >> 7) * DTDLOAD);
    let options = || {
        let resources = resources.clone();
        let resolver = move |request: Request<'_>| {
            let name = request.uri.rsplit('/').next().unwrap_or_default();
            let index = name.parse::<usize>().ok();
            Ok(index.and_then(|index| resources.get(index).cloned()))
        };
        let flags = flags | (u32::from(*surface % 10 == 8) * XINCLUDE);
        ParseOptions::new(flags).with_resolver("file:///d/doc.xml", Box::new(resolver))
    };
    let text = |error: &dyn std::fmt::Debug| format!("{error:?}");
    let mut out = Digest::default();
    match surface % 10 {
        0 => {
            let tree = Document::parse_with(document, options()).map_err(|e| text(&e))?;
            let saved = save::to_vec(&tree, &Default::default()).map_err(|e| text(&e))?;
            out.0.write(&saved);
            let _ = write!(out, "{:?}{}", tree.errors(), tree.validity().len());
        }
        1 | 8 => {
            let tree = OwnedDocument::parse_with(document, options()).map_err(|e| text(&e))?;
            let copy = tree.try_clone().map_err(|e| text(&e))?;
            let saved = save::to_vec(&copy, &Default::default()).map_err(|e| text(&e))?;
            out.0.write(&saved);
            let _ = write!(out, "{:?}{}", tree.errors(), tree.validity().len());
        }
        2 => {
            let mut reader = Reader::with_options(document, options()).map_err(|e| text(&e))?;
            while let Some(event) = reader.read_event().map_err(|e| text(&e))? {
                let _ = write!(out, "{event:?}");
            }
        }
        3 => {
            let mut parser = PushParser::with_options(options()).map_err(|e| text(&e))?;
            let mut emit = |event: &yaxml::Event<'_>, _| {
                let _ = write!(out, "{event:?}");
                ControlFlow::Continue(())
            };
            for chunk in document.chunks(61) {
                parser.feed(chunk, &mut emit).map_err(|e| text(&e))?;
            }
            parser.finish(&mut emit).map_err(|e| text(&e))?;
        }
        4 => {
            let input = BufReader::with_capacity(7, document);
            let mut reader =
                BufferedReader::with_options(input, options()).map_err(|e| text(&e))?;
            while let Some((event, _)) = reader.read_event().map_err(|e| text(&e))? {
                let _ = write!(out, "{event:?}");
            }
        }
        5 => {
            let mut reader = TextReader::with_options(document, options()).map_err(|e| text(&e))?;
            while reader.read().map_err(|e| text(&e))? {
                let (kind, name, value) = (reader.node_type(), reader.name(), reader.value());
                let _ = write!(out, "{kind:?}{name:?}{value:?}");
                if reader.depth() == 1 {
                    let outer = reader.read_outer_xml().map_err(|e| text(&e))?;
                    out.0.write(outer.unwrap_or_default().as_bytes());
                }
            }
        }
        6 => edit(document, options, &mut out)?,
        9 => {
            let parsed = yaxml::html::parse(document, Default::default()).map_err(|e| text(&e))?;
            let saved = yaxml::html::save(&parsed.document, true).map_err(|e| text(&e))?;
            out.0.write(&saved);
            let _ = write!(out, "{:?}{}", parsed.diagnostics, parsed.stopped);
        }
        _ => {
            let tree = Document::parse_with(document, options()).map_err(|e| text(&e))?;
            let mut writer = Writer::new(Vec::new());
            writer.write_document(&tree).map_err(|e| text(&e))?;
            out.0.write(&writer.into_inner());
        }
    }
    Ok(out.0.finish())
}

/// Owned edits and construction: every edit's result goes into the digest,
/// and an allocation failure fails the run.
fn edit(
    document: &[u8],
    options: impl Fn() -> ParseOptions,
    out: &mut Digest,
) -> Result<(), String> {
    let text = |error: &dyn std::fmt::Debug| format!("{error:?}");
    let name = |i: usize| NameSpec {
        local: ["a", "id", "c"][i % 3],
        namespace: [None, Some("urn:p"), Some("urn:q")][i / 3 % 3],
        prefix: [PrefixPolicy::Automatic, PrefixPolicy::Exact(Some("p"))][i / 9 % 2],
    };
    let items = [
        ElementItem::Namespace(NamespaceSpec {
            prefix: Some("q"),
            uri: "urn:q",
        }),
        ElementItem::Attribute(AttributeSpec {
            name: name(4),
            value: "v",
        }),
    ];
    let mut record = |result: Result<(), EditError>| match result {
        Err(error) if error.kind == EditErrorKind::Allocation => Err(text(&error)),
        result => {
            let _ = write!(out, "{result:?}");
            Ok(())
        }
    };
    let element = ElementSpec {
        name: name(1),
        items: &items,
    };
    let created = OwnedDocument::create(element, EditLimits::DEFAULT);
    record(created.as_ref().map(drop).map_err(|e| *e))?;
    let mut tree = OwnedDocument::parse_with(document, options()).map_err(|e| text(&e))?;
    let other = created.map_err(|e| text(&e))?;
    let ids: Vec<_> = tree.nodes().map(|node| node.id()).take(24).collect();
    let mut editor = tree.edit(EditLimits::DEFAULT).map_err(|e| text(&e))?;
    for (i, &id) in ids.iter().enumerate() {
        let element = ElementSpec {
            name: name(i),
            items: &items,
        };
        let result = match i % 8 {
            0 => editor.append_element(id, element).map(drop),
            1 => editor.set_attribute(id, name(i / 8), "w"),
            2 => editor.append_text(id, "t").map(drop),
            3 => editor.declare_namespace(id, Some("p"), "urn:p"),
            4 => editor.import(&other, other.root().id(), true).map(drop),
            5 => editor.copy(id, true).map(drop),
            6 => editor.set_name(id, name(i)),
            _ => editor.remove_attribute(id, None, "a"),
        };
        record(result)?;
    }
    record(editor.compact())?;
    let saved = save::to_vec(&tree, &Default::default()).map_err(|e| text(&e))?;
    out.0.write(&saved);
    Ok(())
}

/// An armed run either matches the baseline (the failure was absorbed) or
/// fails with an allocation error.
pub fn check(baseline: &Run, armed: &Run) {
    let reported = matches!(armed, Err(error) if error.contains("Allocation"));
    assert!(
        reported || armed == baseline,
        "allocation failure changed the result: {armed:?}, unarmed {baseline:?}"
    );
}
