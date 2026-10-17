use std::{borrow::Cow, ops::Range};

use memchr::{memchr, memmem};

use crate::escape::xml_char;
use crate::parser::{Budget, ParserState, space};
use crate::tokenizer::TokenKind;
use crate::{Error, ErrorKind, Limit, Name, SourceOrigin};

/// Inclusive resource ceilings. An element's start tag counts as one token.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    pub max_input_bytes: usize,
    pub max_token_bytes: usize,
    pub max_depth: usize,
    pub max_attributes: usize,
    /// Number of emitted events, including the declaration and empty elements.
    pub max_events: usize,
    /// Cumulative requested string capacity for normalization, namespace copies,
    /// streaming names and full new owned-DOM pool capacities, including buffers
    /// already released. Borrowed slice text is free; retaining owned storage
    /// can spend more of this budget.
    pub max_owned_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_input_bytes: 64 * 1024 * 1024,
            max_token_bytes: 1024 * 1024,
            max_depth: 256,
            max_attributes: 1024,
            max_events: 1_000_000,
            max_owned_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct Attribute<'a> {
    pub name: Name<'a>,
    pub value: Cow<'a, str>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct Element<'a> {
    pub name: Name<'a>,
    /// Source order, including namespace declarations in the XMLNS namespace.
    pub attributes: Vec<Attribute<'a>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Declaration<'a> {
    pub version: &'a str,
    pub encoding: Option<&'a str>,
    pub standalone: Option<bool>,
}

/// Logical events borrow the input except where XML normalization changes it.
#[derive(Debug, Eq, PartialEq)]
pub enum Event<'a> {
    Declaration(Declaration<'a>),
    Start(Element<'a>),
    Empty(Element<'a>),
    End(Name<'a>),
    Text(Cow<'a, str>),
    Cdata(Cow<'a, str>),
    Comment(Cow<'a, str>),
    ProcessingInstruction {
        target: Cow<'a, str>,
        data: Cow<'a, str>,
    },
}

enum ReaderStep<'input> {
    Xml(Event<'input>),
    Complete,
    #[cfg(test)]
    Doctype(usize),
}

/// Strict, namespace-aware XML 1.0 event reader over a complete UTF-8 input.
///
/// This borrows the complete input; bounded-memory I/O streaming is a separate API.
/// Top-level whitespace is skipped, while whitespace inside the root is kept.
/// After an error, every call returns that same error. EOF repeatedly returns
/// `Ok(None)`. DTDs and encodings other than UTF-8 are explicitly unsupported.
pub struct Reader<'a> {
    input: &'a str,
    cursor: usize,
    origin: Option<SourceOrigin>,
    state: ParserState<'a>,
    limits: Limits,
    token_limit_kind: Limit,
    failure: Option<Error>,
    finished: bool,
}

impl<'a> Reader<'a> {
    pub(crate) fn owned_budget(&mut self) -> &mut Budget {
        self.state.owned_budget()
    }

    pub fn new(input: &'a [u8]) -> Result<Self, Error> {
        Self::with_limits(input, Limits::default())
    }

    pub fn with_limits(input: &'a [u8], limits: Limits) -> Result<Self, Error> {
        if input.len() > limits.max_input_bytes {
            return Err(Error::new(ErrorKind::LimitExceeded(Limit::InputBytes), 0));
        }
        if input.starts_with(&[0xff, 0xfe])
            || input.starts_with(&[0xfe, 0xff])
            || input.starts_with(&[0, 0, 0xfe, 0xff])
            || input.starts_with(&[0, b'<', 0, b'?'])
        {
            return Err(Error::new(ErrorKind::UnsupportedEncoding, 0));
        }
        let input = std::str::from_utf8(input)
            .map_err(|e| Error::new(ErrorKind::InvalidUtf8, e.valid_up_to()))?;
        if let Some((i, _)) = input.char_indices().find(|(_, c)| !xml_char(*c)) {
            return Err(Error::new(ErrorKind::InvalidCharacter, i));
        }
        let initial = if input.starts_with('\u{feff}') { 3 } else { 0 };
        Ok(Self {
            input,
            cursor: initial,
            origin: None,
            state: ParserState::new(limits, initial),
            limits,
            token_limit_kind: Limit::TokenBytes,
            failure: None,
            finished: false,
        })
    }

    pub(crate) fn with_decode_limits(
        input: &'a [u8],
        limits: Limits,
        decode: crate::DecodeLimits,
    ) -> Result<Self, Error> {
        let mut reader = Self::with_limits(input, limits)?;
        if input.len() - reader.cursor > decode.max_decoded_bytes {
            return Err(Error::new(ErrorKind::LimitExceeded(Limit::DecodedBytes), 0));
        }
        if decode.max_decoded_token_bytes < limits.max_token_bytes {
            reader.limits.max_token_bytes = decode.max_decoded_token_bytes;
            reader.token_limit_kind = Limit::DecodedTokenBytes;
            reader.state.set_encoding(
                crate::encoding::SourceEncoding::Utf8,
                reader.cursor,
                decode.max_decoded_token_bytes,
            );
        }
        Ok(reader)
    }

    /// Byte position after the most recently consumed input.
    pub fn position(&self) -> usize {
        self.cursor
    }

    /// Origin of the latest emitted event; absent before the first event.
    /// EOF and failures preserve the last delivered origin.
    pub fn event_origin(&self) -> Option<SourceOrigin> {
        self.origin
    }

    /// Exact original input span of the latest emitted event, when available.
    pub fn event_range(&self) -> Option<Range<usize>> {
        self.event_origin().and_then(SourceOrigin::input_range)
    }

    pub fn read_event(&mut self) -> Result<Option<Event<'a>>, Error> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        if self.finished {
            return Ok(None);
        }
        let result = self.read_next();
        if let Err(error) = result {
            self.failure = Some(error);
        }
        result
    }

    fn read_next(&mut self) -> Result<Option<Event<'a>>, Error> {
        match self.read_step::<false>()? {
            ReaderStep::Xml(event) => Ok(Some(event)),
            ReaderStep::Complete => Ok(None),
            #[cfg(test)]
            ReaderStep::Doctype(_) => Err(Error::new(ErrorKind::InvalidState, self.cursor)),
        }
    }
    fn read_step<const DTD: bool>(&mut self) -> Result<ReaderStep<'a>, Error> {
        loop {
            if !self.state.in_content() {
                let start = self.cursor;
                while self
                    .input
                    .as_bytes()
                    .get(self.cursor)
                    .is_some_and(|b| space(*b))
                {
                    self.cursor += 1;
                }
                self.check_token(start, self.cursor)?;
            }
            let start = self.cursor;
            if start == self.input.len() {
                self.state.validate_finish(start)?;
                self.finished = true;
                return Ok(ReaderStep::Complete);
            }
            self.state.check_event_limit(start)?;
            let tail = &self.input[start..];
            let (kind, end) = if !tail.starts_with('<') {
                let rest = &self.input.as_bytes()[start..];
                let end = start + memchr(b'<', rest).unwrap_or(rest.len());
                self.check_token(start, end)?;
                (TokenKind::Text, end)
            } else if tail.starts_with("<!--") {
                (TokenKind::Comment, self.delimited(start, 4, b"-->")?)
            } else if tail.starts_with("<![CDATA[") {
                self.state.check_token_start(TokenKind::Cdata, start)?;
                (TokenKind::Cdata, self.delimited(start, 9, b"]]>")?)
            } else if tail.starts_with("<?") {
                (TokenKind::Instruction, self.delimited(start, 2, b"?>")?)
            } else if tail.starts_with("</") {
                (TokenKind::End, self.tag_end(start)?)
            } else if tail.starts_with("<!DOCTYPE") {
                (TokenKind::Dtd, start + 9)
            } else if tail.starts_with("<!") {
                (TokenKind::InvalidMarkup, start + 2)
            } else {
                self.state.check_token_start(TokenKind::Start, start)?;
                (TokenKind::Start, self.tag_end(start)?)
            };
            #[cfg(test)]
            if DTD && kind == TokenKind::Dtd {
                self.check_token(start, end)?;
                self.state.check_doctype_start(start)?;
                return Ok(ReaderStep::Doctype(start));
            }
            let result =
                self.state
                    .process_token(kind, &self.input[start..end], start..end, |raw, _, _| {
                        Ok(Cow::Borrowed(raw))
                    });
            // Preserve the slice reader's cursor at lexical/structural failures.
            // Value normalization and declaration parsing consume their raw token.
            let consumed_error = result.as_ref().is_err_and(|error| {
                matches!(
                    kind,
                    TokenKind::Text
                        | TokenKind::Comment
                        | TokenKind::Cdata
                        | TokenKind::Instruction
                ) && matches!(
                    error.kind,
                    ErrorKind::InvalidReference
                        | ErrorKind::UnknownEntity
                        | ErrorKind::Allocation
                        | ErrorKind::LimitExceeded(Limit::OwnedBytes)
                        | ErrorKind::InvalidDeclaration
                        | ErrorKind::UnsupportedEncoding
                )
            });
            if result.is_ok() || consumed_error {
                self.cursor = end;
            }
            let event = result?;
            if let Some(event) = event {
                self.origin = Some(SourceOrigin::input(start..end));
                return Ok(ReaderStep::Xml(event));
            }
        }
    }

    fn check_token(&self, start: usize, end: usize) -> Result<(), Error> {
        if end - start > self.limits.max_token_bytes {
            Err(Error::new(
                ErrorKind::LimitExceeded(self.token_limit_kind),
                start,
            ))
        } else {
            Ok(())
        }
    }

    fn delimited(&self, start: usize, prefix: usize, delimiter: &[u8]) -> Result<usize, Error> {
        let available = self.input.len() - start;
        let length = available.min(self.limits.max_token_bytes);
        if prefix <= length
            && let Some(relative) = memmem::find(
                &self.input.as_bytes()[start + prefix..start + length],
                delimiter,
            )
        {
            return Ok(start + prefix + relative + delimiter.len());
        }
        Err(Error::new(
            if available > self.limits.max_token_bytes {
                ErrorKind::LimitExceeded(self.token_limit_kind)
            } else {
                ErrorKind::UnexpectedEof
            },
            start,
        ))
    }

    fn tag_end(&self, start: usize) -> Result<usize, Error> {
        let mut quote = None;
        for (relative, &byte) in self.input.as_bytes()[start..]
            .iter()
            .take(self.limits.max_token_bytes)
            .enumerate()
        {
            match (quote, byte) {
                (Some(q), b) if q == b => quote = None,
                (None, b'\'' | b'"') => quote = Some(byte),
                (None, b'>') => return Ok(start + relative + 1),
                _ => {}
            }
        }
        Err(Error::new(
            if self.input.len() - start > self.limits.max_token_bytes {
                ErrorKind::LimitExceeded(self.token_limit_kind)
            } else {
                ErrorKind::UnexpectedEof
            },
            start,
        ))
    }
}

#[cfg(test)]
#[path = "reader/dtd.rs"]
pub(crate) mod dtd;

#[cfg(test)]
#[path = "../tests/internal/dtd_reader_spec.rs"]
mod dtd_spec_tests;
