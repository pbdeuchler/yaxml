//! Every unsafe operation of the crate: reading caller memory, writing
//! out-parameters, calling caller functions, and the two `Send`/`Sync`
//! assertions for C records stored beside the strings they point to. The
//! exported functions only call these, each call under a `SAFETY:` comment
//! naming the yaxml.h contract it relies on.

use std::ffi::c_void;
use std::ptr::NonNull;

use crate::error::Fail;
use crate::types::{yaxml_attribute, yaxml_diagnostic, yaxml_event, yaxml_str};

/// `len` values at `ptr`; NULL with length 0 is empty.
///
/// # Safety
/// Unless NULL, `ptr` points to `len` initialized values that stay valid and
/// unmodified for `'a`.
pub(crate) unsafe fn slice<'a, T>(ptr: *const T, len: usize) -> Result<&'a [T], Fail> {
    if ptr.is_null() {
        return match len {
            0 => Ok(&[]),
            _ => Err(Fail::null("NULL pointer with a nonzero length")),
        };
    }
    if !ptr.is_aligned()
        || len
            .checked_mul(size_of::<T>())
            .is_none_or(|n| n > isize::MAX as usize)
    {
        return Err(Fail::argument("misaligned pointer or oversized length"));
    }
    // SAFETY: non-null, aligned and within isize::MAX bytes (checked above);
    // validity for `'a` is the caller's contract.
    Ok(unsafe { std::slice::from_raw_parts(ptr, len) })
}

/// One value at `ptr`, which must not be NULL.
///
/// # Safety
/// As [`slice`] with a length of 1.
pub(crate) unsafe fn get<'a, T>(ptr: *const T) -> Result<&'a T, Fail> {
    if ptr.is_null() {
        return Err(Fail::null("NULL argument"));
    }
    // SAFETY: forwarded contract.
    Ok(&unsafe { slice(ptr, 1) }?[0])
}

/// The UTF-8 text of `s`; `None` when `s.ptr` is NULL (absent).
///
/// # Safety
/// As [`slice`] for `s.ptr` and `s.len`.
pub(crate) unsafe fn text<'a>(s: yaxml_str) -> Result<Option<&'a str>, Fail> {
    if s.ptr.is_null() && s.len == 0 {
        return Ok(None);
    }
    // SAFETY: forwarded contract.
    let bytes = unsafe { slice(s.ptr.cast::<u8>(), s.len) }?;
    std::str::from_utf8(bytes)
        .map(Some)
        .map_err(|_| Fail::argument("string is not UTF-8"))
}

/// C structs that start with a `uint32_t struct_size`, grow only by
/// appending fields, and hold only integers and pointers.
pub(crate) trait Versioned: Copy + Default + sealed::Sealed {}

mod sealed {
    pub trait Sealed {}
    impl Sealed for crate::types::yaxml_parse_options {}
    impl Sealed for crate::types::yaxml_edit_limits {}
    impl Sealed for crate::types::yaxml_attribute {}
    impl Sealed for crate::types::yaxml_diagnostic {}
    impl Sealed for crate::types::yaxml_event {}
}

impl Versioned for crate::types::yaxml_parse_options {}
impl Versioned for crate::types::yaxml_edit_limits {}

/// The struct at `ptr` (`None` for NULL), read up to the caller's
/// `struct_size`: fields the caller's header lacks keep their zero default,
/// and bytes past the fields this library knows must be zero.
///
/// # Safety
/// Unless NULL, `ptr` points to `struct_size` readable bytes, the first four
/// of which are that size.
pub(crate) unsafe fn versioned<T: Versioned>(ptr: *const T) -> Result<Option<T>, Fail> {
    if ptr.is_null() {
        return Ok(None);
    }
    if !ptr.is_aligned() {
        return Err(Fail::argument("misaligned options"));
    }
    // SAFETY: aligned and non-null; every versioned struct starts with a u32.
    let size = unsafe { ptr.cast::<u32>().read() } as usize;
    if size < size_of::<T>() {
        return Err(Fail::argument(
            "struct_size is smaller than the first version",
        ));
    }
    // SAFETY: the caller's contract makes `size` bytes readable.
    let bytes = unsafe { slice(ptr.cast::<u8>(), size) }?;
    let (known, extra) = bytes.split_at(size_of::<T>());
    if extra.iter().any(|&b| b != 0) {
        return Err(Fail::argument("unknown nonzero fields after struct_size"));
    }
    let mut value = T::default();
    // SAFETY: `known` is exactly size_of::<T>() bytes and does not overlap
    // the local `value`; T (sealed above) is a repr(C) struct of integers and
    // raw pointers without padding, for which any bytes are a valid value.
    unsafe {
        std::ptr::copy_nonoverlapping(known.as_ptr(), (&raw mut value).cast::<u8>(), known.len())
    };
    Ok(Some(value))
}

/// A checked out-parameter: non-null and aligned.
pub(crate) struct Out<T>(NonNull<T>);

impl<T> Out<T> {
    pub(crate) fn new(ptr: *mut T) -> Result<Self, Fail> {
        match NonNull::new(ptr) {
            Some(ptr) if ptr.is_aligned() => Ok(Self(ptr)),
            Some(_) => Err(Fail::argument("misaligned out-parameter")),
            None => Err(Fail::null("NULL out-parameter")),
        }
    }

    /// An optional out-parameter: `None` for NULL.
    pub(crate) fn optional(ptr: *mut T) -> Result<Option<Self>, Fail> {
        match ptr.is_null() {
            true => Ok(None),
            false => Self::new(ptr).map(Some),
        }
    }

    /// Writes `value` without reading or dropping the old contents.
    ///
    /// # Safety
    /// The pointer is valid for a write of `T`.
    pub(crate) unsafe fn put(self, value: T) {
        // SAFETY: non-null and aligned (checked in `new`); writable per the
        // caller's contract.
        unsafe { self.0.as_ptr().write(value) }
    }
}

impl<T: Default> Out<T> {
    /// Writes `Some` and returns `YAXML_OK`, or writes the null value and
    /// returns `YAXML_NONE`.
    ///
    /// # Safety
    /// As [`Out::put`].
    pub(crate) unsafe fn reply(self, value: Option<T>) -> i32 {
        let status = if value.is_some() {
            crate::OK
        } else {
            crate::NONE
        };
        // SAFETY: forwarded contract.
        unsafe { self.put(value.unwrap_or_default()) };
        status
    }
}

pub(crate) type EventFn = unsafe extern "C" fn(*mut c_void, *const yaxml_event) -> i32;
pub(crate) type WriteFn = unsafe extern "C" fn(*mut c_void, *const u8, usize) -> i32;

/// Calls an event callback.
///
/// # Safety
/// `callback` is a function of this type that accepts `user`.
pub(crate) unsafe fn emit(callback: EventFn, user: *mut c_void, event: &yaxml_event) -> i32 {
    // SAFETY: the caller's contract; `event` outlives the call.
    unsafe { callback(user, event) }
}

/// Calls a write callback.
///
/// # Safety
/// `callback` is a function of this type that accepts `user`.
pub(crate) unsafe fn write(callback: WriteFn, user: *mut c_void, bytes: &[u8]) -> i32 {
    // SAFETY: the caller's contract; `bytes` outlives the call.
    unsafe { callback(user, bytes.as_ptr(), bytes.len()) }
}

/// C records (attributes, diagnostics, events) kept beside the strings their
/// pointers address, by the same owner.
pub(crate) struct Records<T>(pub(crate) Vec<T>);

/// The C records whose raw pointers make them `!Send`.
pub(crate) trait Record: sealed::Sealed {}
impl Record for yaxml_attribute {}
impl Record for yaxml_diagnostic {}
impl Record for yaxml_event {}

// SAFETY: the pointers in these records address heap strings owned by the
// same registry entry as the records; Rust never dereferences them, so moving
// the records to another thread is moving plain integers with their owner.
unsafe impl<T: Record> Send for Records<T> {}
// SAFETY: as for Send; shared access only copies the pointer values.
unsafe impl<T: Record> Sync for Records<T> {}
