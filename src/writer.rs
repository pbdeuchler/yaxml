use std::io::{self, Write};

use crate::dtd::DtdLimits;
use crate::dtd_controller::DtdController;
use crate::escape::invalid_char;
use crate::parser::{ParserState, space};
use crate::tokenizer::TokenKind;
use crate::{
    DecodeLimits, Doctype, Document, Element, Error, ErrorKind, Event, Limit, Limits, Name,
    OwnedDocument, WriteError,
};

/// Inclusive writer ceilings. Token bytes count one complete encoded event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WriterLimits {
    pub max_output_bytes: usize,
    pub max_token_bytes: usize,
    pub max_depth: usize,
    pub max_attributes: usize,
    /// Shared semantic events; empty Text and top-level whitespace are ignored.
    pub max_events: usize,
    /// Cumulative normalization and retained-name/namespace string capacity.
    /// The encoded scratch token is bounded separately by max_token_bytes.
    pub max_owned_bytes: usize,
}

impl Default for WriterLimits {
    fn default() -> Self {
        Self {
            max_output_bytes: 64 * 1024 * 1024,
            max_token_bytes: 1024 * 1024,
            max_depth: 256,
            max_attributes: 1024,
            max_events: 1_000_000,
            max_owned_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy)]
enum Status {
    Fresh,
    Active,
    Finished,
    Failed,
}

/// Validates and serializes namespace-aware UTF-8 XML events to an explicit sink.
///
/// Each complete event is encoded and validated before any of its bytes are
/// written. Names and namespace declarations must agree with the active scope;
/// namespaces are not repaired. Attributes use double quotes. Text/attribute
/// references preserve logical values; raw CR in comments, CDATA and PI data,
/// and leading PI whitespace, follow XML normalization when reparsed.
///
/// A failure or recovered sink panic makes later calls return InvalidState.
/// Partial output cannot be rolled back. The writer performs no closing or I/O
/// on drop; the sink's own destructor retains its usual behavior.
pub struct Writer<W: Write> {
    sink: W,
    state: ParserState<'static>,
    buffer: TokenBuffer,
    limits: WriterLimits,
    position: usize,
    status: Status,
}

impl<W: Write> Writer<W> {
    pub fn new(sink: W) -> Self {
        Self::with_limits(sink, WriterLimits::default())
    }

    pub fn with_limits(sink: W, limits: WriterLimits) -> Self {
        Self {
            sink,
            state: ParserState::new(
                Limits {
                    max_input_bytes: limits.max_output_bytes,
                    max_token_bytes: limits.max_token_bytes,
                    max_depth: limits.max_depth,
                    max_attributes: limits.max_attributes,
                    max_events: limits.max_events,
                    max_owned_bytes: limits.max_owned_bytes,
                    ..Limits::default()
                },
                0,
            )
            .copying_names(),
            buffer: TokenBuffer {
                text: String::new(),
                offset: 0,
                limit: 0,
                ceiling: Limit::TokenBytes,
            },
            limits,
            position: 0,
            status: Status::Fresh,
        }
    }

    /// Bytes confirmed by successful sink write returns, including partial I/O.
    /// Bytes accepted inside a panicking sink call cannot be measured.
    pub fn position(&self) -> usize {
        self.position
    }

    /// Consumes the writer without implicit validation, flushing or closing.
    pub fn into_inner(self) -> W {
        self.sink
    }

    /// Writes one complete event after validation. Adjacent Text events can
    /// coalesce into a larger lexical token when the output is parsed again.
    pub fn write_event(&mut self, event: &Event<'_>) -> Result<(), WriteError> {
        if !matches!(self.status, Status::Fresh | Status::Active) {
            self.status = Status::Failed;
            return Err(self.invalid_state());
        }
        // Poison before state mutation or sink calls; unwinding cannot expose a
        // partially processed token as a reusable writer.
        self.status = Status::Failed;
        self.write_event_inner(event)?;
        self.status = Status::Active;
        Ok(())
    }

    /// Serializes a complete borrowing DOM as UTF-8, then validates and flushes.
    /// Requires a fresh writer and leaves it finished on success. A present
    /// declaration encoding is rewritten to UTF-8; childless elements use `/>`.
    pub fn write_document(&mut self, document: &Document<'_>) -> Result<(), WriteError> {
        self.begin_document()?;
        crate::serialize::borrowed(self, document, self.limits.max_attributes)?;
        self.finish_inner()?;
        self.status = Status::Finished;
        Ok(())
    }

    /// Serializes a complete owned DOM, with the same policy as write_document.
    /// Its original UTF-8 or UTF-16 source encoding does not affect output bytes.
    pub fn write_owned_document<A: crate::allocator::Allocator>(
        &mut self,
        document: &OwnedDocument<A>,
    ) -> Result<(), WriteError> {
        self.begin_document()?;
        crate::serialize::owned(self, document, self.limits.max_attributes)?;
        self.finish_inner()?;
        self.status = Status::Finished;
        Ok(())
    }

    fn begin_document(&mut self) -> Result<(), WriteError> {
        let fresh = matches!(self.status, Status::Fresh);
        // Cover traversal allocations and the final flush with the same guard
        // as event validation and writing. Helpers never reopen the lifecycle.
        self.status = Status::Failed;
        if fresh {
            Ok(())
        } else {
            Err(self.invalid_state())
        }
    }

    pub(crate) fn write_event_inner(&mut self, event: &Event<'_>) -> Result<(), WriteError> {
        // Entity boundaries write nothing: the replacement's events follow.
        if matches!(event, Event::EntityStart(_) | Event::EntityEnd) {
            return Ok(());
        }
        self.prepare(event).map_err(WriteError::Xml)?;
        self.write_buffer()
    }

    fn write_buffer(&mut self) -> Result<(), WriteError> {
        let mut cursor = 0;
        let bytes = self.buffer.text.as_bytes();
        while cursor < bytes.len() {
            match self.sink.write(&bytes[cursor..]) {
                Ok(0) => {
                    return Err(WriteError::Io(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "XML sink accepted no bytes",
                    )));
                }
                Ok(length) if length > bytes.len() - cursor => {
                    return Err(WriteError::Io(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "XML sink returned an oversized byte count",
                    )));
                }
                Ok(length) => {
                    cursor += length;
                    self.position += length;
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(WriteError::Io(error)),
            }
        }
        Ok(())
    }

    /// Requires a complete document and flushes the sink. Repeating a successful
    /// finish performs no I/O. A write after finish starts terminal InvalidState.
    pub fn finish(&mut self) -> Result<(), WriteError> {
        match self.status {
            Status::Finished => return Ok(()),
            Status::Failed => return Err(self.invalid_state()),
            Status::Fresh | Status::Active => {}
        }
        self.status = Status::Failed;
        self.finish_inner()?;
        self.status = Status::Finished;
        Ok(())
    }

    fn finish_inner(&mut self) -> Result<(), WriteError> {
        self.state
            .validate_finish(self.position)
            .map_err(WriteError::Xml)?;
        loop {
            match self.sink.flush() {
                Ok(()) => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(WriteError::Io(error)),
            }
        }
        Ok(())
    }

    fn invalid_state(&self) -> WriteError {
        WriteError::Xml(Error::new(ErrorKind::InvalidState, self.position))
    }

    fn prepare(&mut self, event: &Event<'_>) -> Result<(), Error> {
        self.reset_buffer();
        let kind = self.encode_event(event)?;
        self.validate_buffer_characters()?;
        if let Event::Doctype(doctype) = event {
            return self.check_doctype(doctype);
        }
        let raw = &self.buffer.text;
        let parsed = self.state.process_token(
            kind,
            raw,
            self.position..self.position + raw.len(),
            |raw, offset, budget| budget.copy(raw, offset),
        )?;
        check_event(event, parsed.as_ref(), self.position)?;
        Ok(())
    }

    /// Reparses the encoded declaration, including its whole internal subset.
    fn check_doctype(&mut self, doctype: &Doctype<'_>) -> Result<(), Error> {
        let start = self.position;
        self.state.check_doctype_start(start)?;
        let raw = self.buffer.text.as_str();
        let limits = Limits {
            max_input_bytes: self.limits.max_output_bytes,
            max_token_bytes: self.limits.max_token_bytes,
            max_depth: self.limits.max_depth,
            max_attributes: self.limits.max_attributes,
            max_dtd_bytes: self.limits.max_token_bytes,
            ..Limits::default()
        };
        let options = DtdLimits::new(limits, DecodeLimits::default());
        let relocate = |error: Error| Error::new(error.kind, start + error.offset);
        let mut controller = DtdController::stable_utf8(raw, 0, 0, options, false);
        let budget = self.state.owned_budget();
        while !controller.is_complete() {
            if controller.current().is_some() {
                controller.release();
            } else {
                controller.advance_borrowed(budget).map_err(relocate)?;
            }
        }
        if controller.physical_position() != raw.len() || Doctype::parse(raw) != *doctype {
            return Err(Error::new(ErrorKind::InvalidDtd, start));
        }
        self.state.commit_doctype();
        Ok(())
    }

    fn encode_event(&mut self, event: &Event<'_>) -> Result<TokenKind, Error> {
        let buffer = &mut self.buffer;
        let kind = match event {
            Event::Start(element) | Event::Empty(element) => {
                if element.attributes.len() > self.limits.max_attributes {
                    return Err(Error::new(
                        ErrorKind::LimitExceeded(Limit::Attributes),
                        self.position,
                    ));
                }
                buffer.push("<")?;
                buffer.push(element.name.qualified())?;
                for attribute in &element.attributes {
                    buffer.push(" ")?;
                    buffer.push(attribute.name.qualified())?;
                    buffer.push("=\"")?;
                    buffer.escaped(&attribute.value, true)?;
                    buffer.push("\"")?;
                }
                buffer.push(if matches!(event, Event::Empty(_)) {
                    "/>"
                } else {
                    ">"
                })?;
                TokenKind::Start
            }
            Event::EntityStart(_) | Event::EntityEnd => {
                return Err(Error::new(ErrorKind::InvalidState, self.position));
            }
            Event::End(name) => {
                buffer.push("</")?;
                buffer.push(name.qualified())?;
                buffer.push(">")?;
                TokenKind::End
            }
            Event::Text(value) => {
                buffer.check_size(value.len())?;
                if !self.state.in_content() && value.bytes().all(space) {
                    // References are forbidden outside the root. Whitespace
                    // there is ignored by the shared semantic event counter.
                    buffer.push(value)?;
                } else {
                    buffer.escaped(value, false)?;
                }
                TokenKind::Text
            }
            Event::Cdata(value) => {
                buffer.check_size(value.len())?;
                if value.contains("]]>") {
                    return Err(Error::new(ErrorKind::Syntax, self.position));
                }
                buffer.push("<![CDATA[")?;
                buffer.push(value)?;
                buffer.push("]]>")?;
                TokenKind::Cdata
            }
            Event::Comment(value) => {
                buffer.check_size(value.len())?;
                if value.contains("--") || value.ends_with('-') {
                    return Err(Error::new(ErrorKind::Syntax, self.position));
                }
                buffer.push("<!--")?;
                buffer.push(value)?;
                buffer.push("-->")?;
                TokenKind::Comment
            }
            Event::ProcessingInstruction { target, data } => {
                buffer.check_size(data.len())?;
                if data.contains("?>") {
                    return Err(Error::new(ErrorKind::Syntax, self.position));
                }
                buffer.push("<?")?;
                buffer.push(target)?;
                if !data.is_empty() {
                    buffer.push(" ")?;
                    buffer.push(data)?;
                }
                buffer.push("?>")?;
                TokenKind::Instruction
            }
            Event::Doctype(doctype) => {
                buffer.push("<!DOCTYPE ")?;
                buffer.push(doctype.name)?;
                match (doctype.public_id, doctype.system_id) {
                    (Some(public), Some(system)) => {
                        buffer.push(" PUBLIC ")?;
                        buffer.quoted(public)?;
                        buffer.push(" ")?;
                        buffer.quoted(system)?;
                    }
                    (None, Some(system)) => {
                        buffer.push(" SYSTEM ")?;
                        buffer.quoted(system)?;
                    }
                    (None, None) => {}
                    (Some(_), None) => {
                        return Err(Error::new(ErrorKind::InvalidDtd, self.position));
                    }
                }
                if let Some(subset) = doctype.internal_subset {
                    buffer.push(" [")?;
                    buffer.push(subset)?;
                    buffer.push("]")?;
                }
                buffer.push(">")?;
                TokenKind::Dtd
            }
            Event::Declaration(declaration) => {
                buffer.push("<?xml version=\"")?;
                buffer.push(declaration.version)?;
                buffer.push("\"")?;
                if let Some(encoding) = declaration.encoding {
                    buffer.push(" encoding=\"")?;
                    buffer.push(encoding)?;
                    buffer.push("\"")?;
                }
                if let Some(standalone) = declaration.standalone {
                    buffer.push(if standalone {
                        " standalone=\"yes\""
                    } else {
                        " standalone=\"no\""
                    })?;
                }
                buffer.push("?>")?;
                TokenKind::Instruction
            }
        };
        Ok(kind)
    }

    fn validate_buffer_characters(&self) -> Result<(), Error> {
        if let Some(relative) = invalid_char(&self.buffer.text) {
            return Err(Error::new(
                ErrorKind::InvalidCharacter,
                self.position + relative,
            ));
        }
        Ok(())
    }

    fn reset_buffer(&mut self) {
        let remaining = self.limits.max_output_bytes - self.position;
        self.buffer.text.clear();
        self.buffer.offset = self.position;
        self.buffer.limit = self.limits.max_token_bytes.min(remaining);
        self.buffer.ceiling = if remaining <= self.limits.max_token_bytes {
            Limit::OutputBytes
        } else {
            Limit::TokenBytes
        };
    }
}

fn check_name(supplied: &Name<'_>, parsed: &Name<'_>, offset: usize) -> Result<(), Error> {
    if supplied.qualified() != parsed.qualified() {
        return Err(Error::new(ErrorKind::InvalidName, offset));
    }
    if supplied.namespace() != parsed.namespace() {
        return Err(Error::new(ErrorKind::Namespace, offset));
    }
    Ok(())
}

fn check_element(supplied: &Element<'_>, parsed: &Element<'_>, offset: usize) -> Result<(), Error> {
    check_name(&supplied.name, &parsed.name, offset)?;
    if supplied.attributes.len() != parsed.attributes.len() {
        return Err(Error::new(ErrorKind::InvalidName, offset));
    }
    for (supplied, parsed) in supplied.attributes.iter().zip(&parsed.attributes) {
        check_name(&supplied.name, &parsed.name, offset)?;
        if supplied.value != parsed.value {
            return Err(Error::new(ErrorKind::Syntax, offset));
        }
    }
    Ok(())
}

fn check_event(
    supplied: &Event<'_>,
    parsed: Option<&Event<'_>>,
    offset: usize,
) -> Result<(), Error> {
    match (supplied, parsed) {
        (Event::Start(supplied), Some(Event::Start(parsed)))
        | (Event::Empty(supplied), Some(Event::Empty(parsed))) => {
            check_element(supplied, parsed, offset)
        }
        (Event::End(supplied), Some(Event::End(parsed))) => check_name(supplied, parsed, offset),
        (Event::Declaration(supplied), Some(Event::Declaration(parsed))) if supplied == parsed => {
            Ok(())
        }
        (
            Event::ProcessingInstruction { target, .. },
            Some(Event::ProcessingInstruction { target: actual, .. }),
        ) if target == actual => Ok(()),
        (Event::Text(_), None | Some(Event::Text(_)))
        | (Event::Cdata(_), Some(Event::Cdata(_)))
        | (Event::Comment(_), Some(Event::Comment(_))) => Ok(()),
        (Event::Declaration(_), _) => Err(Error::new(ErrorKind::InvalidDeclaration, offset)),
        (Event::ProcessingInstruction { .. }, _) => Err(Error::new(ErrorKind::InvalidName, offset)),
        _ => Err(Error::new(ErrorKind::Syntax, offset)),
    }
}

struct TokenBuffer {
    text: String,
    offset: usize,
    limit: usize,
    ceiling: Limit,
}

impl TokenBuffer {
    fn check_size(&self, additional: usize) -> Result<usize, Error> {
        self.text
            .len()
            .checked_add(additional)
            .filter(|length| *length <= self.limit)
            .ok_or(Error::new(
                ErrorKind::LimitExceeded(self.ceiling),
                self.offset,
            ))
    }

    fn push(&mut self, value: &str) -> Result<(), Error> {
        let needed = self.check_size(value.len())?;
        if needed > self.text.capacity() {
            let target = self
                .text
                .capacity()
                .saturating_mul(2)
                .max(64)
                .max(needed)
                .min(self.limit);
            self.text
                .try_reserve_exact(target - self.text.len())
                .map_err(|_| Error::new(ErrorKind::Allocation, self.offset))?;
        }
        self.text.push_str(value);
        Ok(())
    }

    /// Quotes a literal; one containing both quote kinds fails reparsing.
    fn quoted(&mut self, value: &str) -> Result<(), Error> {
        let quote = if value.contains('"') { "'" } else { "\"" };
        self.push(quote)?;
        self.push(value)?;
        self.push(quote)
    }

    fn escaped(&mut self, value: &str, attribute: bool) -> Result<(), Error> {
        // Every replacement is at least as long as its source. Reject oversized
        // logical data before scanning it for individual replacement points.
        self.check_size(value.len())?;
        let mut start = 0;
        for (index, character) in value.char_indices() {
            let escaped = match character {
                '&' => "&amp;",
                '<' => "&lt;",
                '>' if !attribute => "&gt;",
                '"' if attribute => "&quot;",
                '\r' => "&#13;",
                '\n' if attribute => "&#10;",
                '\t' if attribute => "&#9;",
                _ => continue,
            };
            self.push(&value[start..index])?;
            self.push(escaped)?;
            start = index + character.len_utf8();
        }
        self.push(&value[start..])
    }
}

#[cfg(test)]
#[path = "../tests/internal/writer.rs"]
mod tests;
