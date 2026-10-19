//! One bounded lexical reference. No lookup, replacement or normalization.
use crate::escape::{invalid_char, xml_char};
use crate::name::validate_name;
use std::ops::Range;

#[derive(Clone, Copy, Debug)]
pub(crate) struct ReferenceLimits {
    pub(crate) max_bytes: usize,
    pub(crate) max_scratch_bytes: usize,
    pub(crate) max_work: usize,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ReferenceUsage {
    pub(crate) scanned: usize,
    pub(crate) copied: usize,
    pub(crate) classified: usize,
    pub(crate) work: usize,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReferenceKind {
    Numeric(char),
    Predefined(char),
    General,
}

/// A spelling whose complete reference grammar has already been checked.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ValidatedReference<'a> {
    kind: ReferenceKind,
    raw: &'a str,
    start: usize,
}

impl<'a> ValidatedReference<'a> {
    pub(crate) fn kind(self) -> ReferenceKind {
        self.kind
    }
    pub(crate) fn raw(self) -> &'a str {
        self.raw
    }
    pub(crate) fn range(self) -> Range<usize> {
        self.start..self.start + self.raw.len()
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReferenceLimit {
    Bytes,
    ScratchBytes,
    Work,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReferenceErrorKind {
    Syntax,
    InvalidUtf8,
    InvalidCharacter,
    UnexpectedEof,
    Limit(ReferenceLimit),
    PositionOverflow,
    Allocation,
    InvalidState,
    Sealed,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ReferenceError {
    pub(crate) kind: ReferenceErrorKind,
    pub(crate) offset: usize,
}

enum ReferenceBuffer {
    Pending(Vec<u8>),
    Ready { kind: ReferenceKind, raw: String },
}

pub(crate) struct ReferenceScratch {
    buffer: ReferenceBuffer,
    start: usize,
    position: usize,
    limits: ReferenceLimits,
    usage: ReferenceUsage,
    complete: bool,
    finalizing: bool,
    failure: Option<ReferenceError>,
}
impl ReferenceScratch {
    pub(crate) fn new(start: usize, limits: ReferenceLimits) -> Self {
        Self {
            buffer: ReferenceBuffer::Pending(Vec::new()),
            start,
            position: start,
            limits,
            usage: ReferenceUsage::default(),
            complete: false,
            finalizing: false,
            failure: None,
        }
    }
    pub(crate) fn cap_work(&mut self, ceiling: usize) {
        self.limits.max_work = self.limits.max_work.min(ceiling);
    }
    pub(crate) fn position(&self) -> usize {
        self.position
    }
    /// Whether a fresh, complete `&...;` spelling can be appended without
    /// reaching any ceiling (two work units per byte plus classification).
    pub(crate) fn fits(&self, spelling: &[u8]) -> bool {
        let length = spelling.len();
        self.buffer_len() == 0
            && spelling.first() == Some(&b'&')
            && length <= self.limits.max_bytes.min(self.limits.max_scratch_bytes)
            && self.start.checked_add(length).is_some()
            && self
                .usage
                .work
                .checked_add(length.saturating_mul(3))
                .is_some_and(|work| work <= self.limits.max_work)
    }
    pub(crate) fn usage(&self) -> ReferenceUsage {
        self.usage
    }
    pub(crate) fn collected_bytes(&self) -> &[u8] {
        match &self.buffer {
            ReferenceBuffer::Pending(bytes) => bytes,
            ReferenceBuffer::Ready { raw, .. } => raw.as_bytes(),
        }
    }
    pub(crate) fn validated(&self) -> Option<ValidatedReference<'_>> {
        if self.failure.is_some() {
            return None;
        }
        match &self.buffer {
            ReferenceBuffer::Ready { kind, raw } => Some(ValidatedReference {
                kind: *kind,
                raw,
                start: self.start,
            }),
            ReferenceBuffer::Pending(_) => None,
        }
    }
    pub(crate) fn release(&mut self) {
        self.buffer =
            match std::mem::replace(&mut self.buffer, ReferenceBuffer::Pending(Vec::new())) {
                ReferenceBuffer::Ready { raw, .. } => {
                    let mut bytes = raw.into_bytes();
                    bytes.clear();
                    ReferenceBuffer::Pending(bytes)
                }
                pending => pending,
            };
    }
    pub(crate) fn restart(&mut self, start: usize) -> Result<(), ReferenceError> {
        self.remember(|s| {
            if !s.complete
                || !matches!(&s.buffer, ReferenceBuffer::Pending(bytes) if bytes.is_empty())
            {
                return Err(s.error(ReferenceErrorKind::InvalidState, s.position()));
            }
            s.start = start;
            s.position = start;
            s.complete = false;
            s.finalizing = false;
            Ok(())
        })
    }
    pub(crate) fn advance(&mut self, input: &[u8]) -> Result<usize, ReferenceError> {
        self.remember(|s| {
            if s.finalizing && !input.is_empty() {
                return Err(s.error(ReferenceErrorKind::Sealed, s.position()));
            }
            if s.complete {
                return Ok(0);
            }
            for (index, &byte) in input.iter().enumerate() {
                s.append(byte)?;
                if s.buffer_len() == 1 && byte != b'&' {
                    return Err(s.error(ReferenceErrorKind::Syntax, s.start));
                }
                if byte == b';' {
                    s.classify(true)?;
                    s.complete = true;
                    return Ok(index + 1);
                }
            }
            Ok(input.len())
        })
    }
    pub(crate) fn finish_step(&mut self) -> Result<(), ReferenceError> {
        self.remember(|s| {
            s.finalizing = true;
            if s.complete {
                return Ok(());
            }
            s.classify(false)?;
            Err(s.error(ReferenceErrorKind::UnexpectedEof, s.position()))
        })
    }
    fn append(&mut self, byte: u8) -> Result<(), ReferenceError> {
        let length = self
            .buffer_len()
            .checked_add(1)
            .ok_or_else(|| self.error(ReferenceErrorKind::PositionOverflow, self.position()))?;
        let end = self
            .start
            .checked_add(length)
            .ok_or_else(|| self.error(ReferenceErrorKind::PositionOverflow, self.position()))?;
        let work = self
            .usage
            .work
            .checked_add(2)
            .filter(|n| *n <= self.limits.max_work)
            .ok_or_else(|| {
                self.error(
                    ReferenceErrorKind::Limit(ReferenceLimit::Work),
                    self.position(),
                )
            })?;
        if length > self.limits.max_bytes {
            return Err(self.error(
                ReferenceErrorKind::Limit(ReferenceLimit::Bytes),
                self.position(),
            ));
        }
        if length > self.limits.max_scratch_bytes {
            return Err(self.error(
                ReferenceErrorKind::Limit(ReferenceLimit::ScratchBytes),
                self.position(),
            ));
        }
        let allocation = self.error(ReferenceErrorKind::Allocation, self.position());
        let invalid = self.error(ReferenceErrorKind::InvalidState, self.position());
        let ReferenceBuffer::Pending(bytes) = &mut self.buffer else {
            return Err(invalid);
        };
        if length > bytes.capacity() {
            let target = bytes
                .capacity()
                .saturating_mul(2)
                .max(32)
                .max(length)
                .min(self.limits.max_bytes)
                .min(self.limits.max_scratch_bytes);
            bytes
                .try_reserve_exact(target - bytes.len())
                .map_err(|_| allocation)?;
        }
        self.usage.work = work;
        self.usage.scanned += 1;
        self.usage.copied += 1;
        bytes.push(byte);
        self.position = end;
        Ok(())
    }
    fn classify(&mut self, closed: bool) -> Result<ReferenceKind, ReferenceError> {
        let length = self.buffer_len();
        self.usage.work = self
            .usage
            .work
            .checked_add(length)
            .filter(|n| *n <= self.limits.max_work)
            .ok_or_else(|| {
                self.error(ReferenceErrorKind::Limit(ReferenceLimit::Work), self.start)
            })?;
        self.usage.classified += length;
        let ReferenceBuffer::Pending(bytes) = &mut self.buffer else {
            return Err(self.error(ReferenceErrorKind::InvalidState, self.start));
        };
        let raw = match String::from_utf8(std::mem::take(bytes)) {
            Ok(raw) => raw,
            Err(error) => {
                let offset = self.start + error.utf8_error().valid_up_to();
                self.buffer = ReferenceBuffer::Pending(error.into_bytes());
                return Err(self.error(ReferenceErrorKind::InvalidUtf8, offset));
            }
        };
        let result = self.classify_spelling(&raw, closed);
        self.buffer = match result {
            Ok(kind) if closed => ReferenceBuffer::Ready { kind, raw },
            _ => ReferenceBuffer::Pending(raw.into_bytes()),
        };
        result
    }
    fn buffer_len(&self) -> usize {
        match &self.buffer {
            ReferenceBuffer::Pending(bytes) => bytes.len(),
            ReferenceBuffer::Ready { raw, .. } => raw.len(),
        }
    }
    fn classify_spelling(&self, raw: &str, closed: bool) -> Result<ReferenceKind, ReferenceError> {
        if let Some(offset) = invalid_char(raw) {
            return Err(self.error(ReferenceErrorKind::InvalidCharacter, self.start + offset));
        }
        if raw.is_empty() {
            return Err(self.error(ReferenceErrorKind::UnexpectedEof, self.start));
        }
        let body = if closed {
            &raw[1..raw.len() - 1]
        } else {
            &raw[1..]
        };
        if body.is_empty() {
            return if closed {
                Err(self.error(ReferenceErrorKind::Syntax, self.start + 1))
            } else {
                Ok(ReferenceKind::General)
            };
        }
        if let Some(number) = body.strip_prefix('#') {
            let (digits, radix, offset) = if let Some(hex) = number.strip_prefix('x') {
                (hex, 16, 3)
            } else {
                (number, 10, 2)
            };
            if let Some((at, _)) = digits.bytes().enumerate().find(|(_, b)| {
                if radix == 16 {
                    !b.is_ascii_hexdigit()
                } else {
                    !b.is_ascii_digit()
                }
            }) {
                return Err(self.error(ReferenceErrorKind::Syntax, self.start + offset + at));
            }
            if !closed {
                return Ok(ReferenceKind::General);
            }
            if digits.is_empty() {
                return Err(self.error(ReferenceErrorKind::Syntax, self.start + offset));
            }
            let value = u32::from_str_radix(digits, radix)
                .ok()
                .and_then(char::from_u32)
                .filter(|c| xml_char(*c))
                .ok_or_else(|| self.error(ReferenceErrorKind::InvalidCharacter, self.start))?;
            return Ok(ReferenceKind::Numeric(value));
        }
        validate_name(body, self.start + 1, true)
            .map_err(|e| self.error(ReferenceErrorKind::Syntax, e.offset))?;
        Ok(match body {
            "lt" => ReferenceKind::Predefined('<'),
            "gt" => ReferenceKind::Predefined('>'),
            "amp" => ReferenceKind::Predefined('&'),
            "apos" => ReferenceKind::Predefined('\''),
            "quot" => ReferenceKind::Predefined('"'),
            _ => ReferenceKind::General,
        })
    }
    fn error(&self, kind: ReferenceErrorKind, offset: usize) -> ReferenceError {
        ReferenceError { kind, offset }
    }
    fn remember<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<T, ReferenceError>,
    ) -> Result<T, ReferenceError> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        let result = f(self);
        if let Err(error) = result {
            self.failure = Some(error)
        }
        result
    }
}
