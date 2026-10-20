//! libxml2 2.15.4's `xmlTextWriter`: a call-by-call writer that auto-closes
//! open start tags, indents, and does not validate names or structure.
//!
//! The same call sequence writes libxml2's bytes and fails where libxml2
//! returns -1, with the same partial output. See the
//! [writer contract](https://github.com/pbdeuchler/yaxml/blob/main/docs/writer.md#libxml2-text-writer-textwriter).
//!
//! ```
//! use yaxml::TextWriter;
//!
//! let mut writer = TextWriter::new(Vec::new());
//! writer.set_indent(true);
//! writer.start_document(None, Some("UTF-8"), None)?;
//! writer.start_element_ns(Some("p"), "root", Some("urn:p"))?;
//! writer.write_attribute("id", "a&b")?;
//! writer.write_element("item", Some("x < y"))?;
//! writer.end_document()?;
//! assert_eq!(
//!     writer.into_inner(),
//!     b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
//!       <p:root id=\"a&amp;b\" xmlns:p=\"urn:p\">\n <item>x &lt; y</item>\n</p:root>\n"
//! );
//! # Ok::<(), yaxml::WriteError>(())
//! ```

use std::io::Write;

use crate::encoding::Output;
use crate::save::{ATTRIBUTE, QUOT, char_reference};
use crate::writer::{flush, write_all};
use crate::{Error, ErrorKind, Limit, WriteError, WriterLimits};

/// `xmlTextWriterState`; `XML_TEXTWRITER_NONE` is an empty stack.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum State {
    Name,
    Attribute,
    Text,
    Pi,
    PiText,
    Cdata,
    Dtd,
    DtdText,
    DtdElement,
    DtdElementText,
    DtdAttlist,
    DtdAttlistText,
    DtdEntity,
    DtdEntityText,
    DtdParameterEntity,
    Comment,
}

/// A node stack entry; `id` stands for libxml2's list link, which namespace
/// entries use to name their element (0 is the empty list's sentinel).
struct Entry {
    name: String,
    state: State,
    id: u64,
}

/// A pending namespace declaration (`xmlTextWriterNsStackEntry`).
struct Namespace {
    /// `xmlns` or `xmlns:prefix`.
    attribute: String,
    uri: String,
    element: u64,
}

type Step = Result<(), ErrorKind>;

/// libxml2's -1 for a call the current state does not allow.
const MISUSE: Step = Err(ErrorKind::InvalidState);

/// The writer state machine: everything but the sink. Each call appends its
/// UTF-8 output to `out`.
struct Machine {
    out: String,
    nodes: Vec<Entry>,
    /// None once libxml2 has deleted its namespace list (an `EndElement`
    /// with nothing open): namespaced calls and start-tag closing then fail.
    namespaces: Option<Vec<Namespace>>,
    ids: u64,
    indent: bool,
    /// `doindent`: the next end tag is indented.
    pending_indent: bool,
    indent_string: String,
    quote: char,
    /// The output converter set by `start_document`; None is UTF-8.
    output: Option<Output>,
    /// A byte-order mark is due before this call's output.
    bom: bool,
    /// `doc->encoding` is set, so attribute values keep non-ASCII characters.
    encoding_named: bool,
    max_depth: usize,
    max_namespaces: usize,
}

impl Machine {
    fn put(&mut self, text: &str) {
        self.out.push_str(text);
    }

    fn put_quote(&mut self) {
        self.out.push(self.quote);
    }

    /// `text` between the quote character.
    fn put_quoted(&mut self, text: &str) {
        self.put_quote();
        self.put(text);
        self.put_quote();
    }

    fn state(&self) -> Option<State> {
        self.nodes.last().map(|entry| entry.state)
    }

    fn set_state(&mut self, state: State) {
        if let Some(entry) = self.nodes.last_mut() {
            entry.state = state;
        }
    }

    fn push(&mut self, name: &str, state: State) -> Step {
        if self.nodes.len() >= self.max_depth {
            return Err(ErrorKind::LimitExceeded(Limit::Depth));
        }
        self.ids += 1;
        let id = self.ids;
        self.nodes.push(Entry {
            name: name.to_owned(),
            state,
            id,
        });
        Ok(())
    }

    /// `xmlTextWriterWriteIndent`: false, writing nothing, on an empty stack.
    fn write_indent(&mut self) -> bool {
        for _ in 1..self.nodes.len() {
            self.out.push_str(&self.indent_string);
        }
        !self.nodes.is_empty()
    }

    /// `xmlSerializeText` with `table`; `hex` escapes non-ASCII as `&#xH;`.
    fn escape(&mut self, text: &str, table: &[Option<&'static str>; 128], hex: bool) {
        crate::save::escape_into(&mut self.out, text, table, hex);
    }

    /// `xmlTextWriterOutputNSDecl`: every pending declaration, newest first.
    fn output_namespaces(&mut self) -> Step {
        let mut namespaces = self.namespaces.take().ok_or(ErrorKind::InvalidState)?;
        while let Some(namespace) = namespaces.pop() {
            // In the NAME state these attribute calls cannot fail.
            self.write_attribute(&namespace.attribute, &namespace.uri)?;
        }
        self.namespaces = Some(namespaces);
        Ok(())
    }

    fn push_namespace(&mut self, attribute: String, uri: &str, element: u64) -> Step {
        let namespaces = self.namespaces.as_mut().ok_or(ErrorKind::InvalidState)?;
        if namespaces.len() >= self.max_namespaces {
            return Err(ErrorKind::LimitExceeded(Limit::Attributes));
        }
        namespaces.push(Namespace {
            attribute,
            uri: uri.to_owned(),
            element,
        });
        Ok(())
    }

    /// Closes an open start tag (an attribute first), as the element,
    /// comment, PI and CDATA starts do; `newline` adds the indent newline.
    fn close_start_tag(&mut self, newline: bool) -> Step {
        if self.state() == Some(State::Attribute) {
            self.end_attribute()?;
        }
        self.output_namespaces()?;
        self.put(">");
        if newline && self.indent {
            self.put("\n");
        }
        self.set_state(State::Text);
        Ok(())
    }

    /// `xmlTextWriterHandleStateDependencies`, before raw content.
    fn handle_state(&mut self) -> Step {
        let (extra, next) = match self.state() {
            Some(State::Name) => {
                self.output_namespaces()?;
                (">", State::Text)
            }
            Some(State::Pi) => (" ", State::PiText),
            Some(State::Dtd) => (" [", State::DtdText),
            Some(State::DtdElement) => (" ", State::DtdElementText),
            Some(State::DtdAttlist) => (" ", State::DtdAttlistText),
            Some(State::DtdEntity | State::DtdParameterEntity) => {
                self.put(" ");
                self.put_quote();
                self.set_state(State::DtdEntityText);
                return Ok(());
            }
            _ => return Ok(()),
        };
        self.put(extra);
        self.set_state(next);
        Ok(())
    }

    fn start_document(
        &mut self,
        version: Option<&str>,
        encoding: Option<&str>,
        standalone: Option<&str>,
    ) -> Step {
        if !self.nodes.is_empty() {
            return MISUSE;
        }
        // `xmlFindCharEncodingHandler`: UTF-8 is named "UTF-8", any other
        // converter by the label given.
        let encoder = match encoding {
            None => None,
            Some(label)
                if label.eq_ignore_ascii_case("UTF-8") || label.eq_ignore_ascii_case("UTF8") =>
            {
                Some((Output::Utf8, "UTF-8"))
            }
            Some(label) => match Output::for_label(label) {
                Some(output) if output != Output::Utf8 => Some((output, label)),
                _ => return Err(ErrorKind::UnsupportedEncoding),
            },
        };
        self.output = encoder.map(|(output, _)| output);
        self.bom = self.output.is_some_and(|output| !output.bom().is_empty());
        self.encoding_named |= encoder.is_some();
        self.put("<?xml version=");
        self.put_quoted(version.unwrap_or("1.0"));
        if let Some((_, name)) = encoder {
            self.put(" encoding=");
            self.put_quoted(name);
        }
        if let Some(standalone) = standalone {
            self.put(" standalone=");
            self.put_quoted(standalone);
        }
        self.put("?>\n");
        Ok(())
    }

    /// Closes everything open, then a newline unless indenting.
    fn end_document(&mut self) -> Step {
        while let Some(state) = self.state() {
            match state {
                State::Name | State::Attribute | State::Text => self.end_element()?,
                State::Pi | State::PiText => self.end_pi()?,
                State::Cdata => self.end_cdata()?,
                State::Comment => self.end_comment()?,
                _ => self.end_dtd()?,
            }
        }
        if !self.indent {
            self.put("\n");
        }
        Ok(())
    }

    fn start_comment(&mut self) -> Step {
        match self.state() {
            None | Some(State::Text) => {}
            Some(State::Name) => self.close_start_tag(true)?,
            Some(_) => return MISUSE,
        }
        self.push("", State::Comment)?;
        if self.indent {
            self.write_indent();
        }
        self.put("<!--");
        Ok(())
    }

    fn end_comment(&mut self) -> Step {
        if self.state() != Some(State::Comment) {
            return MISUSE;
        }
        self.put("-->");
        self.end_line_and_pop();
        Ok(())
    }

    fn end_line_and_pop(&mut self) {
        if self.indent {
            self.put("\n");
        }
        self.nodes.pop();
    }

    fn write_comment(&mut self, content: &str) -> Step {
        self.start_comment()?;
        self.write_string(content)?;
        self.end_comment()
    }

    fn start_element(&mut self, name: &str) -> Step {
        if name.is_empty() {
            return Err(ErrorKind::InvalidName);
        }
        match self.state() {
            Some(State::Pi | State::PiText) => return MISUSE,
            Some(State::Attribute | State::Name) => self.close_start_tag(true)?,
            _ => {}
        }
        self.push(name, State::Name)?;
        if self.indent {
            self.write_indent();
        }
        self.put("<");
        self.put(name);
        Ok(())
    }

    fn start_element_ns(&mut self, prefix: Option<&str>, name: &str, uri: Option<&str>) -> Step {
        if self.namespaces.is_none() {
            return MISUSE;
        }
        if name.is_empty() {
            return Err(ErrorKind::InvalidName);
        }
        self.start_element(&qualified(prefix, name))?;
        match uri {
            Some(uri) => {
                let element = self.nodes.last().map_or(0, |entry| entry.id);
                self.push_namespace(declaration(prefix), uri, element)
            }
            None => Ok(()),
        }
    }

    fn end_element(&mut self) -> Step {
        let Some(state) = self.state() else {
            self.namespaces = None;
            return MISUSE;
        };
        match state {
            State::Attribute | State::Name => {
                if state == State::Attribute {
                    self.end_attribute()?;
                }
                self.output_namespaces()?;
                if self.indent {
                    self.pending_indent = true;
                }
                self.put("/>");
            }
            State::Text => self.end_tag(),
            _ => return MISUSE,
        }
        self.end_line_and_pop();
        Ok(())
    }

    /// `</name>`, indented when the last content was an element.
    fn end_tag(&mut self) {
        if self.indent && self.pending_indent {
            self.write_indent();
        }
        self.pending_indent = true;
        let name = self.nodes.last().map_or("", |entry| entry.name.as_str());
        self.out.push_str("</");
        self.out.push_str(name);
        self.out.push('>');
    }

    fn full_end_element(&mut self) -> Step {
        match self.state() {
            Some(State::Attribute | State::Name) => {
                if self.state() == Some(State::Attribute) {
                    self.end_attribute()?;
                }
                self.output_namespaces()?;
                self.put(">");
                if self.indent {
                    self.pending_indent = false;
                }
            }
            Some(State::Text) => {}
            _ => return MISUSE,
        }
        self.end_tag();
        self.end_line_and_pop();
        Ok(())
    }

    fn write_raw(&mut self, content: &str) -> Step {
        self.raw_prefix()?;
        self.put(content);
        Ok(())
    }

    /// What raw content does before its bytes: close the open construct.
    fn raw_prefix(&mut self) -> Step {
        self.handle_state()?;
        if self.indent {
            self.pending_indent = false;
        }
        Ok(())
    }

    /// Escaped in element content (`xmlEncodeSpecialChars`) and attribute
    /// values (non-ASCII as `&#xH;` until an encoding is named); raw in
    /// comments, PIs, CDATA, the DTD and outside any element.
    fn write_string(&mut self, content: &str) -> Step {
        match self.state() {
            Some(State::Name | State::Text) => {
                self.raw_prefix()?;
                self.escape(content, &QUOT, false);
                Ok(())
            }
            Some(State::Attribute) => {
                self.escape(content, &ATTRIBUTE, !self.encoding_named);
                Ok(())
            }
            _ => self.write_raw(content),
        }
    }

    /// Base64 with a CRLF before each group past 72 columns.
    fn write_base64(&mut self, data: &[u8]) -> Step {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        self.raw_prefix()?;
        for (index, group) in data.chunks(3).enumerate() {
            if index > 0 && index % 18 == 0 {
                self.put("\r\n");
            }
            let bits = group
                .iter()
                .enumerate()
                .fold(0u32, |bits, (i, &b)| bits | u32::from(b) << (16 - 8 * i));
            for i in 0..4 {
                self.out.push(match i <= group.len() {
                    true => char::from(TABLE[(bits >> (18 - 6 * i) & 63) as usize]),
                    false => '=',
                });
            }
        }
        Ok(())
    }

    fn write_bin_hex(&mut self, data: &[u8]) -> Step {
        self.raw_prefix()?;
        for byte in data {
            for nibble in [byte >> 4, byte & 15] {
                self.out
                    .push(char::from(b"0123456789ABCDEF"[usize::from(nibble)]));
            }
        }
        Ok(())
    }

    fn start_attribute(&mut self, name: &str) -> Step {
        if name.is_empty() {
            return Err(ErrorKind::InvalidName);
        }
        match self.state() {
            Some(State::Attribute) => self.end_attribute()?,
            Some(State::Name) => {}
            _ => return MISUSE,
        }
        self.put(" ");
        self.put(name);
        self.put("=");
        self.put_quote();
        self.set_state(State::Attribute);
        Ok(())
    }

    /// The declaration is checked and queued before the attribute starts,
    /// so a misplaced call can leave it pending.
    fn start_attribute_ns(&mut self, prefix: Option<&str>, name: &str, uri: Option<&str>) -> Step {
        let Some(namespaces) = &self.namespaces else {
            return MISUSE;
        };
        if name.is_empty() {
            return Err(ErrorKind::InvalidName);
        }
        if let Some(uri) = uri {
            let attribute = declaration(prefix);
            let element = self.nodes.last().map_or(0, |entry| entry.id);
            let declared = namespaces
                .iter()
                .rev()
                .find(|namespace| namespace.attribute == attribute && namespace.element == element);
            match declared {
                Some(declared) if declared.uri != uri => return Err(ErrorKind::Namespace),
                Some(_) => {}
                None => self.push_namespace(attribute, uri, element)?,
            }
        }
        self.start_attribute(&qualified(prefix, name))
    }

    fn end_attribute(&mut self) -> Step {
        if self.state() != Some(State::Attribute) {
            return MISUSE;
        }
        self.set_state(State::Name);
        self.put_quote();
        Ok(())
    }

    fn write_attribute(&mut self, name: &str, content: &str) -> Step {
        self.start_attribute(name)?;
        self.write_string(content)?;
        self.end_attribute()
    }

    fn write_attribute_ns(
        &mut self,
        prefix: Option<&str>,
        name: &str,
        uri: Option<&str>,
        content: &str,
    ) -> Step {
        self.start_attribute_ns(prefix, name, uri)?;
        self.write_string(content)?;
        self.end_attribute()
    }

    fn write_element(&mut self, name: &str, content: Option<&str>) -> Step {
        self.start_element(name)?;
        if let Some(content) = content {
            self.write_string(content)?;
        }
        self.end_element()
    }

    fn write_element_ns(
        &mut self,
        prefix: Option<&str>,
        name: &str,
        uri: Option<&str>,
        content: &str,
    ) -> Step {
        self.start_element_ns(prefix, name, uri)?;
        self.write_string(content)?;
        self.end_element()
    }

    fn start_pi(&mut self, target: &str) -> Step {
        if target.is_empty() {
            return Err(ErrorKind::InvalidName);
        }
        if target.eq_ignore_ascii_case("xml") {
            return Err(ErrorKind::InvalidDeclaration);
        }
        match self.state() {
            Some(State::Attribute | State::Name) => self.close_start_tag(false)?,
            None | Some(State::Text | State::Dtd) => {}
            Some(_) => return MISUSE,
        }
        self.push(target, State::Pi)?;
        self.put("<?");
        self.put(target);
        Ok(())
    }

    /// With nothing open this succeeds without output, as in libxml2.
    fn end_pi(&mut self) -> Step {
        match self.state() {
            None => return Ok(()),
            Some(State::Pi | State::PiText) => self.put("?>"),
            Some(_) => return MISUSE,
        }
        self.end_line_and_pop();
        Ok(())
    }

    fn write_pi(&mut self, target: &str, content: Option<&str>) -> Step {
        self.start_pi(target)?;
        if let Some(content) = content {
            self.write_string(content)?;
        }
        self.end_pi()
    }

    fn start_cdata(&mut self) -> Step {
        match self.state() {
            None | Some(State::Text | State::Pi | State::PiText) => {}
            Some(State::Attribute | State::Name) => self.close_start_tag(false)?,
            Some(_) => return MISUSE,
        }
        self.push("", State::Cdata)?;
        self.put("<![CDATA[");
        Ok(())
    }

    fn end_cdata(&mut self) -> Step {
        if self.state() != Some(State::Cdata) {
            return MISUSE;
        }
        self.put("]]>");
        self.nodes.pop();
        Ok(())
    }

    fn write_cdata(&mut self, content: Option<&str>) -> Step {
        self.start_cdata()?;
        if let Some(content) = content {
            self.write_string(content)?;
        }
        self.end_cdata()
    }

    /// A public identifier needs a system identifier; that failure comes
    /// after `<!DOCTYPE name` is written and the DTD is open, as in libxml2.
    fn start_dtd(&mut self, name: &str, public: Option<&str>, system: Option<&str>) -> Step {
        if name.is_empty() {
            return Err(ErrorKind::InvalidName);
        }
        if !self.nodes.is_empty() {
            return MISUSE;
        }
        self.push(name, State::Dtd)?;
        self.put("<!DOCTYPE ");
        self.put(name);
        let separator = if self.indent { "\n" } else { " " };
        if let Some(public) = public {
            if system.is_none() {
                return Err(ErrorKind::InvalidDtd);
            }
            self.put(separator);
            self.put("PUBLIC ");
            self.put_quoted(public);
        }
        if let Some(system) = system {
            match (public, self.indent) {
                (None, _) => {
                    self.put(separator);
                    self.put("SYSTEM ");
                }
                (Some(_), true) => self.put("\n       "),
                (Some(_), false) => self.put(" "),
            }
            self.put_quoted(system);
        }
        Ok(())
    }

    /// Closes declarations and comments open in the DTD, then the DTD.
    fn end_dtd(&mut self) -> Step {
        while let Some(state) = self.state() {
            match state {
                State::DtdText | State::Dtd => {
                    if state == State::DtdText {
                        self.put("]");
                    }
                    self.put(">");
                    self.end_line_and_pop();
                }
                State::DtdElement | State::DtdElementText => self.end_dtd_element()?,
                State::DtdAttlist | State::DtdAttlistText => self.end_dtd_attlist()?,
                State::DtdEntity | State::DtdParameterEntity | State::DtdEntityText => {
                    self.end_dtd_entity()?
                }
                State::Comment => self.end_comment()?,
                _ => break,
            }
        }
        Ok(())
    }

    fn write_dtd(
        &mut self,
        name: &str,
        public: Option<&str>,
        system: Option<&str>,
        subset: Option<&str>,
    ) -> Step {
        self.start_dtd(name, public, system)?;
        if let Some(subset) = subset {
            self.write_string(subset)?;
        }
        self.end_dtd()
    }

    /// Opens the internal subset if needed; declarations also start with
    /// nothing open.
    fn enter_subset(&mut self) -> Step {
        match self.state() {
            Some(State::Dtd) => {
                self.put(" [");
                if self.indent {
                    self.put("\n");
                }
                self.set_state(State::DtdText);
            }
            None | Some(State::DtdText) => {}
            Some(_) => return MISUSE,
        }
        Ok(())
    }

    /// `<!ELEMENT`, `<!ATTLIST` or `<!ENTITY` with `name`.
    fn start_declaration(&mut self, name: &str, state: State, keyword: &str) -> Step {
        if name.is_empty() {
            return Err(ErrorKind::InvalidName);
        }
        self.enter_subset()?;
        self.push(name, state)?;
        if self.indent {
            self.write_indent();
        }
        self.put(keyword);
        if state == State::DtdParameterEntity {
            self.put("% ");
        }
        self.put(name);
        Ok(())
    }

    /// `>` for a declaration in one of `states`; `quote` closes an entity value.
    fn end_declaration(&mut self, states: [State; 2], quote: bool) -> Step {
        match self.state() {
            Some(state) if states.contains(&state) => {}
            Some(State::DtdEntityText) if quote => self.put_quote(),
            _ => return MISUSE,
        }
        self.put(">");
        self.end_line_and_pop();
        Ok(())
    }

    fn start_dtd_element(&mut self, name: &str) -> Step {
        self.start_declaration(name, State::DtdElement, "<!ELEMENT ")
    }

    fn end_dtd_element(&mut self) -> Step {
        self.end_declaration([State::DtdElement, State::DtdElementText], false)
    }

    fn write_dtd_element(&mut self, name: &str, content: &str) -> Step {
        self.start_dtd_element(name)?;
        self.write_string(content)?;
        self.end_dtd_element()
    }

    fn start_dtd_attlist(&mut self, name: &str) -> Step {
        self.start_declaration(name, State::DtdAttlist, "<!ATTLIST ")
    }

    fn end_dtd_attlist(&mut self) -> Step {
        self.end_declaration([State::DtdAttlist, State::DtdAttlistText], false)
    }

    fn write_dtd_attlist(&mut self, name: &str, content: &str) -> Step {
        self.start_dtd_attlist(name)?;
        self.write_string(content)?;
        self.end_dtd_attlist()
    }

    fn start_dtd_entity(&mut self, parameter: bool, name: &str) -> Step {
        let state = match parameter {
            true => State::DtdParameterEntity,
            false => State::DtdEntity,
        };
        self.start_declaration(name, state, "<!ENTITY ")
    }

    fn end_dtd_entity(&mut self) -> Step {
        self.end_declaration([State::DtdEntity, State::DtdParameterEntity], true)
    }

    fn write_dtd_internal_entity(&mut self, parameter: bool, name: &str, content: &str) -> Step {
        if name.is_empty() {
            return Err(ErrorKind::InvalidName);
        }
        self.start_dtd_entity(parameter, name)?;
        self.write_string(content)?;
        self.end_dtd_entity()
    }

    fn write_dtd_external_entity(
        &mut self,
        parameter: bool,
        name: &str,
        public: Option<&str>,
        system: Option<&str>,
        notation: Option<&str>,
    ) -> Step {
        if public.is_none() && system.is_none() || parameter && notation.is_some() {
            return Err(ErrorKind::InvalidDtd);
        }
        self.start_dtd_entity(parameter, name)?;
        self.write_dtd_external_entity_contents(public, system, notation)?;
        self.end_dtd_entity()
    }

    /// The external identifier and `NDATA` of an open entity declaration.
    fn write_dtd_external_entity_contents(
        &mut self,
        public: Option<&str>,
        system: Option<&str>,
        notation: Option<&str>,
    ) -> Step {
        match self.state() {
            Some(State::DtdEntity) => {}
            Some(State::DtdParameterEntity) if notation.is_none() => {}
            Some(State::DtdParameterEntity) => return Err(ErrorKind::InvalidDtd),
            _ => return MISUSE,
        }
        self.external_id(public, system)?;
        if let Some(notation) = notation {
            self.put(" NDATA ");
            self.put(notation);
        }
        Ok(())
    }

    /// ` PUBLIC "p" "s"` or ` SYSTEM "s"`; a public identifier needs a
    /// system identifier.
    fn external_id(&mut self, public: Option<&str>, system: Option<&str>) -> Step {
        if public.is_some() && system.is_none() {
            return Err(ErrorKind::InvalidDtd);
        }
        self.identifiers(public, system);
        Ok(())
    }

    /// ` PUBLIC "p"`, then ` SYSTEM "s"` or ` "s"`.
    fn identifiers(&mut self, public: Option<&str>, system: Option<&str>) {
        if let Some(public) = public {
            self.put(" PUBLIC ");
            self.put_quoted(public);
        }
        if let Some(system) = system {
            if public.is_none() {
                self.put(" SYSTEM");
            }
            self.put(" ");
            self.put_quoted(system);
        }
    }

    fn write_dtd_entity(
        &mut self,
        parameter: bool,
        name: &str,
        public: Option<&str>,
        system: Option<&str>,
        notation: Option<&str>,
        content: Option<&str>,
    ) -> Step {
        match (public, system, content) {
            (None, None, None) => Err(ErrorKind::InvalidDtd),
            _ if parameter && notation.is_some() => Err(ErrorKind::InvalidDtd),
            (None, None, Some(content)) => self.write_dtd_internal_entity(parameter, name, content),
            _ => self.write_dtd_external_entity(parameter, name, public, system, notation),
        }
    }

    /// Unlike the other declarations, a notation fails when indenting with
    /// nothing open, and takes a public identifier alone.
    fn write_dtd_notation(
        &mut self,
        name: &str,
        public: Option<&str>,
        system: Option<&str>,
    ) -> Step {
        if name.is_empty() {
            return Err(ErrorKind::InvalidName);
        }
        self.enter_subset()?;
        if self.indent && !self.write_indent() {
            return MISUSE;
        }
        self.put("<!NOTATION ");
        self.put(name);
        self.identifiers(public, system);
        self.put(">");
        Ok(())
    }
}

/// `prefix:name`, or `name` without a prefix.
fn qualified(prefix: Option<&str>, name: &str) -> String {
    prefix.map_or_else(|| name.to_owned(), |prefix| format!("{prefix}:{name}"))
}

/// `xmlns:prefix`, or `xmlns` without a prefix.
fn declaration(prefix: Option<&str>) -> String {
    prefix.map_or_else(|| "xmlns".to_owned(), |prefix| format!("xmlns:{prefix}"))
}

/// libxml2's `xmlTextWriter` over any `Write` sink.
///
/// Each call's output is built in memory, checked against the limits, then
/// handed to the sink (converted when `start_document` names an encoding;
/// characters it lacks become `&#N;`). A failure libxml2 reports as -1 is
/// `WriteError::Xml` and leaves the state and partial output libxml2 leaves,
/// so the writer stays usable. A limit or I/O failure is terminal: later
/// calls return `InvalidState` without I/O. [`WriterLimits`] fields used:
/// `max_output_bytes`, `max_token_bytes` (one call's encoded output),
/// `max_depth` (open elements, comments, PIs, CDATA sections and DTD parts)
/// and `max_attributes` (pending namespace declarations).
pub struct TextWriter<W: Write> {
    sink: W,
    machine: Machine,
    encoded: Vec<u8>,
    limits: WriterLimits,
    position: usize,
    failed: bool,
}

/// Public methods calling the state machine's method of the same name.
macro_rules! calls {
    ($($(#[$doc:meta])* $name:ident($($arg:ident: $type:ty),*);)*) => {$(
        $(#[$doc])*
        pub fn $name(&mut self, $($arg: $type),*) -> Result<(), WriteError> {
            self.call(|machine| machine.$name($($arg),*))
        }
    )*};
}

impl<W: Write> TextWriter<W> {
    /// `xmlNewTextWriter`: no indentation, one-space indent string, `"`.
    pub fn new(sink: W) -> Self {
        Self::with_limits(sink, WriterLimits::default())
    }

    pub fn with_limits(sink: W, limits: WriterLimits) -> Self {
        Self {
            sink,
            machine: Machine {
                out: String::new(),
                nodes: Vec::new(),
                namespaces: Some(Vec::new()),
                ids: 0,
                indent: false,
                pending_indent: false,
                indent_string: " ".to_owned(),
                quote: '"',
                output: None,
                bom: false,
                encoding_named: false,
                max_depth: limits.max_depth,
                max_namespaces: limits.max_attributes,
            },
            encoded: Vec::new(),
            limits,
            position: 0,
            failed: false,
        }
    }

    /// Bytes the sink has confirmed.
    pub fn position(&self) -> usize {
        self.position
    }

    /// The sink, without flushing or closing anything.
    pub fn into_inner(self) -> W {
        self.sink
    }

    /// `xmlTextWriterSetIndent`: newlines and indentation by stack depth.
    pub fn set_indent(&mut self, indent: bool) {
        self.machine.indent = indent;
        self.machine.pending_indent = true;
    }

    /// `xmlTextWriterSetIndentString`; any string, as in libxml2.
    pub fn set_indent_string(&mut self, indent: &str) {
        self.machine.indent_string = indent.to_owned();
    }

    /// `xmlTextWriterSetQuoteChar`: `"` or `'`, else `UnsupportedOption`.
    pub fn set_quote_char(&mut self, quote: char) -> Result<(), WriteError> {
        self.call(|machine| match quote {
            '"' | '\'' => {
                machine.quote = quote;
                Ok(())
            }
            _ => Err(ErrorKind::UnsupportedOption),
        })
    }

    /// `xmlTextWriterFlush`: flushes the sink.
    pub fn flush(&mut self) -> Result<(), WriteError> {
        self.call(|_| Ok(()))?;
        flush(&mut self.sink).inspect_err(|_| self.failed = true)
    }

    /// `xmlTextWriterEndDocument`: closes everything open, writes a newline
    /// unless indenting, and flushes.
    pub fn end_document(&mut self) -> Result<(), WriteError> {
        self.call(Machine::end_document)?;
        self.flush()
    }

    calls! {
        /// `xmlTextWriterStartDocument`: the XML declaration. An encoding
        /// selects the output converter (UTF-8, UTF-16 behind a BOM,
        /// UTF-16LE/BE, US-ASCII, ISO-8859-x, windows-1252); it is named
        /// `UTF-8` for UTF-8 and as given otherwise.
        start_document(version: Option<&str>, encoding: Option<&str>, standalone: Option<&str>);
        start_comment();
        end_comment();
        write_comment(content: &str);
        /// Closes an open start tag first; inside a PI it fails.
        start_element(name: &str);
        /// The declaration of a namespace is written when the start tag closes.
        start_element_ns(prefix: Option<&str>, name: &str, uri: Option<&str>);
        /// `/>` for an open start tag, else an end tag.
        end_element();
        /// Always an end tag: `<a></a>`.
        full_end_element();
        /// Unescaped.
        write_raw(content: &str);
        /// Escaped in element content and attribute values, raw elsewhere.
        write_string(content: &str);
        write_base64(data: &[u8]);
        write_bin_hex(data: &[u8]);
        start_attribute(name: &str);
        start_attribute_ns(prefix: Option<&str>, name: &str, uri: Option<&str>);
        end_attribute();
        write_attribute(name: &str, content: &str);
        write_attribute_ns(prefix: Option<&str>, name: &str, uri: Option<&str>, content: &str);
        write_element(name: &str, content: Option<&str>);
        write_element_ns(prefix: Option<&str>, name: &str, uri: Option<&str>, content: &str);
        /// Targets named `xml` in any case are refused.
        start_pi(target: &str);
        end_pi();
        write_pi(target: &str, content: Option<&str>);
        start_cdata();
        end_cdata();
        write_cdata(content: Option<&str>);
        /// Only with nothing open.
        start_dtd(name: &str, public: Option<&str>, system: Option<&str>);
        end_dtd();
        write_dtd(name: &str, public: Option<&str>, system: Option<&str>, subset: Option<&str>);
        start_dtd_element(name: &str);
        end_dtd_element();
        write_dtd_element(name: &str, content: &str);
        start_dtd_attlist(name: &str);
        end_dtd_attlist();
        write_dtd_attlist(name: &str, content: &str);
        start_dtd_entity(parameter: bool, name: &str);
        end_dtd_entity();
        write_dtd_internal_entity(parameter: bool, name: &str, content: &str);
        write_dtd_external_entity(
            parameter: bool,
            name: &str,
            public: Option<&str>,
            system: Option<&str>,
            notation: Option<&str>
        );
        write_dtd_external_entity_contents(
            public: Option<&str>,
            system: Option<&str>,
            notation: Option<&str>
        );
        /// Internal with content and no identifiers, else external.
        write_dtd_entity(
            parameter: bool,
            name: &str,
            public: Option<&str>,
            system: Option<&str>,
            notation: Option<&str>,
            content: Option<&str>
        );
        write_dtd_notation(name: &str, public: Option<&str>, system: Option<&str>);
    }

    /// Runs one call, then writes its output: a libxml2 failure keeps the
    /// output it produced, a limit or I/O failure ends the writer.
    fn call(&mut self, step: impl FnOnce(&mut Machine) -> Step) -> Result<(), WriteError> {
        if self.failed {
            return Err(WriteError::Xml(Error::new(
                ErrorKind::InvalidState,
                self.position,
            )));
        }
        let result = step(&mut self.machine);
        let committed = match result {
            Err(ErrorKind::LimitExceeded(_)) => Ok(()),
            _ => self.commit(),
        };
        self.machine.out.clear();
        let result = committed
            .and_then(|()| result.map_err(|kind| WriteError::Xml(Error::new(kind, self.position))));
        if let Err(
            WriteError::Io(_)
            | WriteError::Xml(Error {
                kind: ErrorKind::LimitExceeded(_),
                ..
            }),
        ) = result
        {
            self.failed = true;
        }
        result
    }

    /// Checks the call's output against the limits, converts it when an
    /// encoding needs that, and writes it.
    fn commit(&mut self) -> Result<(), WriteError> {
        let limit =
            |limit| WriteError::Xml(Error::new(ErrorKind::LimitExceeded(limit), self.position));
        let machine = &mut self.machine;
        if machine.out.len() > self.limits.max_token_bytes {
            return Err(limit(Limit::TokenBytes));
        }
        let bytes = match machine.output {
            Some(output) if output != Output::Utf8 => {
                self.encoded.clear();
                if std::mem::take(&mut machine.bom) {
                    self.encoded.extend_from_slice(output.bom());
                }
                for c in machine.out.chars() {
                    if !output.push(&mut self.encoded, c) {
                        let mut buffer = [0; 12];
                        self.encoded
                            .extend_from_slice(char_reference(c, false, &mut buffer).as_bytes());
                    }
                }
                &self.encoded[..]
            }
            _ => machine.out.as_bytes(),
        };
        if bytes.len() > self.limits.max_output_bytes.saturating_sub(self.position) {
            return Err(limit(Limit::OutputBytes));
        }
        write_all(&mut self.sink, bytes, &mut self.position)
    }
}
