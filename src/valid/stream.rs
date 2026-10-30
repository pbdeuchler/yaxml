//! [`StreamValidator`]: DTD validation of parser events.

use super::engine::{Node, Validator};
use super::{Code, Diagnostic, Dtd};
use crate::xpath::tree::QName;
use crate::{Event, SourceOrigin};

/// A DTD validator for one document fed as events, like libxml2's reader
/// with `XML_PARSER_VALIDATE` (`xmllint --stream --valid`).
///
/// Feed it every event of a [`Reader`](crate::Reader),
/// [`PushParser`](crate::PushParser) or
/// [`BufferedReader`](crate::BufferedReader) with its origin, then
/// [`finish`](Self::finish). Memory is one frame per open element plus the
/// IDs and IDREFs, bounded by the parser's [`Limits`](crate::Limits).
///
/// Diagnostics arrive in document order as soon as they are known, with
/// `node` the element's index in document order. They match
/// [`Document::validate`](crate::Document::validate), except that content
/// model mismatches use libxml2's push wording (`Misplaced b`, `Text not
/// allowed`, `Expecting more children`) at the child or end tag that shows
/// them, and a missing or empty DTD is `Validation failed: no DTD found !`.
///
/// The DTD is the one [`set_dtd`](Self::set_dtd) supplies, normally the
/// reader's [`dtd`](crate::BufferedReader::dtd); without one, the internal
/// subset of the DOCTYPE event.
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
    /// The element's name, then each attribute's: the qualified name and the namespace.
    names: Vec<(String, Option<String>)>,
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

    /// Supplies the document's DTD as its reader holds it; call it after
    /// every event, as [`TextReader`](crate::TextReader) does. Declarations
    /// are taken until the document element is checked (the external subset
    /// loads while that tag is read); later calls only add the validity
    /// errors the parser has recorded since.
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
    #[cfg(feature = "textreader")]
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
                    (
                        name.qualified().to_owned(),
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
                            .map(|attribute| (QName::of(&attribute.name), &*attribute.value));
                        validator.start(dtd, node, QName::of(&element.name), attributes);
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

fn owned((qualified, namespace): &(String, Option<String>)) -> QName<'_> {
    QName::new((qualified, namespace.as_deref()))
}
