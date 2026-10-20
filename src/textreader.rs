//! libxml2's `xmlTextReader` cursor, built on the bounded [`BufferedReader`].

use std::collections::VecDeque;
use std::io::BufRead;
use std::ops::RangeInclusive;

use crate::options::Flags;
use crate::valid::Dtd;
use crate::{
    BufferedReader, DecodeLimits, Error, ErrorKind, Event, Limit, Limits, Name, OwnedDocument,
    ParseOptions, ReadError, ResourceKind, SourceOrigin,
};
use crate::{relaxng, xsd};

const XML: &str = "http://www.w3.org/XML/1998/namespace";
/// Recycled node buffers kept after a subtree expansion drains.
const SPARE_NODES: usize = 4;

/// libxml2 `xmlReaderTypes` values for the nodes this reader produces;
/// `node_type as i32` is the C constant. Entities, documents, fragments,
/// notations, insignificant whitespace, entity ends and XML declarations are
/// never reported; entity references are, unless `NOENT` substitutes them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum NodeType {
    None = 0,
    Element = 1,
    Attribute = 2,
    Text = 3,
    Cdata = 4,
    /// A kept entity reference (no `NOENT`); its content is not visited.
    EntityReference = 5,
    ProcessingInstruction = 7,
    Comment = 8,
    DocumentType = 10,
    /// Whitespace-only text. libxml2's reader reports all of it as
    /// significant, whatever `xml:space` says.
    SignificantWhitespace = 14,
    EndElement = 15,
}

/// libxml2 `xmlTextReaderMode` values; `as i32` is the C constant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ReadState {
    Initial = 0,
    Interactive = 1,
    Error = 2,
    Eof = 3,
}

/// libxml2 `xmlParserProperties` values; `as i32` is the C constant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParserProperty {
    /// Load the external subset: a resolver was supplied and the options load it.
    LoadDtd = 1,
    /// Insert DTD default attributes: `DTDATTR`.
    DefaultAttributes = 2,
    /// DTD validation: not implemented, always false.
    Validate = 3,
    /// Substitute entity references: `NOENT`.
    SubstituteEntities = 4,
}

/// Byte range in a node's or scope's string pool.
type Span = (usize, usize);

#[derive(Clone, Copy, Default)]
struct QName {
    qualified: Span,
    /// Prefix byte length; zero when unprefixed.
    prefix: usize,
    namespace: Option<Span>,
}

#[derive(Clone, Copy)]
struct Attr {
    name: QName,
    value: Span,
}

#[derive(Clone, Copy, Default, Eq, PartialEq)]
enum Kind {
    #[default]
    Element,
    Empty,
    End,
    Text,
    Whitespace,
    Cdata,
    Comment,
    Pi,
    Doctype,
    EntityRef,
}

/// One logical node with its strings in a reusable pool. Namespace
/// declarations precede ordinary attributes, as in libxml2's `nsDef` list.
#[derive(Default)]
struct Node {
    kind: Kind,
    text: String,
    /// Element, PI target or DOCTYPE name.
    name: QName,
    value: Span,
    attributes: Vec<Attr>,
    declarations: usize,
}

fn get(pool: &str, (start, end): Span) -> &str {
    &pool[start..end]
}

fn push(pool: &mut String, value: &str) -> Span {
    let start = pool.len();
    pool.push_str(value);
    (start, pool.len())
}

impl Node {
    fn get(&self, span: Span) -> &str {
        get(&self.text, span)
    }

    fn push(&mut self, value: &str) -> Span {
        push(&mut self.text, value)
    }

    fn qname(&mut self, name: &Name<'_>) -> QName {
        QName {
            qualified: self.push(name.qualified()),
            prefix: name.prefix().map_or(0, str::len),
            namespace: name.namespace().map(|namespace| self.push(namespace)),
        }
    }

    fn qualified(&self, name: QName) -> &str {
        self.get(name.qualified)
    }

    fn prefix(&self, name: QName) -> Option<&str> {
        (name.prefix > 0).then(|| &self.qualified(name)[..name.prefix])
    }

    fn local(&self, name: QName) -> &str {
        &self.qualified(name)[name.prefix + usize::from(name.prefix > 0)..]
    }

    fn namespace(&self, name: QName) -> Option<&str> {
        name.namespace.map(|span| self.get(span))
    }

    /// Fills this node from an event; false for the unreported XML declaration.
    fn load(&mut self, event: &Event<'_>, merge: bool) -> bool {
        if !merge {
            self.text.clear();
            self.attributes.clear();
            self.declarations = 0;
        }
        self.kind = match event {
            Event::Declaration(_) => return false,
            Event::Doctype(doctype) => {
                self.name = QName {
                    qualified: self.push(doctype.name),
                    ..QName::default()
                };
                Kind::Doctype
            }
            Event::Start(element) | Event::Empty(element) => {
                self.name = self.qname(&element.name);
                for declarations in [true, false] {
                    for attribute in &element.attributes {
                        let name = &attribute.name;
                        if declarations
                            == (name.prefix() == Some("xmlns") || name.qualified() == "xmlns")
                        {
                            let name = self.qname(name);
                            let value = self.push(&attribute.value);
                            self.attributes.push(Attr { name, value });
                        }
                    }
                    if declarations {
                        self.declarations = self.attributes.len();
                    }
                }
                if matches!(event, Event::Start(_)) {
                    Kind::Element
                } else {
                    Kind::Empty
                }
            }
            Event::End(name) => {
                self.name = self.qname(name);
                Kind::End
            }
            Event::Text(value) | Event::Cdata(value) | Event::Comment(value) => {
                let (start, end) = self.push(value);
                self.value = (if merge { self.value.0 } else { start }, end);
                match event {
                    Event::Text(_) => Kind::Text,
                    Event::Cdata(_) => Kind::Cdata,
                    _ => Kind::Comment,
                }
            }
            Event::ProcessingInstruction { target, data } => {
                self.name = QName {
                    qualified: self.push(target),
                    ..QName::default()
                };
                self.value = self.push(data);
                Kind::Pi
            }
            Event::EntityStart(name) => {
                self.name = QName {
                    qualified: self.push(name),
                    ..QName::default()
                };
                Kind::EntityRef
            }
            Event::EntityEnd => return false,
        };
        true
    }
}

struct Frame {
    text: usize,
    bindings: usize,
    /// Inherited `xml:lang`.
    lang: Option<Span>,
    /// This element's own `xml:base`.
    base: Option<Span>,
    attributes: bool,
}

/// Namespace bindings, `xml:lang` and `xml:base` of the open elements.
#[derive(Default)]
struct Scope {
    text: String,
    bindings: Vec<(Option<Span>, Span)>,
    frames: Vec<Frame>,
}

impl Scope {
    fn get(&self, span: Span) -> &str {
        get(&self.text, span)
    }

    fn push(&mut self, node: &Node) {
        let mut frame = Frame {
            text: self.text.len(),
            bindings: self.bindings.len(),
            lang: self.frames.last().and_then(|frame| frame.lang),
            base: None,
            attributes: !node.attributes.is_empty(),
        };
        for (index, attribute) in node.attributes.iter().enumerate() {
            let (prefix, local) = (node.prefix(attribute.name), node.local(attribute.name));
            let value = node.get(attribute.value);
            if index < node.declarations {
                let prefix = prefix.map(|_| push(&mut self.text, local));
                let uri = push(&mut self.text, value);
                self.bindings.push((prefix, uri));
            } else if prefix == Some("xml") && local == "lang" {
                frame.lang = Some(push(&mut self.text, value));
            } else if prefix == Some("xml") && local == "base" {
                frame.base = Some(push(&mut self.text, value));
            }
        }
        self.frames.push(frame);
    }

    fn pop(&mut self) {
        if let Some(frame) = self.frames.pop() {
            self.text.truncate(frame.text);
            self.bindings.truncate(frame.bindings);
        }
    }

    /// `xmlSearchNs`: the innermost binding, with `xml` always bound.
    fn lookup(&self, prefix: Option<&str>) -> Option<&str> {
        if prefix == Some("xml") {
            return Some(XML);
        }
        let binding = self.bindings.iter().rev();
        binding
            .map(|&(bound, uri)| (bound.map(|span| self.get(span)), uri))
            .find(|(bound, _)| *bound == prefix)
            .map(|(_, uri)| self.get(uri))
    }
}

/// A forward-only cursor over the nodes of a document, as libxml2's
/// `xmlTextReader` (`xmlreader.h`), over any [`BufRead`] including `&[u8]`.
///
/// Node types, names, values, depths and attribute order follow libxml2:
/// namespace declarations are attributes listed before ordinary ones, an
/// empty element (`<a/>`) has no `EndElement`, adjacent text (including
/// substituted replacement text) is one node, and the XML declaration and
/// whitespace outside the root are not reported. Without options an entity
/// reference is an `EntityReference` node and no DTD defaults are added;
/// see [`with_options`](Self::with_options). String getters borrow the reader
/// until it moves.
///
/// Memory is that of the [`BufferedReader`] plus one node of lookahead,
/// except that [`expand`](Self::expand) and the `read_*` methods buffer the
/// current subtree, within `Limits::max_owned_bytes` of node text.
///
/// ```
/// use yaxml::{NodeType, TextReader};
/// let mut reader = TextReader::new(b"<r a='1'><i>text</i><e/></r>".as_slice());
/// let mut seen = Vec::new();
/// while reader.read()? {
///     seen.push((reader.depth(), reader.node_type(), reader.name().unwrap().to_owned()));
/// }
/// assert_eq!(seen[0], (0, NodeType::Element, "r".to_owned()));
/// assert_eq!(seen[2], (2, NodeType::Text, "#text".to_owned()));
/// assert_eq!(seen.len(), 6);
/// # Ok::<(), yaxml::ReadError>(())
/// ```
pub struct TextReader<R: BufRead> {
    source: BufferedReader<R>,
    limits: Limits,
    url: Option<String>,
    resolver: bool,
    flags: Flags,
    state: ReadState,
    /// The current node first, then lookahead and buffered subtree nodes.
    queue: VecDeque<Node>,
    spare: Vec<Node>,
    /// Pool bytes held by `queue`.
    buffered: usize,
    /// An I/O failure met while completing the current text node.
    pending: Option<ReadError>,
    attribute: Option<usize>,
    scope: Scope,
    /// Validation of every event read from the source.
    validators: Validators,
}

impl<R: BufRead> TextReader<R> {
    pub fn new(input: R) -> Self {
        Self::with_limits(input, Limits::default())
    }

    pub fn with_limits(input: R, limits: Limits) -> Self {
        Self::with_decode_limits(input, limits, DecodeLimits::default())
    }

    pub fn with_decode_limits(input: R, limits: Limits, decode_limits: DecodeLimits) -> Self {
        let options = ParseOptions::default().with_limits(limits);
        Self::with_options(input, options.with_decode_limits(decode_limits))
            .expect("convenience flags are supported")
    }

    /// A reader with libxml2 parser options, like `xmlReaderForMemory`'s
    /// `options`; see [`crate::options`]. `base` is the document URI.
    /// Tree-only flags (`DTDVALID`, `NOBLANKS`, `XINCLUDE`) are `UnsupportedOption`.
    pub fn with_options(input: R, options: ParseOptions) -> Result<Self, Error> {
        // `BufferedReader` rejects the flags a reader does not support.
        let flags = Flags(options.flags);
        let (limits, _) = options.effective_limits();
        let url = options.base.clone();
        let resolver = options.resolver.is_some();
        Ok(Self {
            source: BufferedReader::with_options(input, options)?,
            limits,
            url,
            resolver,
            flags,
            state: ReadState::Initial,
            queue: VecDeque::new(),
            spare: Vec::new(),
            buffered: 0,
            pending: None,
            attribute: None,
            scope: Scope::default(),
            validators: Validators::default(),
        })
    }

    /// `xmlTextReaderRead`: moves to the next node in document order.
    /// Returns false at the end of a well-formed document. Errors are sticky.
    pub fn read(&mut self) -> Result<bool, ReadError> {
        match self.state {
            ReadState::Eof => return Ok(false),
            ReadState::Error => return Err(self.failure()),
            ReadState::Initial | ReadState::Interactive => {}
        }
        self.attribute = None;
        if let Some(node) = self.queue.pop_front() {
            if matches!(node.kind, Kind::Empty | Kind::End) {
                self.scope.pop();
            }
            self.buffered -= node.text.len();
            if self.spare.len() < SPARE_NODES {
                self.spare.push(node);
            }
        }
        match self.fill() {
            Ok(true) => {
                let node = &mut self.queue[0];
                if node.kind == Kind::Text
                    && node
                        .get(node.value)
                        .bytes()
                        .all(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
                {
                    node.kind = Kind::Whitespace;
                }
                if matches!(node.kind, Kind::Element | Kind::Empty) {
                    self.scope.push(node);
                }
                self.state = ReadState::Interactive;
                Ok(true)
            }
            Ok(false) => {
                self.state = ReadState::Eof;
                Ok(false)
            }
            Err(error) => {
                self.state = ReadState::Error;
                Err(error)
            }
        }
    }

    /// `xmlTextReaderNext`: skips the current element's subtree, then reads.
    #[expect(clippy::should_implement_trait, reason = "libxml2's xmlTextReaderNext")]
    pub fn next(&mut self) -> Result<bool, ReadError> {
        if self.node().is_some_and(|node| node.kind == Kind::Element) {
            let depth = self.scope.frames.len();
            while self.read()? {
                if self.queue[0].kind == Kind::End && self.scope.frames.len() == depth {
                    break;
                }
            }
        }
        self.read()
    }

    pub fn read_state(&self) -> ReadState {
        self.state
    }

    /// Makes the front node complete: text is complete once its successor
    /// (or the end) is known not to be more text.
    fn fill(&mut self) -> Result<bool, ReadError> {
        if let Some(error) = self.pending.take() {
            return Err(error);
        }
        if self.queue.is_empty() && !self.fetch()? {
            return Ok(false);
        }
        while self.queue.len() == 1 && self.queue[0].kind == Kind::Text {
            match self.fetch() {
                Ok(true) => {}
                // Deliver the text first. The source repeats an XML failure
                // but reports an I/O failure only once.
                done => {
                    self.pending = done.err().filter(|error| matches!(error, ReadError::Io(_)));
                    break;
                }
            }
        }
        Ok(true)
    }

    /// Appends the next node to the queue, merging adjacent text. False at EOF.
    fn fetch(&mut self) -> Result<bool, ReadError> {
        loop {
            let Some((event, origin)) = self.source.read_event()? else {
                return Ok(false);
            };
            self.validators.event(&event, origin);
            let merge = matches!(event, Event::Text(_))
                && self
                    .queue
                    .back()
                    .is_some_and(|node| node.kind == Kind::Text);
            if !merge {
                self.queue.push_back(self.spare.pop().unwrap_or_default());
            }
            let node = self.queue.back_mut().expect("queued node");
            let before = if merge { node.text.len() } else { 0 };
            if !node.load(&event, merge) {
                self.queue.pop_back();
                continue;
            }
            self.buffered += node.text.len() - before;
            if node.kind == Kind::Doctype
                && let Some(dtd) = self.source.dtd()
            {
                self.validators.set_dtd(dtd);
            }
            if node.kind == Kind::EntityRef {
                self.skip_entity()?;
            }
            return Ok(true);
        }
    }

    /// Skips an entity reference's replacement events, which libxml2's reader
    /// does not visit without `NOENT`, keeping them serialized as the node's
    /// value for [`expand`](Self::expand).
    fn skip_entity(&mut self) -> Result<(), ReadError> {
        let node = self.queue.back_mut().expect("entity reference node");
        let start = node.text.len();
        let mut depth = 0_usize;
        let result = loop {
            let event = match self.source.read_event() {
                Ok(Some((event, origin))) => {
                    self.validators.event(&event, origin);
                    event
                }
                Ok(None) => {
                    let position = self.source.position();
                    break Err(ReadError::Xml(Error::new(
                        ErrorKind::InvalidState,
                        position,
                    )));
                }
                Err(error) => break Err(error),
            };
            match event {
                Event::EntityStart(_) => depth += 1,
                Event::EntityEnd if depth == 0 => break Ok(()),
                Event::EntityEnd => depth -= 1,
                event => write_event(&mut node.text, &event),
            }
        };
        match result {
            Ok(()) => {
                node.value = (start, node.text.len());
                self.buffered += node.text.len() - start;
            }
            Err(_) => node.text.truncate(start),
        }
        result
    }

    fn failure(&mut self) -> ReadError {
        // The source repeats its failure, so the fallback is never taken.
        let error = self.source.read_event().err();
        error.unwrap_or(ReadError::Xml(Error::new(
            ErrorKind::InvalidState,
            self.source.position(),
        )))
    }

    /// Queue index of the current element's `EndElement`, buffering its
    /// subtree; 0 for any other node.
    fn subtree_end(&mut self) -> Result<usize, ReadError> {
        if self.node().is_none_or(|node| node.kind != Kind::Element) {
            return Ok(0);
        }
        let (mut index, mut depth) = (1, 0usize);
        loop {
            if index == self.queue.len() {
                if self.buffered > self.limits.max_owned_bytes {
                    let limit = ErrorKind::LimitExceeded(Limit::OwnedBytes);
                    return Err(ReadError::Xml(Error::new(limit, self.source.position())));
                }
                // The source reports EOF inside an element as an error.
                let fetched = self.fetch();
                if matches!(fetched, Ok(true)) {
                    continue;
                }
                self.state = ReadState::Error;
                return Err(fetched
                    .err()
                    .unwrap_or(ReadError::Xml(Error::new(ErrorKind::InvalidState, 0))));
            }
            match self.queue[index].kind {
                Kind::Element => depth += 1,
                Kind::End if depth == 0 => return Ok(index),
                Kind::End => depth -= 1,
                _ => {}
            }
            index += 1;
        }
    }

    /// `xmlTextReaderReadString`: text of a Text/CDATA node, or the
    /// concatenated descendant text of an element with children.
    pub fn read_string(&mut self) -> Result<Option<String>, ReadError> {
        if self.attribute.is_some() {
            return Ok(None);
        }
        let end = self.subtree_end()?;
        let Some(node) = self.node() else {
            return Ok(None);
        };
        Ok(match node.kind {
            Kind::Text | Kind::Whitespace | Kind::Cdata => Some(node.get(node.value).to_owned()),
            Kind::Element if end > 1 => Some(
                self.queue
                    .range(1..end)
                    .filter(|node| matches!(node.kind, Kind::Text | Kind::Cdata))
                    .map(|node| node.get(node.value))
                    .collect(),
            ),
            _ => None,
        })
    }

    /// `xmlTextReaderReadInnerXml`: the serialized children of the current
    /// node, each with the ancestor namespace declarations it uses.
    pub fn read_inner_xml(&mut self) -> Result<Option<String>, ReadError> {
        let end = self.subtree_end()?;
        if !self.serializable() {
            return Ok(None);
        }
        let (mut out, mut start, mut depth) = (String::new(), 1, 0usize);
        for index in 1..end {
            match self.queue[index].kind {
                Kind::Element => depth += 1,
                Kind::End => depth -= 1,
                _ => {}
            }
            if depth == 0 {
                self.dump(start..=index, false, &mut out);
                start = index + 1;
            }
        }
        Ok(Some(out))
    }

    /// `xmlTextReaderReadOuterXml`: the current node and its subtree, with the
    /// ancestor namespace declarations it uses added to its start tag.
    pub fn read_outer_xml(&mut self) -> Result<Option<String>, ReadError> {
        self.outer_xml(false)
    }

    /// The outer XML, with entity references written as their content when
    /// `expanded`.
    fn outer_xml(&mut self, expanded: bool) -> Result<Option<String>, ReadError> {
        let end = self.subtree_end()?;
        if !self.serializable() {
            return Ok(None);
        }
        let mut out = String::new();
        self.dump(0..=end, expanded, &mut out);
        Ok(Some(out))
    }

    /// `xmlTextReaderExpand` for an element: its subtree as a standalone
    /// document, parsed from [`read_outer_xml`](Self::read_outer_xml) under
    /// this reader's limits. Source ranges are relative to that text. The
    /// cursor does not move; later reads still visit the subtree.
    pub fn expand(&mut self) -> Result<Option<OwnedDocument>, ReadError> {
        if self.element().is_none() {
            return Ok(None);
        }
        let xml = self.outer_xml(true)?.expect("element serializes");
        OwnedDocument::parse_legacy(xml.as_bytes(), self.limits)
            .map(Some)
            .map_err(ReadError::Xml)
    }

    fn serializable(&self) -> bool {
        self.node()
            .is_some_and(|node| !matches!(node.kind, Kind::End | Kind::Doctype))
    }

    /// `xmlNodeDump` of a copy of one queued subtree: namespaces it uses but
    /// does not declare are added to its root in first-use order.
    fn dump(&self, nodes: RangeInclusive<usize>, expanded: bool, out: &mut String) {
        let (mut added, mut declared, mut marks) = (Vec::new(), Vec::new(), Vec::new());
        for node in self.queue.range(nodes.clone()) {
            match node.kind {
                Kind::Element | Kind::Empty => {
                    marks.push(declared.len());
                    let (declarations, attributes) = node.attributes.split_at(node.declarations);
                    declared.extend(declarations.iter().map(|attribute| {
                        node.prefix(attribute.name)
                            .map(|_| node.local(attribute.name))
                    }));
                    for name in std::iter::once(node.name).chain(attributes.iter().map(|a| a.name))
                    {
                        let prefix = node.prefix(name);
                        if let Some(uri) = node.namespace(name)
                            && prefix != Some("xml")
                            && !declared.contains(&prefix)
                            && !added.iter().any(|(bound, _)| *bound == prefix)
                        {
                            added.push((prefix, uri));
                        }
                    }
                    if node.kind == Kind::Empty {
                        declared.truncate(marks.pop().unwrap_or_default());
                    }
                }
                Kind::End => declared.truncate(marks.pop().unwrap_or_default()),
                _ => {}
            }
        }
        let mut open = false;
        for (index, node) in self.queue.range(nodes).enumerate() {
            if open {
                out.push_str(if node.kind == Kind::End { "/>" } else { ">" });
            }
            match node.kind {
                Kind::Element | Kind::Empty => {
                    out.push('<');
                    out.push_str(node.qualified(node.name));
                    let (declarations, attributes) = node.attributes.split_at(node.declarations);
                    for attribute in declarations {
                        write_attribute(
                            out,
                            None,
                            node.qualified(attribute.name),
                            node.get(attribute.value),
                        );
                    }
                    if index == 0 {
                        for (prefix, uri) in &added {
                            match prefix {
                                Some(prefix) => write_attribute(out, Some("xmlns"), prefix, uri),
                                None => write_attribute(out, None, "xmlns", uri),
                            }
                        }
                    }
                    for attribute in attributes {
                        write_attribute(
                            out,
                            None,
                            node.qualified(attribute.name),
                            node.get(attribute.value),
                        );
                    }
                    if node.kind == Kind::Empty {
                        out.push_str("/>");
                    }
                }
                Kind::End if open => {}
                Kind::End => {
                    out.push_str("</");
                    out.push_str(node.qualified(node.name));
                    out.push('>');
                }
                Kind::Text | Kind::Whitespace => escape(out, node.get(node.value), false),
                Kind::Cdata => {
                    out.push_str("<![CDATA[");
                    out.push_str(node.get(node.value));
                    out.push_str("]]>");
                }
                Kind::Comment => {
                    out.push_str("<!--");
                    out.push_str(node.get(node.value));
                    out.push_str("-->");
                }
                Kind::Pi => {
                    out.push_str("<?");
                    out.push_str(node.qualified(node.name));
                    if node.value.0 < node.value.1 {
                        out.push(' ');
                        out.push_str(node.get(node.value));
                    }
                    out.push_str("?>");
                }
                Kind::Doctype => {}
                Kind::EntityRef if expanded => out.push_str(node.get(node.value)),
                Kind::EntityRef => {
                    out.push('&');
                    out.push_str(node.qualified(node.name));
                    out.push(';');
                }
            }
            open = node.kind == Kind::Element;
        }
    }

    fn node(&self) -> Option<&Node> {
        self.queue
            .front()
            .filter(|_| self.state == ReadState::Interactive)
    }

    /// The element whose attributes the cursor can visit, even from one of them.
    fn element(&self) -> Option<&Node> {
        self.node()
            .filter(|node| matches!(node.kind, Kind::Element | Kind::Empty))
    }

    /// The attribute under the cursor, or the current node's own name.
    fn qname(&self) -> Option<(&Node, QName)> {
        let node = self.node()?;
        match (self.attribute, node.kind) {
            (Some(index), _) => Some((node, node.attributes[index].name)),
            (None, Kind::Element | Kind::Empty | Kind::End) => Some((node, node.name)),
            _ => None,
        }
    }

    pub fn node_type(&self) -> NodeType {
        let Some(node) = self.node() else {
            return NodeType::None;
        };
        if self.attribute.is_some() {
            return NodeType::Attribute;
        }
        match node.kind {
            Kind::Element | Kind::Empty => NodeType::Element,
            Kind::End => NodeType::EndElement,
            Kind::Text => NodeType::Text,
            Kind::Whitespace => NodeType::SignificantWhitespace,
            Kind::Cdata => NodeType::Cdata,
            Kind::Comment => NodeType::Comment,
            Kind::Pi => NodeType::ProcessingInstruction,
            Kind::Doctype => NodeType::DocumentType,
            Kind::EntityRef => NodeType::EntityReference,
        }
    }

    /// Qualified name; `#text`, `#cdata-section` or `#comment` for those
    /// nodes; the target of a PI and the name of a DOCTYPE.
    pub fn name(&self) -> Option<&str> {
        if let Some((node, name)) = self.qname() {
            return Some(node.qualified(name));
        }
        let node = self.node()?;
        Some(match node.kind {
            Kind::Text | Kind::Whitespace => "#text",
            Kind::Cdata => "#cdata-section",
            Kind::Comment => "#comment",
            _ => node.qualified(node.name),
        })
    }

    /// Local part of an element or attribute name (`xmlns` or the declared
    /// prefix for a namespace declaration); otherwise [`name`](Self::name).
    pub fn local_name(&self) -> Option<&str> {
        match self.qname() {
            Some((node, name)) => Some(node.local(name)),
            None => self.name(),
        }
    }

    pub fn prefix(&self) -> Option<&str> {
        self.qname().and_then(|(node, name)| node.prefix(name))
    }

    /// Namespace of an element or attribute; the XMLNS namespace for declarations.
    pub fn namespace_uri(&self) -> Option<&str> {
        self.qname().and_then(|(node, name)| node.namespace(name))
    }

    /// Attribute, text, CDATA, comment or PI content. A PI without data has
    /// none, although [`has_value`](Self::has_value) is true.
    pub fn value(&self) -> Option<&str> {
        let node = self.node()?;
        match (self.attribute, node.kind) {
            (Some(index), _) => Some(node.get(node.attributes[index].value)),
            (None, Kind::Text | Kind::Whitespace | Kind::Cdata | Kind::Comment) => {
                Some(node.get(node.value))
            }
            (None, Kind::Pi) => Some(node.get(node.value)).filter(|data| !data.is_empty()),
            _ => None,
        }
    }

    pub fn has_value(&self) -> bool {
        self.node().is_some_and(|node| {
            self.attribute.is_some()
                || matches!(
                    node.kind,
                    Kind::Text | Kind::Whitespace | Kind::Cdata | Kind::Comment | Kind::Pi
                )
        })
    }

    /// Nesting depth: 0 for the document element, +1 on its attributes.
    pub fn depth(&self) -> usize {
        self.node().map_or(0, |node| {
            let element = matches!(node.kind, Kind::Element | Kind::Empty | Kind::End);
            self.scope.frames.len() - usize::from(element) + usize::from(self.attribute.is_some())
        })
    }

    pub fn is_empty_element(&self) -> bool {
        self.attribute.is_none() && self.node().is_some_and(|node| node.kind == Kind::Empty)
    }

    /// True on an element, or its `EndElement`, that has attributes.
    pub fn has_attributes(&self) -> bool {
        self.attribute.is_none()
            && self.node().is_some_and(|node| match node.kind {
                Kind::Element | Kind::Empty => !node.attributes.is_empty(),
                Kind::End => self
                    .scope
                    .frames
                    .last()
                    .is_some_and(|frame| frame.attributes),
                _ => false,
            })
    }

    /// Attributes, including namespace declarations, of the current element;
    /// 0 while on an attribute.
    pub fn attribute_count(&self) -> usize {
        match self.attribute {
            Some(_) => 0,
            None => self.element().map_or(0, |node| node.attributes.len()),
        }
    }

    /// Value of the attribute named by a QName: `xmlns` and `xmlns:p` name
    /// declarations, another prefix resolves in scope. None while on an
    /// attribute, as in libxml2.
    pub fn get_attribute(&self, name: &str) -> Option<&str> {
        self.lookup(self.find(name, true))
    }

    /// Value of the attribute with this local name and namespace (`None`:
    /// no namespace; the XMLNS namespace selects declarations).
    pub fn get_attribute_ns(&self, local: &str, namespace: Option<&str>) -> Option<&str> {
        self.lookup(self.find_ns(local, namespace))
    }

    /// Value of attribute `index`, declarations first.
    pub fn get_attribute_no(&self, index: usize) -> Option<&str> {
        self.lookup(Some(index))
    }

    fn lookup(&self, index: Option<usize>) -> Option<&str> {
        let node = self.element().filter(|_| self.attribute.is_none())?;
        node.attributes
            .get(index?)
            .map(|attribute| node.get(attribute.value))
    }

    /// Index of a QName; `resolve` matches a prefix by its namespace.
    fn find(&self, name: &str, resolve: bool) -> Option<usize> {
        let node = self.element()?;
        let (prefix, local) = name
            .split_once(':')
            .map_or((None, name), |(p, l)| (Some(p), l));
        match prefix {
            Some(prefix) if resolve && prefix != "xmlns" => {
                self.find_ns(local, Some(self.scope.lookup(Some(prefix))?))
            }
            _ => node.attributes.iter().position(|attribute| {
                node.prefix(attribute.name) == prefix && node.local(attribute.name) == local
            }),
        }
    }

    fn find_ns(&self, local: &str, namespace: Option<&str>) -> Option<usize> {
        let node = self.element()?;
        node.attributes.iter().position(|attribute| {
            node.local(attribute.name) == local && node.namespace(attribute.name) == namespace
        })
    }

    fn move_to(&mut self, index: Option<usize>) -> bool {
        if index.is_some() {
            self.attribute = index;
        }
        index.is_some()
    }

    /// Moves to the attribute with this QName, matching prefixes lexically.
    pub fn move_to_attribute(&mut self, name: &str) -> bool {
        self.move_to(self.find(name, false))
    }

    pub fn move_to_attribute_ns(&mut self, local: &str, namespace: &str) -> bool {
        self.move_to(self.find_ns(local, Some(namespace)))
    }

    /// Moves to attribute `index`; out of range, returns to the element.
    pub fn move_to_attribute_no(&mut self, index: usize) -> bool {
        let Some(count) = self.element().map(|node| node.attributes.len()) else {
            return false;
        };
        self.attribute = (index < count).then_some(index);
        self.attribute.is_some()
    }

    pub fn move_to_first_attribute(&mut self) -> bool {
        self.move_to(
            self.element()
                .filter(|node| !node.attributes.is_empty())
                .map(|_| 0),
        )
    }

    pub fn move_to_next_attribute(&mut self) -> bool {
        let next = self.attribute.map_or(0, |index| index + 1);
        self.move_to(
            self.element()
                .filter(|node| next < node.attributes.len())
                .map(|_| next),
        )
    }

    /// Returns from an attribute to its element; false if not on one.
    pub fn move_to_element(&mut self) -> bool {
        self.attribute.take().is_some()
    }

    /// `xmlNodeGetBase`: the innermost `xml:base` resolved against enclosing
    /// ones and the document URI (`http://`, `ftp://` and `urn:` stop early).
    pub fn base_uri(&self) -> Option<String> {
        self.node()?;
        let frames = self.scope.frames.iter().rev();
        let bases = frames.filter_map(|frame| Some(self.scope.get(frame.base?)));
        crate::xpath::tree::resolve_base(bases, self.url.as_deref())
    }

    /// Inherited `xml:lang` of the current node.
    pub fn xml_lang(&self) -> Option<&str> {
        self.node()?;
        let lang = self.scope.frames.last()?.lang?;
        Some(self.scope.get(lang))
    }

    /// Namespace bound to `prefix` (`None`: the default namespace) at the
    /// current node; `xml` is always bound.
    pub fn lookup_namespace(&self, prefix: Option<&str>) -> Option<&str> {
        self.node()?;
        self.scope.lookup(prefix)
    }

    /// `xmlTextReaderGetParserProp`.
    pub fn parser_property(&self, property: ParserProperty) -> bool {
        match property {
            ParserProperty::LoadDtd => self.resolver && self.flags.loads(ResourceKind::DtdSubset),
            ParserProperty::DefaultAttributes => self.flags.default_attributes(),
            ParserProperty::SubstituteEntities => self.flags.substitutes(),
            ParserProperty::Validate => false,
        }
    }

    /// `xmlTextReaderSetParserProp`. Properties are fixed by the options at
    /// construction ([`with_options`](Self::with_options)); setting another
    /// value returns `UnsupportedOption`.
    pub fn set_parser_property(
        &mut self,
        property: ParserProperty,
        value: bool,
    ) -> Result<(), Error> {
        if self.parser_property(property) == value {
            Ok(())
        } else {
            Err(Error::new(
                ErrorKind::UnsupportedOption,
                self.source.position(),
            ))
        }
    }

    /// `xmlTextReaderRelaxNGSetSchema`: validates the document against `schema`
    /// while it is read, or stops validating with `None`. Only before the first
    /// read; afterwards `InvalidState`. `xmlTextReaderRelaxNGValidate` is
    /// [`Schema::parse`](relaxng::Schema::parse) followed by this call.
    pub fn relaxng_set_schema(&mut self, schema: Option<&relaxng::Schema>) -> Result<(), Error> {
        if self.state != ReadState::Initial {
            return Err(Error::new(ErrorKind::InvalidState, self.source.position()));
        }
        self.validators.relaxng = schema.map(relaxng::Schema::stream);
        Ok(())
    }

    /// The RELAX NG diagnostics so far, as [`relaxng::Stream::diagnostics`];
    /// they can concern nodes read ahead of the cursor.
    pub fn relaxng_diagnostics(&self) -> &[relaxng::Diagnostic<usize>] {
        (self.validators.relaxng.as_ref()).map_or(&[], relaxng::Stream::diagnostics)
    }

    /// `xmlTextReaderSetSchema`: validates the document against the XML Schema
    /// `schema` while it is read, alongside any RELAX NG schema, or stops
    /// validating with `None`. Only before the first read; afterwards
    /// `InvalidState`. `xmlTextReaderSchemaValidate` is
    /// [`Schema::parse`](xsd::Schema::parse) followed by this call.
    pub fn xsd_set_schema(&mut self, schema: Option<&xsd::Schema>) -> Result<(), Error> {
        if self.state != ReadState::Initial {
            return Err(Error::new(ErrorKind::InvalidState, self.source.position()));
        }
        self.validators.xsd = schema.map(xsd::StreamValidator::new);
        Ok(())
    }

    /// The XML Schema diagnostics so far, as [`xsd::StreamValidator::diagnostics`];
    /// they can concern nodes read ahead of the cursor.
    pub fn xsd_diagnostics(&self) -> &[xsd::Diagnostic<usize>] {
        (self.validators.xsd.as_ref()).map_or(&[], xsd::StreamValidator::diagnostics)
    }

    /// `xmlTextReaderIsValid`: true when a schema is set and no validity error
    /// was found so far by any validator.
    pub fn is_valid(&self) -> bool {
        self.validators.is_valid()
    }
}

/// The schema validators attached to a reader; each sees every source event.
#[derive(Default)]
struct Validators {
    relaxng: Option<relaxng::Stream>,
    xsd: Option<xsd::StreamValidator>,
}

impl Validators {
    fn event(&mut self, event: &Event<'_>, origin: SourceOrigin) {
        if let Some(validation) = &mut self.relaxng {
            validation.event(event, origin);
        }
        if let Some(validation) = &mut self.xsd {
            validation.event(event, origin);
        }
    }

    fn set_dtd(&mut self, dtd: &Dtd) {
        if let Some(validation) = &mut self.relaxng {
            validation.set_dtd(dtd);
        }
        if let Some(validation) = &mut self.xsd {
            validation.set_dtd(dtd);
        }
    }

    /// True when some validator is attached and none found an error.
    fn is_valid(&self) -> bool {
        let relaxng = self.relaxng.as_ref().map(|v| v.diagnostics().is_empty());
        let xsd = self.xsd.as_ref().map(|v| xsd::is_valid(v.diagnostics()));
        matches!(
            (relaxng, xsd),
            (Some(true), Some(true) | None) | (None, Some(true))
        )
    }
}

/// Serializes one replacement-text event of a skipped entity reference.
fn write_event(out: &mut String, event: &Event<'_>) {
    match event {
        Event::Start(element) | Event::Empty(element) => {
            out.push('<');
            out.push_str(element.name.qualified());
            for attribute in element.attributes.iter() {
                write_attribute(out, None, attribute.name.qualified(), &attribute.value);
            }
            out.push_str(if matches!(event, Event::Empty(_)) {
                "/>"
            } else {
                ">"
            });
        }
        Event::End(name) => {
            out.push_str("</");
            out.push_str(name.qualified());
            out.push('>');
        }
        Event::Text(text) => escape(out, text, false),
        Event::Cdata(text) => {
            out.push_str("<![CDATA[");
            out.push_str(text);
            out.push_str("]]>");
        }
        Event::Comment(text) => {
            out.push_str("<!--");
            out.push_str(text);
            out.push_str("-->");
        }
        Event::ProcessingInstruction { target, data } => {
            out.push_str("<?");
            out.push_str(target);
            out.push(' ');
            out.push_str(data);
            out.push_str("?>");
        }
        _ => {}
    }
}

/// ` prefix:name="value"` with libxml2's attribute escaping.
fn write_attribute(out: &mut String, prefix: Option<&str>, name: &str, value: &str) {
    out.push(' ');
    if let Some(prefix) = prefix {
        out.push_str(prefix);
        out.push(':');
    }
    out.push_str(name);
    out.push_str("=\"");
    escape(out, value, true);
    out.push('"');
}

/// libxml2's content and attribute escaping.
fn escape(out: &mut String, value: &str, attribute: bool) {
    let mut start = 0;
    for (index, byte) in value.bytes().enumerate() {
        let escaped = match byte {
            b'<' => "&lt;",
            b'>' => "&gt;",
            b'&' => "&amp;",
            b'\r' => "&#13;",
            b'"' if attribute => "&quot;",
            b'\n' if attribute => "&#10;",
            b'\t' if attribute => "&#9;",
            _ => continue,
        };
        out.push_str(&value[start..index]);
        out.push_str(escaped);
        start = index + 1;
    }
    out.push_str(&value[start..]);
}
