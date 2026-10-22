//! SAX2-style callbacks (libxml2's `xmlSAXHandler` with `xmlSAXVersion(2)`,
//! `xmlSAXUserParseMemory`): [`parse`] reads a document with the slice
//! [`Reader`] and calls a [`Handler`] for each event, in libxml2's order:
//! `start_document`; for a DOCTYPE `internal_subset`, the internal subset's
//! declarations, `external_subset`, and before the root's `start_element` the
//! declarations the external subset added; then the content and
//! `end_document`. Errors kept by the reader (warnings, namespace errors,
//! errors `RECOVER` repaired) are reported before the event they precede; an
//! error that ends the parse goes to `fatal_error`, then `end_document`, and
//! is returned. A callback returning [`ControlFlow::Break`] stops the parse
//! (`xmlStopParser`): nothing more is called and `parse` returns `Ok`.
//!
//! Differences from libxml2 (docs/compatibility.md#sax2-callbacks):
//!
//! - declarations are reported by kind (entities, notations, then each
//!   element type with its attribute lists) after each subset, not
//!   interleaved in source order; a repeated declaration, and comments and
//!   PIs inside the DTD, are not reported;
//! - without `NOENT` an entity reference is `reference` alone (libxml2 also
//!   reports the content of the first reference to each entity, as it parses
//!   it into the entity's node list); with `NOENT` every reference's content
//!   is reported in full (libxml2 replays later ones as their text only);
//! - DTD defaults reach `start_element` only under `DTDATTR` (libxml2 passes
//!   them always, counted in `nb_defaulted`);
//! - libxml2's queries to the handler (`getEntity`, `resolveEntity`,
//!   `isStandalone`, `hasInternalSubset`, ...) have no counterpart: the
//!   parser keeps the declarations and a [`Resolver`](crate::Resolver) loads
//!   resources; there is no locator (errors carry offsets) and no
//!   `ignorableWhitespace` (`NOBLANKS` is rejected, as by the `Reader`).
//!
//! ```
//! use std::ops::ControlFlow;
//! use yaxml::{Element, ParseOptions, sax};
//!
//! #[derive(Default)]
//! struct Names(Vec<String>);
//!
//! impl sax::Handler for Names {
//!     fn start_element(&mut self, element: &Element<'_>) -> ControlFlow<()> {
//!         self.0.push(element.name.qualified().to_owned());
//!         ControlFlow::Continue(())
//!     }
//! }
//!
//! let mut names = Names::default();
//! sax::parse(b"<r><a/><b/></r>", ParseOptions::default(), &mut names)?;
//! assert_eq!(names.0, ["r", "a", "b"]);
//! # Ok::<(), yaxml::Error>(())
//! ```

use std::ops::ControlFlow;

use crate::valid::{self, ContentKind, Dtd, EntityKind};
use crate::{
    Declaration, Doctype, Element, Error, ErrorKind, Event, Name, ParseOptions, Reader, Transcoded,
};

const GO: ControlFlow<()> = ControlFlow::Continue(());

/// The callbacks (libxml2's `xmlSAXHandler` fields); each does nothing by
/// default, and returning [`ControlFlow::Break`] stops the parse.
#[allow(unused_variables)]
pub trait Handler {
    /// The document starts, after its XML declaration (`startDocument`).
    fn start_document(&mut self, declaration: Option<&Declaration<'_>>) -> ControlFlow<()> {
        GO
    }
    /// The DOCTYPE, before its declarations (`internalSubset`).
    fn internal_subset(&mut self, doctype: &Doctype<'_>) -> ControlFlow<()> {
        GO
    }
    /// The end of the internal subset, before any external subset's
    /// declarations (`externalSubset`), called for every DOCTYPE.
    fn external_subset(&mut self, doctype: &Doctype<'_>) -> ControlFlow<()> {
        GO
    }
    /// A parsed entity declaration, general or parameter (`entityDecl`).
    fn entity_decl(&mut self, entity: valid::Entity<'_>) -> ControlFlow<()> {
        GO
    }
    /// An unparsed entity declaration (`unparsedEntityDecl`).
    fn unparsed_entity_decl(&mut self, entity: valid::Entity<'_>) -> ControlFlow<()> {
        GO
    }
    /// A notation declaration (`notationDecl`).
    fn notation_decl(&mut self, notation: valid::Notation<'_>) -> ControlFlow<()> {
        GO
    }
    /// An `<!ELEMENT>` declaration (`elementDecl`).
    fn element_decl(&mut self, element: valid::Element<'_>) -> ControlFlow<()> {
        GO
    }
    /// One attribute of an `<!ATTLIST>` declaration (`attributeDecl`).
    fn attribute_decl(&mut self, attribute: valid::Attribute<'_>) -> ControlFlow<()> {
        GO
    }
    /// A start tag, its namespace declarations among the attributes
    /// (`startElementNs`); an empty-element tag is followed by `end_element`.
    fn start_element(&mut self, element: &Element<'_>) -> ControlFlow<()> {
        GO
    }
    /// An end tag (`endElementNs`).
    fn end_element(&mut self, name: &Name<'_>) -> ControlFlow<()> {
        GO
    }
    /// Character data, references substituted (`characters`).
    fn characters(&mut self, text: &str) -> ControlFlow<()> {
        GO
    }
    /// A CDATA section's content (`cdataBlock`); text under `NOCDATA`.
    fn cdata_block(&mut self, text: &str) -> ControlFlow<()> {
        GO
    }
    /// A comment (`comment`).
    fn comment(&mut self, text: &str) -> ControlFlow<()> {
        GO
    }
    /// A processing instruction (`processingInstruction`).
    fn processing_instruction(&mut self, target: &str, data: &str) -> ControlFlow<()> {
        GO
    }
    /// A general entity reference that is not substituted (`reference`).
    fn reference(&mut self, name: &str) -> ControlFlow<()> {
        GO
    }
    /// A kept warning, [`ErrorKind::Warning`] (`warning`).
    fn warning(&mut self, error: &Error) {}
    /// A kept error the parse continues after (`error`).
    fn error(&mut self, error: &Error) {}
    /// The error that ends the parse (`fatalError`).
    fn fatal_error(&mut self, error: &Error) {}
    /// The document ends, also after a fatal error (`endDocument`); like
    /// `start_document`, not called for empty input.
    fn end_document(&mut self) {}
}

/// Parses `input`, in any encoding the [`Transcoded`] input takes, calling
/// `handler` (`xmlSAXUserParseMemory`, `xmlCtxtReadMemory` on an
/// `xmlNewSAXParserCtxt`).
///
/// # Errors
///
/// The error that ended the parse, after `fatal_error` and `end_document`
/// (empty input calls neither document callback); offsets count the UTF-8
/// text. A decoding or option error is returned
/// before any callback.
pub fn parse(input: &[u8], options: ParseOptions, handler: &mut impl Handler) -> Result<(), Error> {
    let text = Transcoded::new(input, &options)?;
    let mut reader = Reader::transcoded(&text, options)?;
    let mut sax = Sax::default();
    let outcome = sax.drive(&mut reader, handler);
    if outcome == Err(None) {
        return Ok(());
    }
    sax.errors(&reader, handler);
    // libxml2 starts every document but an empty one ("Document is empty").
    let document = !text.as_bytes().is_empty();
    if document && !sax.started {
        let _ = handler.start_document(None);
    }
    if let Err(Some(error)) = &outcome {
        handler.fatal_error(error);
    }
    if document {
        handler.end_document();
    }
    outcome.or_else(|error| error.map_or(Ok(()), Err))
}

/// `Err(None)`: a callback stopped the parse.
type Step = Result<(), Option<Error>>;

fn go(flow: ControlFlow<()>) -> Step {
    match flow {
        ControlFlow::Continue(()) => Ok(()),
        ControlFlow::Break(()) => Err(None),
    }
}

/// What has been reported: kept errors, and declarations by kind
/// (`elements` = whether its `<!ELEMENT>` was, and how many attributes).
#[derive(Default)]
struct Sax {
    started: bool,
    errors: usize,
    entities: usize,
    notations: usize,
    elements: Vec<(bool, usize)>,
    /// The root's start tag was read: the DTD is complete.
    rooted: bool,
    /// Depth inside entity references that are reported as `reference`.
    skipped: usize,
}

impl Sax {
    fn drive(&mut self, reader: &mut Reader<'_>, handler: &mut impl Handler) -> Step {
        while let Some(event) = reader.read_event()? {
            self.errors(reader, handler);
            if !self.started {
                self.started = true;
                let declaration = match &event {
                    Event::Declaration(declaration) => Some(declaration),
                    _ => None,
                };
                go(handler.start_document(declaration))?;
            }
            if self.skipped > 0 {
                match event {
                    Event::EntityStart(_) => self.skipped += 1,
                    Event::EntityEnd => self.skipped -= 1,
                    _ => {}
                }
                continue;
            }
            go(match &event {
                Event::Declaration(_) | Event::EntityEnd => GO,
                Event::Doctype(doctype) => {
                    go(handler.internal_subset(doctype))?;
                    self.declarations(reader.dtd(), handler)?;
                    handler.external_subset(doctype)
                }
                Event::Start(element) | Event::Empty(element) => {
                    if !self.rooted {
                        self.rooted = true;
                        self.declarations(reader.dtd(), handler)?;
                    }
                    go(handler.start_element(element))?;
                    match event {
                        Event::Empty(_) => handler.end_element(&element.name),
                        _ => GO,
                    }
                }
                Event::End(name) => handler.end_element(name),
                Event::Text(text) => handler.characters(text),
                Event::Cdata(text) => handler.cdata_block(text),
                Event::Comment(text) => handler.comment(text),
                Event::ProcessingInstruction { target, data } => {
                    handler.processing_instruction(target, data)
                }
                Event::EntityStart(name) => {
                    self.skipped = 1;
                    handler.reference(name)
                }
            })?;
        }
        Ok(())
    }

    fn errors(&mut self, reader: &Reader<'_>, handler: &mut impl Handler) {
        for error in &reader.errors()[self.errors..] {
            match error.kind {
                ErrorKind::Warning(_) => handler.warning(error),
                _ => handler.error(error),
            }
        }
        self.errors = reader.errors().len();
    }

    /// Reports the declarations `dtd` gained since the last call.
    fn declarations(&mut self, dtd: Option<&Dtd>, handler: &mut impl Handler) -> Step {
        let Some(dtd) = dtd else {
            return Ok(());
        };
        for entity in dtd.entities().skip(self.entities) {
            self.entities += 1;
            go(match entity.kind {
                EntityKind::ExternalUnparsedGeneral => handler.unparsed_entity_decl(entity),
                _ => handler.entity_decl(entity),
            })?;
        }
        for notation in dtd.notations().skip(self.notations) {
            self.notations += 1;
            go(handler.notation_decl(notation))?;
        }
        self.elements.resize(dtd.elements.len(), (false, 0));
        for (element, (declared, attributes)) in dtd.elements().zip(&mut self.elements) {
            if !*declared && element.kind != ContentKind::Undefined {
                *declared = true;
                go(handler.element_decl(element))?;
            }
            for attribute in element.attributes().skip(*attributes) {
                *attributes += 1;
                go(handler.attribute_decl(attribute))?;
            }
        }
        Ok(())
    }
}
