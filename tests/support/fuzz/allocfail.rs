//! Allocation failure. yaxml-harness's `allocfail` target runs [`run`] under a
//! global allocator that fails one chosen allocation (this crate forbids
//! unsafe code, so the allocator lives there) and compares the result with an
//! unarmed run through [`check`].
//!
//! Format: a surface byte, a flags byte (bits 0-6 as the options selector,
//! bit 7 adds `DTDLOAD`), then the document; NUL-separated pieces after it
//! are served as resources `0`..`9` to every parse. Surfaces 0-9: the
//! borrowed and owned DOMs (with `try_clone` and `save`), the slice, push,
//! buffered and text readers, owned editing and construction, `Writer`,
//! `XINCLUDE`, and HTML parsing and saving. Surfaces 10-18 take a second
//! input from the first piece (the rest are the resources): XPath (the
//! expression), XML Schema, RELAX NG and Schematron (the schema, compiled and
//! validated against the document), C14N (an optional node-set expression),
//! `TextWriter` (replaying the document's events), catalogs (the document;
//! the piece is looked up), the regexp engine (the document is the pattern,
//! the piece is matched) and HTML event parsing.

use std::fmt::Write as _;
use std::hash::{DefaultHasher, Hasher};
use std::io::BufReader;
use std::ops::ControlFlow;

use yaxml::catalog::Catalog;
use yaxml::options::{DTDLOAD, XINCLUDE};
use yaxml::xpath::{Context, XPath};
use yaxml::{
    AttributeSpec, BufferedReader, Document, EditError, EditErrorKind, EditLimits, ElementItem,
    ElementSpec, Event, NameSpec, NamespaceSpec, OwnedDocument, ParseOptions, PrefixPolicy,
    PushParser, Reader, Request, Resolver, TextReader, TextWriter, Writer, c14n, html, regexp,
    relaxng, save, schematron, xsd,
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

/// A sink for `Writer` and `TextWriter`: the caller's buffer is not yaxml's.
impl std::io::Write for Digest {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.write(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Validation output into the digest, or the allocation failure one of its
/// `codes` reports. Codes are matched by name, so this driver also builds
/// against sources that predate them.
fn diagnosed<C: std::fmt::Debug>(
    out: &mut Digest,
    output: &dyn std::fmt::Debug,
    mut codes: impl Iterator<Item = C>,
) -> Result<(), String> {
    match codes.any(|code| format!("{code:?}") == "Allocation") {
        true => Err("Allocation".to_owned()),
        false => write!(out, "{output:?}").map_err(|_| String::new()),
    }
}

/// Runs the surface the first byte selects. Deterministic, so an unarmed run
/// is the baseline for an armed one.
pub fn run(input: &[u8]) -> Run {
    let [surface, selector, rest @ ..] = input else {
        return Ok(0);
    };
    let surface = surface % 20;
    let mut pieces = rest.split(|byte| *byte == 0);
    let document = pieces.next().unwrap_or_default();
    let pieces: Vec<&[u8]> = pieces.take(11).collect();
    let (second, served) = match pieces.split_first() {
        Some((second, served)) if surface >= 10 => (*second, served),
        _ => (&[][..], &pieces[..]),
    };
    let expression = std::str::from_utf8(second).unwrap_or_default();
    let flags = super::options::split(&[selector | 0x80]).0 | (u32::from(selector >> 7) * DTDLOAD);
    let resolver = || -> Box<dyn Resolver> {
        let resources: Vec<Vec<u8>> = served.iter().take(10).map(|p| p.to_vec()).collect();
        Box::new(move |request: Request<'_>| {
            let name = request.uri.rsplit('/').next().unwrap_or_default();
            let index = name.parse::<usize>().ok();
            Ok(index.and_then(|index| resources.get(index).cloned()))
        })
    };
    let options = || {
        let flags = flags | (u32::from(surface == 8) * XINCLUDE);
        ParseOptions::new(flags).with_resolver("file:///d/doc.xml", resolver())
    };
    let text = |error: &dyn std::fmt::Debug| format!("{error:?}");
    let mut out = Digest::default();
    match surface {
        0 => {
            let tree = Document::parse_with(document, options()).map_err(|e| text(&e))?;
            let saved = save::to_vec(&tree, &Default::default()).map_err(|e| text(&e))?;
            out.0.write(&saved);
            let _ = write!(out, "{:?}", tree.errors());
            diagnosed(
                &mut out,
                &tree.validity().len(),
                tree.validity().iter().map(|d| d.code),
            )?;
        }
        1 | 8 => {
            let tree = OwnedDocument::parse_with(document, options()).map_err(|e| text(&e))?;
            let copy = tree.try_clone().map_err(|e| text(&e))?;
            let saved = save::to_vec(&copy, &Default::default()).map_err(|e| text(&e))?;
            out.0.write(&saved);
            let _ = write!(out, "{:?}", tree.errors());
            diagnosed(
                &mut out,
                &tree.validity().len(),
                tree.validity().iter().map(|d| d.code),
            )?;
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
        10 => {
            let tree = Document::parse_with(document, options()).map_err(|e| text(&e))?;
            let xpath = XPath::compile(expression).map_err(|e| text(&e))?;
            let context = Context::new().with_namespace("p", "urn:p");
            let value = xpath.evaluate(&tree, tree.root().id(), &context);
            let _ = write!(out, "{:?}", value.map_err(|e| text(&e))?);
        }
        11 => {
            let schema = xsd::Schema::with_resolver(
                second,
                "file:///d/s.xsd",
                Some(resolver()),
                Default::default(),
            );
            let schema = schema.map_err(|e| text(&e))?;
            let tree = Document::parse_with(document, options()).map_err(|e| text(&e))?;
            let found = schema.validate(&tree);
            diagnosed(&mut out, &found, found.iter().map(|d| d.code))?;
            let found = schema.validate_stream(document, ParseOptions::new(flags));
            let found = found.map_err(|e| text(&e))?;
            diagnosed(&mut out, &found, found.iter().map(|d| d.code))?;
        }
        12 => {
            let limits = relaxng::Limits::default();
            let schema = relaxng::Schema::parse(second, "file:///d/s.rng", resolver(), &limits);
            let schema = schema.map_err(|e| text(&e))?;
            let tree = Document::parse_with(document, options()).map_err(|e| text(&e))?;
            let found = schema.validate(&tree);
            diagnosed(&mut out, &found, found.iter().map(|d| d.code))?;
            let mut reader = TextReader::with_options(document, options()).map_err(|e| text(&e))?;
            reader
                .relaxng_set_schema(Some(&schema))
                .map_err(|e| text(&e))?;
            while reader.read().map_err(|e| text(&e))? {}
            let found = reader.relaxng_diagnostics();
            diagnosed(&mut out, &found, found.iter().map(|d| d.code))?;
        }
        13 => {
            let limits = schematron::Limits::default();
            let schema = schematron::Schema::parse(second, "file:///d/s.sct", resolver(), &limits);
            let schema = schema.map_err(|e| text(&e))?;
            let tree = Document::parse_with(document, options()).map_err(|e| text(&e))?;
            let found = schema.validate(&tree, schematron::OUT_ERROR, document);
            let codes = found.output.iter().filter_map(|item| match item {
                schematron::Output::Error(d) => Some(d.code),
                schematron::Output::Text(_) => None,
            });
            diagnosed(&mut out, &found, codes)?;
        }
        14 => {
            let tree = Document::parse_with(document, options()).map_err(|e| text(&e))?;
            let modes = [
                c14n::Mode::C14n10,
                c14n::Mode::C14n11,
                c14n::Mode::Exclusive10,
            ];
            let mode = modes[usize::from(selector % 3)];
            let nodes = match expression {
                "" => None,
                expression => {
                    let xpath = XPath::compile(expression).map_err(|e| text(&e))?;
                    let context = Context::new().with_namespace("p", "urn:p");
                    Some(xpath.select_nodes(&tree, tree.root().id(), &context))
                }
            };
            let nodes = nodes.transpose().map_err(|e| text(&e))?;
            let options = c14n::Options::new(mode)
                .with_comments(selector & 8 != 0)
                .nodes(nodes.as_deref());
            c14n::write(&tree, &options, &mut out).map_err(|e| text(&e))?;
        }
        15 => {
            let mut reader = Reader::with_options(document, options()).map_err(|e| text(&e))?;
            let mut writer = TextWriter::new(&mut out);
            writer.set_indent(selector & 8 != 0);
            while let Some(event) = reader.read_event().map_err(|e| text(&e))? {
                let written = match &event {
                    Event::Start(element) | Event::Empty(element) => writer
                        .start_element(element.name.qualified())
                        .and_then(|()| {
                            element.attributes.iter().try_for_each(|attribute| {
                                let name = attribute.name.qualified();
                                writer.write_attribute(name, &attribute.value)
                            })
                        }),
                    Event::End(_) => writer.end_element(),
                    Event::Text(text) => writer.write_string(text),
                    Event::Cdata(text) => writer.write_cdata(Some(text)),
                    Event::Comment(text) => writer.write_comment(text),
                    Event::ProcessingInstruction { target, data } => {
                        writer.write_pi(target, Some(data))
                    }
                    _ => Ok(()),
                };
                written
                    .and_then(|()| match event {
                        Event::Empty(_) => writer.end_element(),
                        _ => Ok(()),
                    })
                    .map_err(|e| text(&e))?;
            }
            writer.end_document().map_err(|e| text(&e))?;
        }
        16 => {
            let limits = Default::default();
            let catalog = Catalog::parse(document, limits, "file:///d/catalog.xml", resolver());
            let mut catalog = catalog.map_err(|e| text(&e))?;
            let found = catalog.resolve(Some(expression), Some(expression));
            let uri = catalog.resolve_uri(expression);
            let _ = write!(out, "{found:?}{uri:?}{}", catalog.dump());
        }
        17 => {
            let pattern = std::str::from_utf8(document).unwrap_or_default();
            let regex = regexp::Regex::new(pattern).map_err(|e| text(&e))?;
            let _ = write!(out, "{}", regex.is_match(expression));
        }
        18 => {
            let parsed = html::parse_events(document, Default::default(), |event| {
                let _ = write!(out, "{event:?}");
                ControlFlow::Continue(())
            });
            let _ = write!(out, "{:?}", parsed.map_err(|e| text(&e))?.diagnostics);
        }
        _ => {
            let tree = Document::parse_with(document, options()).map_err(|e| text(&e))?;
            let mut writer = Writer::new(&mut out);
            writer.write_document(&tree).map_err(|e| text(&e))?;
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
