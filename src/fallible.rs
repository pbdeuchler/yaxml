//! Growth the input sizes, failing instead of aborting when the allocator
//! refuses (docs/security.md#allocation-failure). Each subsystem maps
//! [`TryReserveError`] to its own allocation error.

use std::borrow::Cow;
use std::collections::TryReserveError;
use std::io;

/// `push` and `push_str` that report a refused allocation.
pub(crate) trait TryPush<T> {
    fn try_push(&mut self, item: T) -> Result<(), TryReserveError>;
}

impl<T> TryPush<T> for Vec<T> {
    #[inline]
    fn try_push(&mut self, item: T) -> Result<(), TryReserveError> {
        self.try_reserve(1)?;
        self.push(item);
        Ok(())
    }
}

impl TryPush<&str> for String {
    #[inline]
    fn try_push(&mut self, text: &str) -> Result<(), TryReserveError> {
        self.try_reserve(text.len())?;
        self.push_str(text);
        Ok(())
    }
}

/// A copy of `text`.
#[inline]
pub(crate) fn copied(text: &str) -> Result<String, TryReserveError> {
    concat(&[text])
}

/// A copy of `text` that borrows what it borrows.
pub(crate) fn cow<'a>(text: &Cow<'a, str>) -> Result<Cow<'a, str>, TryReserveError> {
    match text {
        Cow::Borrowed(text) => Ok(Cow::Borrowed(text)),
        Cow::Owned(text) => copied(text).map(Cow::Owned),
    }
}

/// `parts` joined, in one allocation.
#[inline]
pub(crate) fn concat(parts: &[&str]) -> Result<String, TryReserveError> {
    let mut joined = String::new();
    joined.try_reserve_exact(parts.iter().map(|part| part.len()).sum())?;
    parts.iter().for_each(|part| joined.push_str(part));
    Ok(joined)
}

/// A string built by many appends that fail once: growth the allocator
/// refuses is skipped and remembered in `refused`.
#[derive(Default)]
pub(crate) struct Text {
    pub(crate) text: String,
    pub(crate) refused: bool,
}

impl Text {
    #[inline]
    pub(crate) fn push_str(&mut self, text: &str) {
        if self.text.capacity() - self.text.len() < text.len() {
            return self.grow(text);
        }
        self.text.push_str(text);
    }

    #[inline]
    pub(crate) fn push(&mut self, c: char) {
        self.push_str(c.encode_utf8(&mut [0; 4]));
    }

    /// Growth, kept out of the appends' fast path.
    #[cold]
    #[inline(never)]
    fn grow(&mut self, text: &str) {
        self.refused |= self.text.try_push(text).is_err();
    }

    /// The text, unless some growth was refused.
    pub(crate) fn finish(self) -> Option<String> {
        (!self.refused).then_some(self.text)
    }
}

/// `n` copies of `value`.
pub(crate) fn filled<T: Clone>(value: T, n: usize) -> Result<Vec<T>, TryReserveError> {
    let mut items = Vec::new();
    items.try_reserve_exact(n)?;
    items.resize(n, value);
    Ok(items)
}

/// A copy of `items`.
pub(crate) fn cloned<T: Clone>(items: &[T]) -> Result<Vec<T>, TryReserveError> {
    let mut copy = Vec::new();
    copy.try_reserve_exact(items.len())?;
    copy.extend_from_slice(items);
    Ok(copy)
}

/// `items` collected into a vector.
pub(crate) fn collected<T>(items: impl IntoIterator<Item = T>) -> Result<Vec<T>, TryReserveError> {
    let mut vec = Vec::new();
    for item in items {
        vec.try_push(item)?;
    }
    Ok(vec)
}

/// An in-memory sink whose refused growth is an `OutOfMemory` write error.
#[derive(Default)]
pub(crate) struct Memory(pub(crate) Vec<u8>);

impl io::Write for Memory {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .try_reserve(bytes.len())
            .map_err(|_| io::ErrorKind::OutOfMemory)?;
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/internal/fallible.rs"]
mod tests;
