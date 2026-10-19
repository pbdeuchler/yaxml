//! One bounded lexical reference. No lookup, replacement or normalization.
use crate::escape::{invalid_char, numeric_reference, predefined};
use crate::name::validate_name;
use crate::{Error, ErrorKind, Limit};
use std::ops::Range;

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

enum ReferenceBuffer {
    Pending(Vec<u8>),
    Ready { kind: ReferenceKind, raw: String },
}

/// Collects one reference spelling. Offsets are decoded positions; work is
/// two units per byte plus one per byte classified.
pub(crate) struct ReferenceScratch {
    buffer: ReferenceBuffer,
    start: usize,
    position: usize,
    max_bytes: usize,
    max_work: usize,
    work: usize,
    complete: bool,
}
impl ReferenceScratch {
    pub(crate) fn new(start: usize, max_bytes: usize, max_work: usize) -> Self {
        Self {
            buffer: ReferenceBuffer::Pending(Vec::new()),
            start,
            position: start,
            max_bytes,
            max_work,
            work: 0,
            complete: false,
        }
    }
    pub(crate) fn cap_work(&mut self, ceiling: usize) {
        self.max_work = self.max_work.min(ceiling);
    }
    pub(crate) fn position(&self) -> usize {
        self.position
    }
    pub(crate) fn work(&self) -> usize {
        self.work
    }
    /// Whether a fresh, complete `&...;` spelling can be appended without
    /// reaching any ceiling (two work units per byte plus classification).
    pub(crate) fn fits(&self, spelling: &[u8]) -> bool {
        let length = spelling.len();
        self.collected_bytes().is_empty()
            && spelling.first() == Some(&b'&')
            && length <= self.max_bytes
            && self.start.checked_add(length).is_some()
            && self
                .work
                .checked_add(length.saturating_mul(3))
                .is_some_and(|work| work <= self.max_work)
    }
    pub(crate) fn collected_bytes(&self) -> &[u8] {
        match &self.buffer {
            ReferenceBuffer::Pending(bytes) => bytes,
            ReferenceBuffer::Ready { raw, .. } => raw.as_bytes(),
        }
    }
    pub(crate) fn validated(&self) -> Option<ValidatedReference<'_>> {
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
        if let ReferenceBuffer::Ready { raw, .. } = &mut self.buffer {
            let mut bytes = std::mem::take(raw).into_bytes();
            bytes.clear();
            self.buffer = ReferenceBuffer::Pending(bytes);
        }
    }
    /// Reuses the scratch, released after a complete reference, at `start`.
    pub(crate) fn restart(&mut self, start: usize) {
        debug_assert!(self.complete && self.collected_bytes().is_empty());
        self.start = start;
        self.position = start;
        self.complete = false;
    }
    pub(crate) fn advance(&mut self, input: &[u8]) -> Result<usize, Error> {
        if self.complete {
            return Ok(0);
        }
        for (index, &byte) in input.iter().enumerate() {
            self.append(byte)?;
            if self.collected_bytes().len() == 1 && byte != b'&' {
                return Err(Error::new(ErrorKind::InvalidReference, self.start));
            }
            if byte == b';' {
                self.classify(true)?;
                self.complete = true;
                return Ok(index + 1);
            }
        }
        Ok(input.len())
    }
    /// Input ended inside the reference: report its first lexical error.
    pub(crate) fn finish_step(&mut self) -> Result<(), Error> {
        if self.complete {
            return Ok(());
        }
        self.classify(false)?;
        Err(Error::new(ErrorKind::UnexpectedEof, self.position))
    }
    fn append(&mut self, byte: u8) -> Result<(), Error> {
        let fail = |kind| Error::new(kind, self.position);
        let length = self.collected_bytes().len() + 1;
        let end = self
            .start
            .checked_add(length)
            .ok_or(fail(ErrorKind::InvalidState))?;
        let work = self
            .work
            .checked_add(2)
            .filter(|n| *n <= self.max_work)
            .ok_or(fail(ErrorKind::LimitExceeded(Limit::EntityExpansion)))?;
        if length > self.max_bytes {
            return Err(fail(ErrorKind::LimitExceeded(Limit::TokenBytes)));
        }
        let ReferenceBuffer::Pending(bytes) = &mut self.buffer else {
            return Err(fail(ErrorKind::InvalidState));
        };
        if length > bytes.capacity() {
            let target = bytes
                .capacity()
                .saturating_mul(2)
                .max(32)
                .max(length)
                .min(self.max_bytes);
            bytes
                .try_reserve_exact(target - bytes.len())
                .map_err(|_| fail(ErrorKind::Allocation))?;
        }
        self.work = work;
        bytes.push(byte);
        self.position = end;
        Ok(())
    }
    fn classify(&mut self, closed: bool) -> Result<ReferenceKind, Error> {
        self.work = self
            .work
            .checked_add(self.collected_bytes().len())
            .filter(|n| *n <= self.max_work)
            .ok_or(Error::new(
                ErrorKind::LimitExceeded(Limit::EntityExpansion),
                self.start,
            ))?;
        let ReferenceBuffer::Pending(bytes) = &mut self.buffer else {
            return Err(Error::new(ErrorKind::InvalidState, self.start));
        };
        let raw = match String::from_utf8(std::mem::take(bytes)) {
            Ok(raw) => raw,
            Err(error) => {
                let offset = self.start + error.utf8_error().valid_up_to();
                self.buffer = ReferenceBuffer::Pending(error.into_bytes());
                return Err(Error::new(ErrorKind::InvalidUtf8, offset));
            }
        };
        let result = classify(&raw, self.start, closed);
        self.buffer = match result {
            Ok(kind) if closed => ReferenceBuffer::Ready { kind, raw },
            _ => ReferenceBuffer::Pending(raw.into_bytes()),
        };
        result
    }
}

/// Classifies a spelling at decoded offset `start`, through its `;` when
/// `closed`; an unclosed prefix reports only errors already certain.
pub(crate) fn classify(raw: &str, start: usize, closed: bool) -> Result<ReferenceKind, Error> {
    let fail = |kind, offset| Err(Error::new(kind, offset));
    if let Some(offset) = invalid_char(raw) {
        return fail(ErrorKind::InvalidCharacter, start + offset);
    }
    if raw.is_empty() {
        return fail(ErrorKind::UnexpectedEof, start);
    }
    let body = match closed {
        true => &raw[1..raw.len() - 1],
        false => &raw[1..],
    };
    if body.is_empty() {
        return match closed {
            true => fail(ErrorKind::InvalidReference, start + 1),
            false => Ok(ReferenceKind::General),
        };
    }
    if let Some(number) = body.strip_prefix('#') {
        let (digits, radix, offset) = match number.strip_prefix('x') {
            Some(hex) => (hex, 16, 3),
            None => (number, 10, 2),
        };
        if let Some(at) = digits.bytes().position(|b| !char::from(b).is_digit(radix)) {
            return fail(ErrorKind::InvalidReference, start + offset + at);
        }
        if !closed {
            return Ok(ReferenceKind::General);
        }
        if digits.is_empty() {
            return fail(ErrorKind::InvalidReference, start + offset);
        }
        return numeric_reference(number)
            .map(ReferenceKind::Numeric)
            .ok_or(Error::new(ErrorKind::InvalidCharacter, start));
    }
    validate_name(body, start + 1, true)
        .map_err(|error| Error::new(ErrorKind::InvalidReference, error.offset))?;
    Ok(predefined(body).map_or(ReferenceKind::General, ReferenceKind::Predefined))
}
