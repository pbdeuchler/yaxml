//! [`StreamValidator`]: DTD validation of parser events.

use super::engine::{Node, Validator};
use super::{Code, Diagnostic, Dtd};
use crate::xpath::tree::QName;
use crate::{Event, SourceOrigin};

/// Validates one document against its DTD from its parser events, as
/// libxml2's reader does with `XML_PARSER_VALIDATE` (`xmllint --stream
/// --valid`): feed it every event of a [`Reader`](crate::Reader),
/// [`PushParser`](crate::PushParser) or [`BufferedReader`](crate::BufferedReader)
/// with its origin, then [`finish`](Self::finish). Memory is one frame (a
/// declaration and a content-model state) per open element, the ID values and
/// the IDREF attributes, all bounded by the parser's
/// [`Limits`](crate::Limits).
///
/// Diagnostics come in document order, as soon as they are known; `node` is
/// the element's index in document order. They are the tree's
/// ([`Document::validate`](crate::Document::validate)), except that a content
/// model mismatch is worded as libxml2's push validator words it (`Misplaced
/// b`, `Text not allowed`, `Expecting more children`) and reported at the
/// child or end tag that shows it, and a DTD that declares nothing is
/// libxml2's `Validation failed: no DTD found !`, as the parser reports it.
///
/// The DTD is the one [`set_dtd`](Self::set_dtd) supplies: a reader's
/// [`dtd`](crate::BufferedReader::dtd), with the external subset it loaded and
/// the validity errors the parser finds (standalone and undeclared-entity
/// checks). Without one, the internal subset of the DOCTYPE event is used.
///
/// ```
/// use yaxml::{BufferedReader, valid::{Code, StreamValidator}};
///
/// let input = &b"<!DOCTYPE r [<!ELEMENT r (a)><!ELEMENT a EMPTY>]><r><b/></r>"[..];
/// let mut reader = BufferedReader::new(input);
/// let mut validator = StreamValidator::new();
/// while let Some((event, origin)) = reader.read_event()? {
///     validator.event(&event, origin);
///     if let Some(dtd) = reader.dtd() {
///         validator.set_dtd(dtd);
///     }
/// }
/// let diagnostics = validator.finish();
/// assert_eq!(diagnostics[0].code, Code::ContentModel);
/// assert_eq!(
///     diagnostics[0].message,
///     "Element r content does not follow the DTD, Misplaced b"
/// );
/// assert_eq!(diagnostics[1].code, Code::UnknownElement);
/// # Ok::<(), yaxml::ReadError>(())
/// ```
pub struct StreamValidator {
    validator: Validator<usize>,
    dtd: Option<Dtd>,
    /// `set_dtd` supplied `dtd`.
    supplied: bool,
    root: Option<Root>,
    /// No DTD: reported at the document element, and nothing is checked.
    off: bool,
}

/// The document element's start tag, kept until the DTD is complete: a
/// parser loads the external subset while it reads this tag, so the tag is
/// checked against the DTD passed after its event.
struct Root {
    /// The element's name, then each attribute's: the qualified name, where
    /// the local part starts, and the namespace.
    names: Vec<(String, usize, Option<String>)>,
    values: Vec<String>,
    offset: Option<usize>,
    empty: bool,
}

impl Default for StreamValidator {
    fn default() -> Self {
        Self {
            validator: Validator::new(true),
            dtd: None,
            supplied: false,
            root: None,
            off: false,
        }
    }
}

impl StreamValidator {
    /// A validator for one document.
    pub fn new() -> Self {
        Self::default()
    }

    /// Supplies the document's DTD, as its reader holds it after an event.
    /// Until the document element's start tag is checked, each call copies
    /// the declarations (a parser loads the external subset while reading
    /// that tag); afterwards a call only reports the validity errors the
    /// parser recorded in the DTD since. So a caller passes its reader's DTD
    /// after every event, as [`TextReader`](crate::TextReader) does.
    pub fn set_dtd(&mut self, dtd: &Dtd) {
        if self.validator.elements() > 0 {
            return self.validator.issues(dtd);
        }
        self.dtd = Some(dtd.clone());
        self.supplied = true;
        self.release();
    }

    /// [`set_dtd`](Self::set_dtd) with a reader's DTD after an event; a reader
    /// without one after the document element's start tag has none.
    pub(crate) fn sync(&mut self, dtd: Option<&Dtd>) {
        match dtd {
            Some(dtd) => self.set_dtd(dtd),
            None => self.release(),
        }
    }

    /// Validates the next event of the document.
    pub fn event(&mut self, event: &Event<'_>, origin: SourceOrigin) {
        self.release();
        let Self { validator, dtd, .. } = self;
        match event {
            _ if self.off => {}
            Event::Doctype(doctype) if !self.supplied => {
                let subset = doctype.internal_subset.unwrap_or_default();
                *dtd = Dtd::parse(subset.as_bytes()).ok().map(|dtd| Dtd {
                    name: Some(doctype.name.to_owned()),
                    ..dtd
                });
            }
            Event::Start(element) | Event::Empty(element) if validator.elements() == 0 => {
                let name = |name: &crate::Name<'_>| {
                    let qualified = name.qualified();
                    let local = qualified.len() - name.local().len();
                    (
                        qualified.to_owned(),
                        local,
                        name.namespace().map(str::to_owned),
                    )
                };
                let attributes = element.attributes.iter();
                self.root = Some(Root {
                    names: (std::iter::once(name(&element.name)))
                        .chain(attributes.clone().map(|attribute| name(&attribute.name)))
                        .collect(),
                    values: attributes.map(|a| a.value.clone().into_owned()).collect(),
                    offset: origin.input_range().map(|range| range.start),
                    empty: matches!(event, Event::Empty(_)),
                });
            }
            _ => {
                let Some(dtd) = dtd.as_ref() else {
                    return;
                };
                match event {
                    Event::Start(element) | Event::Empty(element) => {
                        let node = Node {
                            id: validator.elements(),
                            offset: origin.input_range().map(|range| range.start),
                        };
                        let attributes = (element.attributes.iter())
                            .map(|attribute| (qname(&attribute.name), &*attribute.value));
                        validator.start(dtd, node, qname(&element.name), attributes);
                        if matches!(event, Event::Empty(_)) {
                            validator.end(dtd);
                        }
                    }
                    Event::End(_) => validator.end(dtd),
                    Event::Text(text) => validator.text(dtd, text, false),
                    Event::Cdata(text) => validator.text(dtd, text, true),
                    Event::Comment(_) | Event::ProcessingInstruction { .. } => validator.other(dtd),
                    Event::EntityStart(_) => validator.other(dtd),
                    Event::EntityEnd => validator.flush(dtd),
                    Event::Declaration(_) | Event::Doctype(_) => {}
                }
            }
        }
    }

    /// Checks the document element's start tag against the DTD now held.
    fn release(&mut self) {
        let Some(root) = self.root.take() else {
            return;
        };
        let Some(dtd) = self.dtd.as_ref().filter(|dtd| declares(dtd)) else {
            let message = "Validation failed: no DTD found !".to_owned();
            self.validator.note(Code::NoDtd, root.offset, message);
            self.off = true;
            return;
        };
        self.validator.begin(dtd);
        let attributes =
            (root.names[1..].iter().map(owned)).zip(root.values.iter().map(String::as_str));
        let node = Node {
            id: 0,
            offset: root.offset,
        };
        self.validator
            .start(dtd, node, owned(&root.names[0]), attributes);
        if root.empty {
            self.validator.end(dtd);
        }
    }

    /// The diagnostics so far; complete once the document element ends.
    pub fn diagnostics(&self) -> &[Diagnostic<usize>] {
        self.validator.diagnostics()
    }

    /// The diagnostics, once the document has ended.
    pub fn finish(mut self) -> Vec<Diagnostic<usize>> {
        self.release();
        self.validator.finish()
    }
}

/// Whether the DTD declares anything, as libxml2's parser requires to validate.
fn declares(dtd: &Dtd) -> bool {
    !(dtd.elements.is_empty()
        && dtd.notations.is_empty()
        && dtd.general.is_empty()
        && dtd.unbound.is_empty())
}

fn owned((qualified, local, namespace): &(String, usize, Option<String>)) -> QName<'_> {
    QName {
        qualified,
        local: &qualified[*local..],
        namespace: namespace.as_deref().filter(|uri| !uri.is_empty()),
    }
}

fn qname<'a>(name: &'a crate::Name<'_>) -> QName<'a> {
    QName {
        qualified: name.qualified(),
        local: name.local(),
        namespace: name.namespace().filter(|uri| !uri.is_empty()),
    }
}
