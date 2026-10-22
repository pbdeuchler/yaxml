//! libxml2 2.15.4's legacy HTML tree construction (`HTMLparser.c`: htmlAutoClose,
//! htmlAutoCloseOnClose, htmlCheckImplied, htmlStartCharData, areBlanks and the
//! tree parts of htmlParseStartTag/EndTag/ElementInternal), reporting to a
//! [`Sink`] in the shape of libxml2's SAX callbacks. [`Document`] is the SAX2
//! tree builder (`xmlSAX2StartElement`, `xmlSAX2Characters`, ...); [`Events`]
//! hands the callbacks to the caller. This is not the WHATWG tree builder:
//! there is no adoption agency, and misnested end tags close by priority.
//! Under `HTML_PARSE_HTML5` every tag is reported as it is read, unbalanced.
//!
//! Every scan of the open-element stack is bounded: names map to their stack
//! positions, so membership and the end-tag priority check take constant time
//! and each element is popped once.

use std::collections::HashMap;
use std::ops::ControlFlow;

use super::tables::{self, Mode};
use super::tokenizer::{Attribute, Doctype, TagEnd, Token, blank};
use super::{Diagnostic, DiagnosticKind, Event};
use crate::diagnostic::{Domain, Level};
use crate::owned::{HtmlBuilder, LastChild};
use crate::{Error, NodeId, OwnedDocument};

/// libxml2's SAX callbacks as tree construction calls them. `mark` is a text
/// offset on the line libxml2's input is on at the call (its `input->line`),
/// so lines are counted only when asked for.
pub(super) trait Sink {
    /// An element the sink created (`ctxt->node`); `()` without a tree.
    type Node: Copy;
    /// The `#root` element fragments are parsed below.
    fn root(&self) -> Self::Node;
    /// `startElement` below `parent` with `depth` elements open; None stops
    /// parsing (the tree's depth ceiling, nodePush's).
    fn start(
        &mut self,
        parent: Option<Self::Node>,
        depth: usize,
        name: &str,
        attributes: &[Attribute],
        mark: usize,
    ) -> Result<Option<Self::Node>, Error>;
    /// `endElement`, which ends `node`, the SAX2 builder's current element.
    fn end(&mut self, node: Option<Self::Node>, name: &str);
    /// Completes an element no callback ends (parsing stopped, or HTML5).
    fn close(&mut self, node: Self::Node);
    /// `characters`, or `cdataBlock` for raw text, in `parent`; false stops
    /// parsing (the tree's text ceiling). NOBLANKS installs a no-op
    /// `ignorableWhitespace`, so ignorable blanks never arrive.
    fn text(
        &mut self,
        parent: Option<Self::Node>,
        text: &str,
        cdata: bool,
        mark: usize,
    ) -> Result<bool, Error>;
    fn comment(&mut self, parent: Option<Self::Node>, text: &str, mark: usize)
    -> Result<(), Error>;
    /// `internalSubset`, for every DOCTYPE; `first` before any content.
    fn doctype(&mut self, doctype: Doctype, first: bool);
    /// The end of areBlanks, after its checks of the current name `top`;
    /// without a tree libxml2 keeps the text.
    fn blanks(&self, parent: Option<Self::Node>, top: &str) -> bool;
    /// The caller stopped parsing (`xmlStopParser`).
    fn stopped(&self) -> bool;
}

/// `ctxt->html`: whether a head or body was opened yet.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
enum Insert {
    Initial,
    InHead,
    InBody,
}

/// What the tokenizer does next.
pub(super) enum Reaction {
    Continue,
    /// After a start tag: read the new current element's content in `Mode`.
    Enter(Mode),
    /// A discarded start tag left no current element: htmlParseElementInternal
    /// stops before the tag's last bytes, which are then read as text.
    Reread(usize),
    /// Parsing stopped: a resource limit (XML_ERR_RESOURCE_LIMIT) or the caller.
    Halt,
}

pub(super) struct Settings {
    pub(super) implied: bool,
    pub(super) keep_blanks: bool,
    pub(super) report: bool,
    /// `HTML_PARSE_HTML5`: no implied elements, auto-closing or balancing.
    pub(super) html5: bool,
}

/// Where libxml2's input is during the current token: text offsets of its
/// start (after skipped blanks) and end.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Place {
    pub(super) start: usize,
    pub(super) end: usize,
}

/// The elements whose end-tag priority exceeds the default (htmlEndPriority):
/// only they can block a misplaced end tag.
const PRIORITIZED: [&str; 11] = [
    "div", "td", "th", "tr", "thead", "tbody", "tfoot", "table", "head", "body", "html",
];
const HEAD: usize = 8;
const BODY: usize = 9;

/// The index of a name in [`PRIORITIZED`].
fn prioritized(name: &str) -> Option<usize> {
    Some(match name {
        "div" => 0,
        "td" => 1,
        "th" => 2,
        "tr" => 3,
        "thead" => 4,
        "tbody" => 5,
        "tfoot" => 6,
        "table" => 7,
        "head" => HEAD,
        "body" => BODY,
        "html" => 10,
        _ => return None,
    })
}

/// libxml2 reports at most 100 errors (`XML_MAX_ERRORS`), resource limits aside.
const MAX_ERRORS: usize = 100;

pub(super) struct Tree<S: Sink> {
    pub(super) sink: S,
    /// The open names (htmlnamePush) and, outside HTML5, their elements,
    /// which are then also the SAX2 builder's node stack.
    stack: Vec<(String, S::Node)>,
    /// Under HTML5 the SAX2 builder's node stack, which end tags pop
    /// whatever their name while the name stack keeps one entry.
    nodes: Vec<S::Node>,
    positions: HashMap<String, Vec<usize>>,
    /// The stack positions of the [`PRIORITIZED`] elements, which end tags
    /// and implied elements consult without hashing.
    prioritized: [Vec<usize>; 11],
    insert: Insert,
    /// Misplaced html/head/body start tags whose end tags are ignored.
    discarded: usize,
    settings: Settings,
    halted: bool,
    errors: usize,
    pub(super) at: Place,
    /// Where the current character-data run began: libxml2 updates its line
    /// only after a run, so the run's callbacks report this one.
    pub(super) run: Option<usize>,
    pub(super) diagnostics: Vec<Diagnostic>,
}

impl<S: Sink> Tree<S> {
    pub(super) fn new(sink: S, settings: Settings) -> Self {
        Self {
            sink,
            stack: Vec::new(),
            nodes: Vec::new(),
            positions: HashMap::new(),
            prioritized: Default::default(),
            insert: Insert::Initial,
            discarded: 0,
            settings,
            halted: false,
            errors: 0,
            at: Place::default(),
            run: None,
            diagnostics: Vec::new(),
        }
    }

    /// htmlCtxtParseContentInternal: content below a `#root` element.
    pub(super) fn fragment(&mut self) {
        self.positions.insert("#root".to_owned(), vec![0]);
        self.stack.push(("#root".to_owned(), self.sink.root()));
    }

    /// Records a diagnostic raised in `raised` (domain and level) at a text
    /// offset, with the offset whose line libxml2 reports.
    pub(super) fn report(
        &mut self,
        kind: DiagnosticKind,
        offset: usize,
        mark: usize,
        message: String,
        raised: (Domain, Level),
    ) {
        if kind != DiagnosticKind::ResourceLimit {
            if self.errors == MAX_ERRORS {
                return;
            }
            self.errors += 1;
        }
        if self.settings.report {
            let diagnostic = Diagnostic::new(kind, offset, mark, message, raised);
            self.diagnostics.push(diagnostic);
        }
    }

    /// An `htmlParseErr`.
    fn error(&mut self, kind: DiagnosticKind, offset: usize, mark: usize, message: String) {
        self.report(kind, offset, mark, message, (Domain::Html, Level::Error));
    }

    pub(super) fn consume(&mut self, token: Token<'_>) -> Result<Reaction, Error> {
        let at = self.at;
        if let Token::Text { text, mode, next } = token {
            let mark = *self.run.get_or_insert(at.start);
            let done = self.char_data(&text, mode, next, mark);
            return done.map(|()| self.reaction(Reaction::Continue));
        }
        self.run = None;
        let reaction = match token {
            Token::Text { .. } => Ok(()),
            Token::Literal(text) => self
                .start_char_data(at.start)
                .and_then(|()| self.text(text, false, at.start)),
            Token::StartTag {
                name,
                name_end,
                attributes,
                end,
            } => return self.start_tag(&name, name_end, &attributes, end),
            Token::EndTag(name) => self.end_tag(&name),
            Token::Comment(text) => self.comment(&text),
            Token::Doctype { mut doctype, first } => {
                if self.settings.html5
                    && let Some(name) = &mut doctype.name
                {
                    name.make_ascii_lowercase();
                }
                if !self.halted {
                    self.sink.doctype(doctype, first);
                }
                Ok(())
            }
        };
        reaction.map(|()| self.reaction(Reaction::Continue))
    }

    fn reaction(&self, reaction: Reaction) -> Reaction {
        if self.halted || self.sink.stopped() {
            Reaction::Halt
        } else {
            reaction
        }
    }

    fn located(&self, error: Error) -> Error {
        Error::new(error.kind, self.at.start)
    }

    /// XML_ERR_RESOURCE_LIMIT at a text offset: parsing stops.
    pub(super) fn halt(
        &mut self,
        offset: usize,
        mark: usize,
        message: String,
        raised: (Domain, Level),
    ) {
        if !self.halted {
            self.halted = true;
            self.report(DiagnosticKind::ResourceLimit, offset, mark, message, raised);
        }
    }

    #[inline]
    fn top(&self) -> Option<&str> {
        self.stack.last().map(|(name, _)| name.as_str())
    }

    /// The SAX2 builder's current element (`ctxt->node`).
    #[inline]
    fn current(&self) -> Option<S::Node> {
        if self.settings.html5 {
            self.nodes.last().copied()
        } else {
            self.stack.last().map(|&(_, node)| node)
        }
    }

    /// `ctxt->nodeNr`.
    #[inline]
    fn depth(&self) -> usize {
        if self.settings.html5 {
            self.nodes.len()
        } else {
            self.stack.len()
        }
    }

    pub(super) fn top_name(&self) -> &str {
        self.top().unwrap_or("")
    }

    /// How many `head` or `body` elements are open.
    fn open(&self, index: usize) -> usize {
        self.prioritized[index].len()
    }

    /// htmlnamePush plus the SAX startElement callback, which libxml2 makes
    /// with its input at text offset `offset` on `mark`.
    fn push(
        &mut self,
        name: &str,
        attributes: &[Attribute],
        offset: usize,
        mark: usize,
    ) -> Result<(), Error> {
        if self.insert < Insert::InHead && name == "head" {
            self.insert = Insert::InHead;
        }
        if self.insert < Insert::InBody && name == "body" {
            self.insert = Insert::InBody;
        }
        if self.halted {
            return Ok(());
        }
        let (current, depth) = (self.current(), self.depth());
        let started = self.sink.start(current, depth, name, attributes, mark);
        let Some(node) = started.map_err(|error| self.located(error))? else {
            let message =
                format!("Excessive depth in document: {depth}, use XML_PARSE_HUGE option");
            self.halt(offset, mark, message, (Domain::Parser, Level::Fatal));
            return Ok(());
        };
        if self.settings.html5 {
            self.nodes.push(node);
        }
        let position = self.stack.len();
        match self.positions.get_mut(name) {
            Some(positions) => positions.push(position),
            None => drop(self.positions.insert(name.to_owned(), vec![position])),
        }
        if let Some(index) = prioritized(name) {
            self.prioritized[index].push(position);
        }
        self.stack.push((name.to_owned(), node));
        Ok(())
    }

    /// htmlnamePop without a callback.
    #[inline]
    fn pop_name(&mut self) -> Option<(String, S::Node)> {
        let (name, node) = self.stack.pop()?;
        if let Some(positions) = self.positions.get_mut(&name) {
            positions.pop();
        }
        if let Some(index) = prioritized(&name) {
            self.prioritized[index].pop();
        }
        Some((name, node))
    }

    /// The SAX endElement callback plus htmlnamePop.
    #[inline]
    fn pop(&mut self) {
        if let Some((name, node)) = self.pop_name() {
            if self.halted {
                self.sink.close(node);
            } else {
                self.sink.end(Some(node), &name);
            }
        }
    }

    /// htmlAutoClose: pop elements the new start tag implicitly ends.
    fn auto_close(&mut self, new: &str) {
        while self.top().is_some_and(|top| tables::closes(new, top)) {
            self.pop();
        }
    }

    /// htmlCheckImplied: open html, head or body before `new` as needed.
    fn check_implied(&mut self, new: &str, offset: usize, mark: usize) -> Result<(), Error> {
        if !self.settings.implied || self.settings.html5 || new == "html" {
            return Ok(());
        }
        if self.stack.is_empty() {
            self.push("html", &[], offset, mark)?;
        }
        if new == "body" || new == "head" {
            return Ok(());
        }
        let head_content = matches!(new, "script" | "style" | "meta" | "link" | "title" | "base");
        if self.stack.len() <= 1 && head_content {
            if self.insert < Insert::InHead {
                self.push("head", &[], offset, mark)?;
            }
        } else if !matches!(new, "noframes" | "frame" | "frameset")
            && self.insert < Insert::InBody
            && self.open(BODY) == 0
            && self.open(HEAD) == 0
        {
            self.push("body", &[], offset, mark)?;
        }
        Ok(())
    }

    /// htmlStartCharData: non-blank text ends the head and implies a body.
    fn start_char_data(&mut self, mark: usize) -> Result<(), Error> {
        if !self.settings.implied || self.settings.html5 {
            return Ok(());
        }
        if self.top() == Some("head") {
            self.auto_close("p");
        }
        self.check_implied("p", self.at.start, mark)
    }

    /// A character-data callback, unless parsing stopped.
    #[inline]
    fn text(&mut self, text: &str, cdata: bool, mark: usize) -> Result<(), Error> {
        if self.halted || text.is_empty() {
            return Ok(());
        }
        let current = self.current();
        if !self
            .sink
            .text(current, text, cdata, mark)
            .map_err(|error| self.located(error))?
        {
            let message = "Text node too long, try XML_PARSE_HUGE".to_owned();
            self.halt(self.at.end, mark, message, (Domain::Html, Level::Fatal));
        }
        Ok(())
    }

    /// htmlCharDataSAXCallback.
    fn char_data(
        &mut self,
        mut text: &str,
        mode: Mode,
        next: u8,
        mark: usize,
    ) -> Result<(), Error> {
        if mode.raw() {
            return self.text(text, true, mark);
        }
        if matches!(self.top(), None | Some("html" | "head")) {
            let blanks = text.bytes().take_while(|&c| blank(c)).count();
            if self.settings.keep_blanks {
                self.text(&text[..blanks], false, mark)?;
            }
            text = &text[blanks..];
            if text.is_empty() {
                return Ok(());
            }
            self.start_char_data(mark)?;
        }
        if mode == Mode::Data && !self.settings.keep_blanks && self.ignorable(text, next) {
            return Ok(());
        }
        self.text(text, false, mark)
    }

    /// areBlanks: whether blank text is ignorable under NOBLANKS.
    fn ignorable(&self, text: &str, next: u8) -> bool {
        if !text.bytes().all(blank) {
            return false;
        }
        if next == 0 {
            return true;
        }
        if next != b'<' {
            return false;
        }
        match self.top() {
            None | Some("html" | "head") => true,
            Some(top) => self.sink.blanks(self.current(), top),
        }
    }

    fn comment(&mut self, text: &str) -> Result<(), Error> {
        if self.halted {
            return Ok(());
        }
        self.sink
            .comment(self.current(), text, self.at.end)
            .map_err(|error| self.located(error))
    }

    /// The tree parts of htmlParseStartTag and htmlParseElementInternal.
    fn start_tag(
        &mut self,
        name: &str,
        name_end: usize,
        attributes: &[Attribute],
        end: TagEnd,
    ) -> Result<Reaction, Error> {
        let at = self.at;
        // Implied elements and misplaced tags are reported after the name,
        // the element where its attributes end.
        let named = at.start + name_end;
        let tag_end = at.end - if end == TagEnd::SelfClosing { 2 } else { 1 };
        let mut misplaced = 0;
        if !self.settings.html5 {
            self.auto_close(name);
            self.check_implied(name, named, at.start)?;
            misplaced = match name {
                "html" => usize::from(!self.stack.is_empty()),
                "head" => usize::from(self.stack.len() != 1),
                _ => 0,
            };
            if name == "body" {
                misplaced += self.open(BODY);
            }
            for _ in 0..misplaced {
                let message = format!("htmlParseStartTag: misplaced <{name}> tag");
                self.error(DiagnosticKind::MisplacedTag, named, at.start, message);
            }
            self.discarded += misplaced;
        }
        if end == TagEnd::Eof {
            return Ok(self.reaction(Reaction::Continue));
        }
        if misplaced == 0 {
            if self.settings.html5 {
                self.pop_name();
            }
            self.push(name, attributes, tag_end, at.end)?;
        }
        // htmlParseElementInternal acts on the current element, which is the
        // previous one when the tag was discarded.
        let Some(top) = self.top() else {
            return Ok(self.reaction(Reaction::Reread(at.end - tag_end)));
        };
        let info = tables::element(top);
        let mode = if end == TagEnd::SelfClosing || info.is_some_and(|info| info.empty) {
            if self.settings.html5 {
                self.pop_name();
            } else {
                self.pop();
            }
            Mode::Data
        } else {
            info.map_or(Mode::Data, |info| info.mode)
        };
        Ok(self.reaction(Reaction::Enter(mode)))
    }

    /// The tree part of htmlParseEndTag, with htmlAutoCloseOnClose. libxml2
    /// reports its errors after the tag.
    fn end_tag(&mut self, name: &str) -> Result<(), Error> {
        if self.settings.html5 {
            if !self.halted {
                let node = self.nodes.pop();
                self.sink.end(node, name);
            }
            return Ok(());
        }
        let (offset, mark) = (self.at.end, self.at.end);
        if self.discarded > 0 && matches!(name, "html" | "body" | "head") {
            self.discarded -= 1;
            return Ok(());
        }
        let Some(&position) = self
            .positions
            .get(name)
            .and_then(|positions| positions.last())
        else {
            let message = format!("Unexpected end tag : {name}");
            self.error(DiagnosticKind::UnexpectedEndTag, offset, mark, message);
            return Ok(());
        };
        // A misplaced end tag only closes elements of lower or equal priority.
        let priority = tables::end_priority(name);
        let blocked = PRIORITIZED
            .iter()
            .zip(&self.prioritized)
            .filter(|(blocker, _)| tables::end_priority(blocker) > priority)
            .filter_map(|(_, positions)| positions.last())
            .any(|&blocker| blocker > position);
        if !blocked {
            while self.stack.len() > position + 1 {
                if let Some(top) = self.top()
                    && tables::element(top).is_some_and(|info| info.end_tag == 3)
                {
                    let message = format!("Opening and ending tag mismatch: {name} and {top}");
                    self.error(DiagnosticKind::TagMismatch, offset, mark, message);
                }
                self.pop();
            }
        }
        match self.top() {
            Some(top) if top == name => self.pop(),
            top => {
                let message = format!(
                    "Opening and ending tag mismatch: {name} and {}",
                    top.unwrap_or("(null)")
                );
                self.error(DiagnosticKind::TagMismatch, offset, mark, message);
            }
        }
        Ok(())
    }

    /// htmlAutoCloseOnEnd (not under HTML5). Returns the sink, the
    /// diagnostics and whether parsing stopped early.
    pub(super) fn finish(mut self) -> (S, Vec<Diagnostic>, bool) {
        if !self.settings.html5 {
            while !self.stack.is_empty() {
                self.pop();
            }
        }
        while let Some(node) = self.nodes.pop() {
            self.sink.close(node);
        }
        let stopped = self.halted || self.sink.stopped();
        (self.sink, self.diagnostics, stopped)
    }
}

/// The SAX2 tree builder: libxml2's default HTML callbacks.
pub(super) struct Document {
    builder: HtmlBuilder,
    max_depth: usize,
    text_limit: usize,
    fragment: bool,
    doctype: Option<Doctype>,
}

impl Document {
    pub(super) fn new(builder: HtmlBuilder, max_depth: usize, text_limit: usize) -> Self {
        Self {
            builder,
            max_depth,
            text_limit,
            fragment: false,
            doctype: None,
        }
    }

    /// Fragment content goes below the document node, as below
    /// htmlCtxtParseContentInternal's `#root`; a DOCTYPE is ignored there.
    pub(super) fn fragment(mut self) -> Self {
        self.fragment = true;
        self
    }

    /// xmlSAX2EndDocument and htmlParseDocument's default DOCTYPE.
    pub(super) fn finish(
        self,
        default_doctype: bool,
        encoding: Option<String>,
        decoding: super::Decoding,
    ) -> Result<OwnedDocument, Error> {
        let implied = crate::Doctype {
            name: "html",
            public_id: Some("-//W3C//DTD HTML 4.0 Transitional//EN"),
            system_id: Some("http://www.w3.org/TR/REC-html40/loose.dtd"),
            internal_subset: None,
        };
        let doctype = match &self.doctype {
            Some(declared) => Some(crate::Doctype {
                name: declared.name.as_deref().unwrap_or(""),
                public_id: declared.public_id.as_deref(),
                system_id: declared.system_id.as_deref(),
                internal_subset: None,
            }),
            None => (default_doctype && !self.fragment).then_some(implied),
        };
        self.builder.finish(doctype, encoding, decoding)
    }
}

impl Sink for Document {
    type Node = NodeId;

    fn root(&self) -> NodeId {
        NodeId::new(0)
    }

    #[inline]
    fn start(
        &mut self,
        parent: Option<NodeId>,
        depth: usize,
        name: &str,
        attributes: &[Attribute],
        mark: usize,
    ) -> Result<Option<NodeId>, Error> {
        if depth >= self.max_depth {
            return Ok(None);
        }
        let parent = parent.unwrap_or(NodeId::new(0));
        self.builder
            .element(parent, name, attributes, mark)
            .map(Some)
    }

    #[inline]
    fn end(&mut self, node: Option<NodeId>, _: &str) {
        if let Some(node) = node {
            self.builder.close(node);
        }
    }

    #[inline]
    fn close(&mut self, node: NodeId) {
        self.builder.close(node);
    }

    /// xmlSAX2Characters / xmlSAX2CDataBlock: dropped outside any element.
    #[inline]
    fn text(
        &mut self,
        parent: Option<NodeId>,
        text: &str,
        cdata: bool,
        mark: usize,
    ) -> Result<bool, Error> {
        let Some(parent) = parent else {
            return Ok(true);
        };
        self.builder
            .text(parent, text, cdata, self.text_limit, mark)
    }

    #[inline]
    fn comment(&mut self, parent: Option<NodeId>, text: &str, mark: usize) -> Result<(), Error> {
        let parent = parent.unwrap_or(NodeId::new(0));
        self.builder.comment(parent, text, mark)
    }

    /// xmlSAX2InternalSubset keeps a DOCTYPE only before content.
    fn doctype(&mut self, doctype: Doctype, first: bool) {
        if first && !self.fragment {
            self.doctype = Some(doctype);
        }
    }

    fn blanks(&self, parent: Option<NodeId>, top: &str) -> bool {
        let Some(id) = parent else {
            return false;
        };
        let strict = self
            .doctype
            .as_ref()
            .and_then(|d| d.public_id.as_deref())
            .is_some_and(|id| {
                id.eq_ignore_ascii_case("-//W3C//DTD HTML 4.01//EN")
                    || id.eq_ignore_ascii_case("-//W3C//DTD HTML 4//EN")
            });
        if top == "body" && strict {
            return true;
        }
        match self.builder.last_child(id) {
            LastChild::None => !tables::allows_text(top),
            LastChild::Text => false,
            LastChild::Element(child) => !tables::allows_text(child),
            LastChild::Other => true,
        }
    }

    fn stopped(&self) -> bool {
        false
    }
}

/// Custom SAX callbacks: each one becomes an [`Event`] for the handler.
pub(super) struct Events<F> {
    handler: F,
    stopped: bool,
}

impl<F: FnMut(Event<'_>) -> ControlFlow<()>> Events<F> {
    pub(super) fn new(handler: F) -> Self {
        Self {
            handler,
            stopped: false,
        }
    }

    fn emit(&mut self, event: Event<'_>) {
        if !self.stopped {
            self.stopped = (self.handler)(event).is_break();
        }
    }
}

impl<F: FnMut(Event<'_>) -> ControlFlow<()>> Sink for Events<F> {
    type Node = ();

    fn root(&self) {}

    fn start(
        &mut self,
        _: Option<()>,
        _: usize,
        name: &str,
        attributes: &[Attribute],
        _: usize,
    ) -> Result<Option<()>, Error> {
        self.emit(Event::Start { name, attributes });
        Ok(Some(()))
    }

    fn end(&mut self, _: Option<()>, name: &str) {
        self.emit(Event::End(name));
    }

    fn close(&mut self, (): ()) {}

    #[inline]
    fn text(&mut self, _: Option<()>, text: &str, cdata: bool, _: usize) -> Result<bool, Error> {
        self.emit(if cdata {
            Event::Cdata(text)
        } else {
            Event::Text(text)
        });
        Ok(true)
    }

    fn comment(&mut self, _: Option<()>, text: &str, _: usize) -> Result<(), Error> {
        self.emit(Event::Comment(text));
        Ok(())
    }

    fn doctype(&mut self, doctype: Doctype, _: bool) {
        self.emit(Event::Doctype(crate::Doctype {
            name: doctype.name.as_deref().unwrap_or(""),
            public_id: doctype.public_id.as_deref(),
            system_id: doctype.system_id.as_deref(),
            internal_subset: None,
        }));
    }

    fn blanks(&self, _: Option<()>, _: &str) -> bool {
        false
    }

    fn stopped(&self) -> bool {
        self.stopped
    }
}
