//! Bounded framing of decoded internal-DTD input; not declaration semantics.

use std::ops::Range;

use crate::name::validate_name;
use crate::parser::space;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ScanMode {
    Doctype,
    SubsetFragment,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ScanLimits {
    pub(crate) max_unit_bytes: usize,
    pub(crate) max_scratch_bytes: usize,
    pub(crate) max_work: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ScanLimit {
    UnitBytes,
    ScratchBytes,
    Work,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ScanErrorKind {
    Syntax,
    UnexpectedEof,
    InvalidUtf8,
    Limit(ScanLimit),
    Allocation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ScanError {
    pub(crate) kind: ScanErrorKind,
    pub(crate) offset: usize,
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
    HeaderPrefix(usize),
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
    limits: ScanLimits,
    state: State,
    buffer: Vec<u8>,
    start: usize,
    position: usize,
    ready: Option<ScanKind>,
    complete: bool,
    finalizing: bool,
    failure: Option<ScanError>,
}

impl DtdScanner {
    pub(crate) fn new(mode: ScanMode, limits: ScanLimits) -> Self {
        Self {
            mode,
            limits,
            state: match mode {
                ScanMode::Doctype => State::HeaderPrefix(0),
                ScanMode::SubsetFragment => State::Between,
            },
            buffer: Vec::new(),
            start: 0,
            position: 0,
            ready: None,
            complete: false,
            finalizing: false,
            failure: None,
        }
    }

    pub(crate) fn cap_work(&mut self, ceiling: usize) {
        self.limits.max_work = self.limits.max_work.min(ceiling);
    }

    pub(crate) fn pending(&self) -> (&[u8], Range<usize>) {
        (&self.buffer, self.start..self.position)
    }

    /// Reuse scratch only at a complete between-declaration boundary.
    pub(crate) fn restart_fragment(&mut self) -> Result<(), ScanError> {
        self.remember(|scanner| {
            if scanner.mode != ScanMode::SubsetFragment
                || scanner.ready.is_some()
                || !scanner.buffer.is_empty()
                || !matches!(scanner.state, State::Between)
            {
                return Err(scanner.error(ScanErrorKind::Syntax));
            }
            scanner.start = 0;
            scanner.position = 0;
            scanner.complete = false;
            scanner.finalizing = false;
            Ok(())
        })
    }

    pub(crate) fn position(&self) -> usize {
        self.position
    }

    /// The ready unit remains available until release, including after EOF calls.
    pub(crate) fn current(&self) -> Option<(ScanKind, &[u8], Range<usize>)> {
        self.ready
            .map(|kind| (kind, self.buffer.as_slice(), self.start..self.position))
    }

    pub(crate) fn release(&mut self) {
        if self.ready.take().is_some() {
            self.buffer.clear();
        }
    }

    /// Stop at one unit; ready, completed, or finalizing scanners consume no suffix.
    pub(crate) fn advance(&mut self, input: &[u8]) -> Result<usize, ScanError> {
        self.remember(|scanner| scanner.advance_inner(input))
    }

    fn advance_inner(&mut self, input: &[u8]) -> Result<usize, ScanError> {
        if self.ready.is_some() || self.complete || self.finalizing {
            return Ok(0);
        }
        for (index, &byte) in input.iter().enumerate() {
            if matches!(self.state, State::Whitespace) && !space(byte) {
                self.mark(ScanKind::Whitespace)?;
                self.state = State::Between;
                return Ok(index);
            }
            self.append(byte)?;
            self.consume(byte)?;
            if self.ready.is_some() {
                return Ok(index + 1);
            }
        }
        Ok(input.len())
    }

    /// Declare final EOF, draining a trailing whitespace unit before closure checks.
    pub(crate) fn finish_step(&mut self) -> Result<(), ScanError> {
        self.remember(|scanner| {
            scanner.finalizing = true;
            if scanner.ready.is_some() || scanner.complete {
                return Ok(());
            }
            match scanner.state {
                State::Whitespace => {
                    scanner.mark(ScanKind::Whitespace)?;
                    scanner.state = State::Between;
                    Ok(())
                }
                State::Between if scanner.mode == ScanMode::SubsetFragment => {
                    scanner.complete = true;
                    Ok(())
                }
                _ => {
                    let raw = scanner.validate_utf8()?;
                    if matches!(scanner.state, State::ParameterReference) && raw.len() > 1 {
                        Self::validate_parameter_name(&raw[1..], scanner.start + 1)?;
                    }
                    Err(scanner.error(ScanErrorKind::UnexpectedEof))
                }
            }
        })
    }

    fn remember<T>(
        &mut self,
        operation: impl FnOnce(&mut Self) -> Result<T, ScanError>,
    ) -> Result<T, ScanError> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        let result = operation(self);
        if let Err(error) = result {
            self.failure = Some(error);
        }
        result
    }

    fn error(&self, kind: ScanErrorKind) -> ScanError {
        ScanError {
            kind,
            offset: self.position,
        }
    }

    fn append(&mut self, byte: u8) -> Result<(), ScanError> {
        let next = self
            .position
            .checked_add(1)
            .filter(|position| *position <= self.limits.max_work)
            .ok_or_else(|| self.error(ScanErrorKind::Limit(ScanLimit::Work)))?;
        let needed = self
            .buffer
            .len()
            .checked_add(1)
            .filter(|length| *length <= self.limits.max_unit_bytes)
            .ok_or_else(|| self.error(ScanErrorKind::Limit(ScanLimit::UnitBytes)))?;
        if needed > self.limits.max_scratch_bytes {
            return Err(self.error(ScanErrorKind::Limit(ScanLimit::ScratchBytes)));
        }
        if needed > self.buffer.capacity() {
            let target = self
                .buffer
                .capacity()
                .saturating_mul(2)
                .max(64)
                .max(needed)
                .min(self.limits.max_unit_bytes)
                .min(self.limits.max_scratch_bytes);
            self.buffer
                .try_reserve_exact(target - self.buffer.len())
                .map_err(|_| self.error(ScanErrorKind::Allocation))?;
        }
        if self.buffer.is_empty() {
            self.start = self.position;
        }
        self.buffer.push(byte);
        self.position = next;
        Ok(())
    }

    fn consume(&mut self, byte: u8) -> Result<(), ScanError> {
        self.state = match self.state {
            State::HeaderPrefix(index) => {
                if byte != b"<!DOCTYPE"[index] {
                    return Err(self.syntax());
                }
                if index + 1 == b"<!DOCTYPE".len() {
                    State::Header(None)
                } else {
                    State::HeaderPrefix(index + 1)
                }
            }
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
                    self.complete = true;
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
                _ => return Err(self.syntax()),
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
                self.complete = true;
                State::Between
            }
            State::Closing if space(byte) => State::Closing,
            State::Closing => return Err(self.syntax()),
        };
        Ok(())
    }

    fn declaration_byte(&mut self, quote: Option<u8>, byte: u8) -> Result<State, ScanError> {
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

    fn syntax(&self) -> ScanError {
        ScanError {
            kind: ScanErrorKind::Syntax,
            offset: self.position - 1,
        }
    }

    fn validate_utf8(&self) -> Result<&str, ScanError> {
        std::str::from_utf8(&self.buffer).map_err(|error| ScanError {
            kind: ScanErrorKind::InvalidUtf8,
            offset: self.start + error.valid_up_to(),
        })
    }

    fn validate_parameter_name(name: &str, offset: usize) -> Result<(), ScanError> {
        validate_name(name, offset, true).map_err(|error| ScanError {
            kind: ScanErrorKind::Syntax,
            offset: error.offset,
        })
    }

    fn mark(&mut self, kind: ScanKind) -> Result<(), ScanError> {
        let raw = self.validate_utf8()?;
        if kind == ScanKind::ParameterReference {
            Self::validate_parameter_name(&raw[1..raw.len() - 1], self.start + 1)?;
        }
        self.ready = Some(kind);
        Ok(())
    }
}
