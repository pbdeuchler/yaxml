//! One bounded lexical reference. No lookup, replacement or normalization.
use crate::escape::xml_char;
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

pub(crate) struct ReferenceScratch {
    bytes: Vec<u8>,
    start: usize,
    position: usize,
    limits: ReferenceLimits,
    usage: ReferenceUsage,
    ready: Option<ReferenceKind>,
    complete: bool,
    finalizing: bool,
    failure: Option<ReferenceError>,
}
impl ReferenceScratch {
    pub(crate) fn new(start: usize, limits: ReferenceLimits) -> Self {
        Self {
            bytes: Vec::new(),
            start,
            position: start,
            limits,
            usage: ReferenceUsage::default(),
            ready: None,
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
    pub(crate) fn usage(&self) -> ReferenceUsage {
        self.usage
    }
    pub(crate) fn current(&self) -> Option<(ReferenceKind, &str, Range<usize>)> {
        if self.failure.is_some() {
            return None;
        }
        self.ready.map(|kind| {
            (
                kind,
                std::str::from_utf8(&self.bytes).expect("validated reference"),
                self.start..self.position(),
            )
        })
    }
    pub(crate) fn release(&mut self) {
        if self.ready.take().is_some() {
            self.bytes.clear();
        }
    }
    pub(crate) fn restart(&mut self, start: usize) -> Result<(), ReferenceError> {
        self.remember(|s| {
            if !s.complete || s.ready.is_some() || !s.bytes.is_empty() {
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
                if s.bytes.len() == 1 && byte != b'&' {
                    return Err(s.error(ReferenceErrorKind::Syntax, s.start));
                }
                if byte == b';' {
                    let kind = s.classify(true)?;
                    s.ready = Some(kind);
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
            .bytes
            .len()
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
        if length > self.bytes.capacity() {
            let target = self
                .bytes
                .capacity()
                .saturating_mul(2)
                .max(32)
                .max(length)
                .min(self.limits.max_bytes)
                .min(self.limits.max_scratch_bytes);
            self.bytes
                .try_reserve_exact(target - self.bytes.len())
                .map_err(|_| self.error(ReferenceErrorKind::Allocation, self.position()))?;
        }
        self.usage.work = work;
        self.usage.scanned += 1;
        self.usage.copied += 1;
        self.bytes.push(byte);
        self.position = end;
        Ok(())
    }
    fn classify(&mut self, closed: bool) -> Result<ReferenceKind, ReferenceError> {
        let length = self.bytes.len();
        self.usage.work = self
            .usage
            .work
            .checked_add(length)
            .filter(|n| *n <= self.limits.max_work)
            .ok_or_else(|| {
                self.error(ReferenceErrorKind::Limit(ReferenceLimit::Work), self.start)
            })?;
        self.usage.classified += length;
        let raw = std::str::from_utf8(&self.bytes).map_err(|e| {
            self.error(
                ReferenceErrorKind::InvalidUtf8,
                self.start + e.valid_up_to(),
            )
        })?;
        if let Some((offset, _)) = raw.char_indices().find(|(_, c)| !xml_char(*c)) {
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

#[cfg(test)]
#[path = "../tests/internal/dtd_reference_author.rs"]
mod tests;
