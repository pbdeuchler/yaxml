//! Bounded framing of decoded internal-DTD input; not declaration semantics.

use std::ops::Range;

use memchr::{memchr, memchr2, memchr3};

use crate::name::validate_name;
use crate::parser::space;
use crate::{Error, ErrorKind, Limit};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ScanMode {
    Doctype,
    SubsetFragment,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ScanKind {
    Header,
    Declaration,
    Comment,
    Instruction,
    ParameterReference,
    Whitespace,
    End,
}

#[derive(Clone, Copy, Debug)]
enum State {
    /// The header, from its `<!DOCTYPE` (which the caller verified).
    Header(Option<u8>),
    Between,
    Whitespace,
    LessThan,
    Bang,
    CommentDash,
    Declaration(Option<u8>),
    Comment(u8),
    Instruction(bool),
    ParameterReference,
    Closing,
}

pub(crate) struct DtdScanner {
    mode: ScanMode,
    max_unit_bytes: usize,
    max_work: usize,
    state: State,
    buffer: Vec<u8>,
    start: usize,
    position: usize,
    ready: Option<ScanKind>,
}

impl DtdScanner {
    pub(crate) fn new(mode: ScanMode, max_unit_bytes: usize, max_work: usize) -> Self {
        Self {
            mode,
            max_unit_bytes,
            max_work,
            state: match mode {
                ScanMode::Doctype => State::Header(None),
                ScanMode::SubsetFragment => State::Between,
            },
            buffer: Vec::new(),
            start: 0,
            position: 0,
            ready: None,
        }
    }

    pub(crate) fn cap_work(&mut self, ceiling: usize) {
        self.max_work = self.max_work.min(ceiling);
    }

    pub(crate) fn pending(&self) -> (&[u8], Range<usize>) {
        (&self.buffer, self.start..self.position)
    }

    /// Reuse scratch only at a complete between-declaration boundary.
    pub(crate) fn restart_fragment(&mut self) {
        debug_assert!(
            self.mode == ScanMode::SubsetFragment
                && self.ready.is_none()
                && self.buffer.is_empty()
                && matches!(self.state, State::Between)
        );
        self.start = 0;
        self.position = 0;
    }

    pub(crate) fn position(&self) -> usize {
        self.position
    }

    /// The ready unit remains available until release, including after EOF calls.
    pub(crate) fn current(&self) -> Option<(ScanKind, &[u8], Range<usize>)> {
        self.ready
            .map(|kind| (kind, self.buffer.as_slice(), self.start..self.position))
    }

    /// Releases the ready unit.
    pub(crate) fn release(&mut self) {
        self.ready = None;
        self.buffer.clear();
    }

    /// Stop at one unit; the controller advances only without a ready unit
    /// and before the scanner completes or finalizes.
    pub(crate) fn advance(&mut self, input: &[u8]) -> Result<usize, Error> {
        let mut index = 0;
        while index < input.len() {
            // Bytes that leave the state as it is are taken as one run.
            let run = self.run(&input[index..]);
            if run != 0 {
                self.append_run(&input[index..index + run])?;
                index += run;
                continue;
            }
            let byte = input[index];
            if matches!(self.state, State::Whitespace) && !space(byte) {
                self.ready = Some(ScanKind::Whitespace);
                self.state = State::Between;
                return Ok(index);
            }
            self.append(byte)?;
            self.consume(byte)?;
            index += 1;
            if self.ready.is_some() {
                return Ok(index);
            }
        }
        Ok(input.len())
    }

    /// The length of the run at the front of `input` that `consume` would
    /// read without a transition (comments and instructions drop their
    /// pending closing marks).
    fn run(&self, input: &[u8]) -> usize {
        let until = |found: Option<usize>| found.unwrap_or(input.len());
        match self.state {
            State::Header(Some(quote)) | State::Declaration(Some(quote)) => {
                until(memchr(quote, input))
            }
            State::Header(None) => until(
                input
                    .iter()
                    .position(|byte| matches!(byte, b'\'' | b'"' | b'[' | b'>')),
            ),
            State::Declaration(None) => until(memchr3(b'\'', b'"', b'>', input)),
            State::Comment(_) => until(memchr2(b'-', b'>', input)),
            State::Instruction(_) => until(memchr2(b'?', b'>', input)),
            State::ParameterReference => until(memchr(b';', input)),
            State::Whitespace => input.iter().take_while(|byte| space(**byte)).count(),
            _ => 0,
        }
    }

    /// `append`s a run of bytes that `run` found leave the state as it is.
    fn append_run(&mut self, run: &[u8]) -> Result<(), Error> {
        // The first byte past either ceiling fails as `append` fails it.
        let work = self.max_work.saturating_sub(self.position);
        let unit = self.max_unit_bytes.saturating_sub(self.buffer.len());
        if run.len() > work.min(unit) {
            let fits = work.min(unit);
            self.append_run(&run[..fits])?;
            let byte = run[fits];
            return self.append(byte);
        }
        if self.buffer.len() + run.len() > self.buffer.capacity() {
            let needed = self.buffer.len() + run.len();
            let target = self
                .buffer
                .capacity()
                .saturating_mul(2)
                .max(64)
                .max(needed)
                .min(self.max_unit_bytes);
            self.buffer
                .try_reserve_exact(target - self.buffer.len())
                .map_err(|_| self.error(ErrorKind::Allocation))?;
        }
        if self.buffer.is_empty() {
            self.start = self.position;
        }
        self.buffer.extend_from_slice(run);
        self.position += run.len();
        self.state = match self.state {
            State::Comment(_) => State::Comment(0),
            State::Instruction(_) => State::Instruction(false),
            state => state,
        };
        Ok(())
    }

    /// Declare final EOF, draining a trailing whitespace unit before closure checks.
    pub(crate) fn finish_step(&mut self) -> Result<(), Error> {
        match self.state {
            State::Whitespace => {
                self.ready = Some(ScanKind::Whitespace);
                self.state = State::Between;
                Ok(())
            }
            State::Between if self.mode == ScanMode::SubsetFragment => Ok(()),
            _ => {
                let raw = self.validate_utf8()?;
                if matches!(self.state, State::ParameterReference) && raw.len() > 1 {
                    Self::validate_parameter_name(&raw[1..], self.start + 1)?;
                }
                Err(self.error(ErrorKind::UnexpectedEof))
            }
        }
    }

    fn error(&self, kind: ErrorKind) -> Error {
        Error::new(kind, self.position)
    }

    fn append(&mut self, byte: u8) -> Result<(), Error> {
        let next = self
            .position
            .checked_add(1)
            .filter(|position| *position <= self.max_work)
            .ok_or_else(|| self.error(ErrorKind::LimitExceeded(Limit::EntityExpansion)))?;
        let needed = self
            .buffer
            .len()
            .checked_add(1)
            .filter(|length| *length <= self.max_unit_bytes)
            .ok_or_else(|| self.error(ErrorKind::LimitExceeded(Limit::TokenBytes)))?;
        if needed > self.buffer.capacity() {
            let target = self
                .buffer
                .capacity()
                .saturating_mul(2)
                .max(64)
                .max(needed)
                .min(self.max_unit_bytes);
            self.buffer
                .try_reserve_exact(target - self.buffer.len())
                .map_err(|_| self.error(ErrorKind::Allocation))?;
        }
        if self.buffer.is_empty() {
            self.start = self.position;
        }
        self.buffer.push(byte);
        self.position = next;
        Ok(())
    }

    fn consume(&mut self, byte: u8) -> Result<(), Error> {
        self.state = match self.state {
            State::Header(Some(quote)) if byte == quote => State::Header(None),
            State::Header(Some(quote)) => State::Header(Some(quote)),
            State::Header(None) => match byte {
                b'\'' | b'"' => State::Header(Some(byte)),
                b'[' => {
                    self.mark(ScanKind::Header)?;
                    State::Between
                }
                b'>' => {
                    self.mark(ScanKind::Header)?;
                    State::Between
                }
                _ => State::Header(None),
            },
            State::Between => match byte {
                byte if space(byte) => State::Whitespace,
                b'<' => State::LessThan,
                b'%' => State::ParameterReference,
                b']' if self.mode == ScanMode::Doctype => State::Closing,
                _ => return Err(self.syntax()),
            },
            State::Whitespace => State::Whitespace,
            State::LessThan => match byte {
                b'!' => State::Bang,
                b'?' => State::Instruction(false),
                // libxml2 reports (and recovery resumes at) the `<` that
                // starts no declaration.
                _ => return Err(Error::new(ErrorKind::InvalidDtd, self.position - 2)),
            },
            State::Bang => match byte {
                b'-' => State::CommentDash,
                // Conditional sections belong to a future external-PE profile.
                b'[' => return Err(self.syntax()),
                byte => self.declaration_byte(None, byte)?,
            },
            State::CommentDash if byte == b'-' => State::Comment(0),
            State::CommentDash => return Err(self.syntax()),
            State::Declaration(quote) => self.declaration_byte(quote, byte)?,
            State::Comment(2) if byte == b'>' => {
                self.mark(ScanKind::Comment)?;
                State::Between
            }
            State::Comment(hyphens) => State::Comment(if byte == b'-' {
                (hyphens + 1).min(2)
            } else {
                0
            }),
            State::Instruction(true) if byte == b'>' => {
                self.mark(ScanKind::Instruction)?;
                State::Between
            }
            State::Instruction(_) => State::Instruction(byte == b'?'),
            State::ParameterReference if byte == b';' => {
                self.mark(ScanKind::ParameterReference)?;
                State::Between
            }
            State::ParameterReference => State::ParameterReference,
            State::Closing if byte == b'>' => {
                self.mark(ScanKind::End)?;
                State::Between
            }
            State::Closing if space(byte) => State::Closing,
            State::Closing => return Err(self.syntax()),
        };
        Ok(())
    }

    fn declaration_byte(&mut self, quote: Option<u8>, byte: u8) -> Result<State, Error> {
        match (quote, byte) {
            (Some(quote), byte) if byte == quote => Ok(State::Declaration(None)),
            (Some(quote), _) => Ok(State::Declaration(Some(quote))),
            (None, b'\'' | b'"') => Ok(State::Declaration(Some(byte))),
            (None, b'>') => {
                self.mark(ScanKind::Declaration)?;
                Ok(State::Between)
            }
            (None, _) => Ok(State::Declaration(None)),
        }
    }

    fn syntax(&self) -> Error {
        Error::new(ErrorKind::InvalidDtd, self.position - 1)
    }

    fn validate_utf8(&self) -> Result<&str, Error> {
        std::str::from_utf8(&self.buffer)
            .map_err(|error| Error::new(ErrorKind::InvalidUtf8, self.start + error.valid_up_to()))
    }

    fn validate_parameter_name(name: &str, offset: usize) -> Result<(), Error> {
        validate_name(name, offset, true)
            .map_err(|error| Error::new(ErrorKind::InvalidDtd, error.offset))
    }

    fn mark(&mut self, kind: ScanKind) -> Result<(), Error> {
        let raw = self.validate_utf8()?;
        if kind == ScanKind::ParameterReference {
            Self::validate_parameter_name(&raw[1..raw.len() - 1], self.start + 1)?;
        }
        self.ready = Some(kind);
        Ok(())
    }
}
