//! libxml2 2.15.4's legacy HTML tree construction (`HTMLparser.c`: htmlAutoClose,
//! htmlAutoCloseOnClose, htmlCheckImplied, htmlStartCharData, areBlanks and the
//! tree parts of htmlParseStartTag/EndTag/ElementInternal), feeding the SAX2
//! builder callbacks. This is not the WHATWG tree builder: there is no
//! adoption agency, and misnested end tags close by priority.
//!
//! Every scan of the open-element stack is bounded: names map to their stack
//! positions, so membership and the end-tag priority check take constant time
//! and each element is popped once.

use std::collections::HashMap;

use super::tables::{self, Mode};
use super::tokenizer::{Attribute, TagEnd, Token, blank};
use super::{Diagnostic, DiagnosticKind, Doctype};
use crate::owned::{HtmlBuilder, LastChild};
use crate::{Error, NodeId, OwnedDocument};

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
    /// A resource limit stopped parsing, as XML_ERR_RESOURCE_LIMIT does.
    Halt,
}

pub(super) struct Settings {
    pub(super) implied: bool,
    pub(super) keep_blanks: bool,
    pub(super) report: bool,
    pub(super) max_depth: usize,
    pub(super) text_limit: usize,
}

pub(super) struct Tree {
    builder: HtmlBuilder,
    stack: Vec<(String, NodeId)>,
    positions: HashMap<String, Vec<usize>>,
    insert: Insert,
    /// Misplaced html/head/body start tags whose end tags are ignored.
    discarded: usize,
    settings: Settings,
    halted: bool,
    pub(super) offset: usize,
    pub(super) doctype: Option<Doctype>,
    pub(super) diagnostics: Vec<Diagnostic>,
}

impl Tree {
    pub(super) fn new(builder: HtmlBuilder, settings: Settings) -> Self {
        Self {
            builder,
            stack: Vec::new(),
            positions: HashMap::new(),
            insert: Insert::Initial,
            discarded: 0,
            settings,
            halted: false,
            offset: 0,
            doctype: None,
            diagnostics: Vec::new(),
        }
    }

    pub(super) fn report(&mut self, kind: DiagnosticKind) {
        if self.settings.report {
            let offset = self.offset;
            self.diagnostics.push(Diagnostic { kind, offset });
        }
    }

    pub(super) fn consume(&mut self, token: Token<'_>) -> Result<Reaction, Error> {
        let reaction = match token {
            Token::Text { text, mode, next } => self.char_data(&text, mode, next),
            Token::Literal(text) => self
                .start_char_data()
                .and_then(|()| self.append_text(text, false)),
            Token::StartTag {
                name,
                attributes,
                end,
            } => return self.start_tag(&name, &attributes, end),
            Token::EndTag(name) => self.end_tag(&name),
            Token::Comment(text) => self.comment(&text),
            Token::Doctype { doctype, first } => {
                if first {
                    self.doctype = Some(doctype);
                }
                Ok(())
            }
        };
        reaction.map(|()| self.reaction(Reaction::Continue))
    }

    fn reaction(&self, reaction: Reaction) -> Reaction {
        if self.halted {
            Reaction::Halt
        } else {
            reaction
        }
    }

    fn located(&self, error: Error) -> Error {
        Error::new(error.kind, self.offset)
    }

    pub(super) fn halt(&mut self) {
        if !self.halted {
            self.halted = true;
            self.report(DiagnosticKind::ResourceLimit);
        }
    }

    fn top(&self) -> Option<&str> {
        self.stack.last().map(|(name, _)| name.as_str())
    }

    pub(super) fn top_name(&self) -> &str {
        self.top().unwrap_or("")
    }

    fn node(&self) -> NodeId {
        self.stack.last().map_or(NodeId(0), |(_, id)| *id)
    }

    fn open(&self, name: &str) -> usize {
        self.positions.get(name).map_or(0, Vec::len)
    }

    /// htmlnamePush plus the SAX2 startElement callback.
    fn push(&mut self, name: &str, attributes: &[Attribute]) -> Result<(), Error> {
        if self.insert < Insert::InHead && name == "head" {
            self.insert = Insert::InHead;
        }
        if self.insert < Insert::InBody && name == "body" {
            self.insert = Insert::InBody;
        }
        if self.halted {
            return Ok(());
        }
        if self.stack.len() >= self.settings.max_depth {
            self.halt();
            return Ok(());
        }
        // xmlSAX2HtmlAttribute: a valueless boolean attribute gets its name.
        let defaulted: Vec<Attribute>;
        let attributes = if attributes
            .iter()
            .any(|(name, value)| value.is_none() && tables::boolean_attribute(name))
        {
            defaulted = attributes
                .iter()
                .map(|(name, value)| {
                    let value = value
                        .clone()
                        .or_else(|| tables::boolean_attribute(name).then(|| name.clone()));
                    (name.clone(), value)
                })
                .collect();
            &defaulted
        } else {
            attributes
        };
        let parent = self.node();
        let id = self
            .builder
            .element(parent, name, attributes)
            .map_err(|error| self.located(error))?;
        let position = self.stack.len();
        match self.positions.get_mut(name) {
            Some(positions) => positions.push(position),
            None => drop(self.positions.insert(name.to_owned(), vec![position])),
        }
        self.stack.push((name.to_owned(), id));
        Ok(())
    }

    /// The SAX2 endElement callback plus htmlnamePop.
    fn pop(&mut self) {
        if let Some((name, id)) = self.stack.pop() {
            if let Some(positions) = self.positions.get_mut(&name) {
                positions.pop();
            }
            self.builder.close(id);
        }
    }

    /// htmlAutoClose: pop elements the new start tag implicitly ends.
    fn auto_close(&mut self, new: &str) {
        while self.top().is_some_and(|top| tables::closes(new, top)) {
            self.pop();
        }
    }

    /// htmlCheckImplied: open html, head or body before `new` as needed.
    fn check_implied(&mut self, new: &str) -> Result<(), Error> {
        if !self.settings.implied || new == "html" {
            return Ok(());
        }
        if self.stack.is_empty() {
            self.push("html", &[])?;
        }
        if new == "body" || new == "head" {
            return Ok(());
        }
        let head_content = matches!(new, "script" | "style" | "meta" | "link" | "title" | "base");
        if self.stack.len() <= 1 && head_content {
            if self.insert < Insert::InHead {
                self.push("head", &[])?;
            }
        } else if !matches!(new, "noframes" | "frame" | "frameset")
            && self.insert < Insert::InBody
            && self.open("body") == 0
            && self.open("head") == 0
        {
            self.push("body", &[])?;
        }
        Ok(())
    }

    /// htmlStartCharData: non-blank text ends the head and implies a body.
    fn start_char_data(&mut self) -> Result<(), Error> {
        if !self.settings.implied {
            return Ok(());
        }
        if self.top() == Some("head") {
            self.auto_close("p");
        }
        self.check_implied("p")
    }

    /// xmlSAX2Characters / xmlSAX2CDataBlock: dropped outside any element.
    fn append_text(&mut self, text: &str, cdata: bool) -> Result<(), Error> {
        if self.halted || text.is_empty() || self.stack.is_empty() {
            return Ok(());
        }
        let parent = self.node();
        let appended = self
            .builder
            .text(parent, text, cdata, self.settings.text_limit)
            .map_err(|error| self.located(error))?;
        if !appended {
            self.halt();
        }
        Ok(())
    }

    /// htmlCharDataSAXCallback.
    fn char_data(&mut self, mut text: &str, mode: Mode, next: u8) -> Result<(), Error> {
        if mode.raw() {
            return self.append_text(text, true);
        }
        if matches!(self.top(), None | Some("html" | "head")) {
            let blanks = text.bytes().take_while(|&c| blank(c)).count();
            if blanks > 0 && self.settings.keep_blanks {
                self.append_text(&text[..blanks], false)?;
            }
            text = &text[blanks..];
            if text.is_empty() {
                return Ok(());
            }
            self.start_char_data()?;
        }
        if mode == Mode::Data && !self.settings.keep_blanks && self.ignorable(text, next) {
            return Ok(());
        }
        self.append_text(text, false)
    }

    /// areBlanks: whether blank text may be dropped under NOBLANKS.
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
        let Some((name, id)) = self.stack.last() else {
            return true;
        };
        let strict = self
            .doctype
            .as_ref()
            .and_then(|d| d.public_id.as_deref())
            .is_some_and(|id| {
                id.eq_ignore_ascii_case("-//W3C//DTD HTML 4.01//EN")
                    || id.eq_ignore_ascii_case("-//W3C//DTD HTML 4//EN")
            });
        if name == "html" || name == "head" || name == "body" && strict {
            return true;
        }
        match self.builder.last_child(*id) {
            LastChild::None => !tables::allows_text(name),
            LastChild::Text => false,
            LastChild::Element(child) => !tables::allows_text(child),
            LastChild::Other => true,
        }
    }

    fn comment(&mut self, text: &str) -> Result<(), Error> {
        if self.halted {
            return Ok(());
        }
        let parent = self.node();
        self.builder
            .comment(parent, text)
            .map_err(|error| self.located(error))
    }

    /// The tree parts of htmlParseStartTag and htmlParseElementInternal.
    fn start_tag(
        &mut self,
        name: &str,
        attributes: &[Attribute],
        end: TagEnd,
    ) -> Result<Reaction, Error> {
        self.auto_close(name);
        self.check_implied(name)?;
        let mut misplaced = match name {
            "html" => usize::from(!self.stack.is_empty()),
            "head" => usize::from(self.stack.len() != 1),
            _ => 0,
        };
        if name == "body" {
            misplaced += self.open("body");
        }
        for _ in 0..misplaced {
            self.report(DiagnosticKind::MisplacedTag);
        }
        self.discarded += misplaced;
        if end == TagEnd::Eof {
            return Ok(self.reaction(Reaction::Continue));
        }
        if misplaced == 0 {
            self.push(name, attributes)?;
        }
        // htmlParseElementInternal acts on the current element, which is the
        // previous one when the tag was discarded.
        let Some(top) = self.top() else {
            return Ok(self.reaction(Reaction::Enter(Mode::Data)));
        };
        let info = tables::element(top);
        let mode = if end == TagEnd::SelfClosing || info.is_some_and(|info| info.empty) {
            self.pop();
            Mode::Data
        } else {
            info.map_or(Mode::Data, |info| info.mode)
        };
        Ok(self.reaction(Reaction::Enter(mode)))
    }

    /// The tree part of htmlParseEndTag, with htmlAutoCloseOnClose.
    fn end_tag(&mut self, name: &str) -> Result<(), Error> {
        if self.discarded > 0 && matches!(name, "html" | "body" | "head") {
            self.discarded -= 1;
            return Ok(());
        }
        let Some(&position) = self
            .positions
            .get(name)
            .and_then(|positions| positions.last())
        else {
            self.report(DiagnosticKind::UnexpectedEndTag);
            return Ok(());
        };
        // A misplaced end tag only closes elements of lower or equal priority.
        let priority = tables::end_priority(name);
        let blocked = [
            "div", "td", "th", "tr", "thead", "tbody", "tfoot", "table", "head", "body", "html",
        ]
        .iter()
        .filter(|blocker| tables::end_priority(blocker) > priority)
        .filter_map(|blocker| self.positions.get(*blocker)?.last())
        .any(|&blocker| blocker > position);
        if !blocked {
            while self.stack.len() > position + 1 {
                if self
                    .top()
                    .and_then(tables::element)
                    .is_some_and(|info| info.end_tag == 3)
                {
                    self.report(DiagnosticKind::TagMismatch);
                }
                self.pop();
            }
        }
        if self.top() == Some(name) {
            self.pop();
        } else {
            self.report(DiagnosticKind::TagMismatch);
        }
        Ok(())
    }

    /// htmlAutoCloseOnEnd and xmlSAX2EndDocument.
    pub(super) fn finish(
        mut self,
        default_doctype: bool,
        encoding: Option<String>,
    ) -> Result<(OwnedDocument, Vec<Diagnostic>, bool), Error> {
        while !self.stack.is_empty() {
            self.pop();
        }
        let doctype = self.doctype.take().or_else(|| {
            default_doctype.then(|| Doctype {
                name: Some("html".to_owned()),
                public_id: Some("-//W3C//DTD HTML 4.0 Transitional//EN".to_owned()),
                system_id: Some("http://www.w3.org/TR/REC-html40/loose.dtd".to_owned()),
            })
        });
        let document = self.builder.finish(doctype, encoding)?;
        Ok((document, self.diagnostics, self.halted))
    }
}
