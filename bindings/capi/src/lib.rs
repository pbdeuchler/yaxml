//! yaxml's C ABI (include/yaxml.h; reference in docs/bindings.md#c-abi).
//!
//! Handles are registry tokens ([`registry`]), so misuse of a handle is an
//! error code, never undefined behavior. All unsafe operations live in
//! [`ffi`]; each exported function is an `unsafe extern "C" fn` whose body
//! runs inside [`error::guard`], so no panic crosses the boundary.
#![deny(unsafe_op_in_unsafe_fn)]
#![deny(clippy::undocumented_unsafe_blocks)]

mod doc;
mod edit;
mod error;
mod ffi;
mod options;
pub mod registry;
mod stream;
pub mod types;

pub use doc::*;
pub use edit::*;
pub use stream::*;

use types::yaxml_error;

pub const OK: i32 = 0;
pub const NONE: i32 = 1;
pub const ERR_NULL: i32 = -1;
pub const ERR_ARGUMENT: i32 = -2;
pub const ERR_HANDLE: i32 = -3;
pub const ERR_BUSY: i32 = -4;
pub const ERR_POISONED: i32 = -5;
pub const ERR_PANIC: i32 = -6;
pub const ERR_XML: i32 = -7;
pub const ERR_EDIT: i32 = -8;
pub const ERR_XPATH: i32 = -9;
pub const ERR_SCHEMA: i32 = -10;
pub const ERR_C14N: i32 = -11;
pub const ERR_CALLBACK: i32 = -12;

const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "\0");

/// The yaxml version, NUL-terminated and static.
#[unsafe(no_mangle)]
pub extern "C" fn yaxml_version() -> *const std::ffi::c_char {
    VERSION.as_ptr().cast()
}

/// libxml2's `xmlHasFeature` with its `XML_WITH_*` numbers.
#[unsafe(no_mangle)]
pub extern "C" fn yaxml_has_feature(feature: i32) -> i32 {
    u32::try_from(feature)
        .is_ok_and(yaxml::features::has_code)
        .into()
}

/// Frees a buffer, array or message the library returned. NULL, unknown and
/// already freed pointers are ignored.
#[unsafe(no_mangle)]
pub extern "C" fn yaxml_free(ptr: *const std::ffi::c_void) {
    // Unwinding cannot start here (a map removal and drops of plain data),
    // but the guard keeps the rule uniform.
    let _ = error::guard(std::ptr::null_mut::<yaxml_error>(), || {
        registry::release(ptr.addr());
        Ok(OK)
    });
}

/// Live handles and allocations, for leak tests and the fuzzer.
#[doc(hidden)]
pub fn live() -> (usize, usize) {
    (doc::live() + stream::live(), registry::live_allocations())
}

/// Counts the live handles and the allocations not yet freed. Both are held
/// by library tables, which leak checkers see as reachable, so a test that
/// frees everything checks both are zero at exit.
///
/// # Safety
/// See include/yaxml.h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn yaxml_live_objects(handles: *mut usize, allocations: *mut usize) -> i32 {
    error::guard(std::ptr::null_mut(), || {
        let (handles, allocations) = (ffi::Out::new(handles)?, ffi::Out::new(allocations)?);
        let (live_handles, live_allocations) = live();
        // SAFETY: yaxml.h: `handles` is valid for writing a size_t.
        unsafe { handles.put(live_handles) };
        // SAFETY: yaxml.h: `allocations` is valid for writing a size_t.
        unsafe { allocations.put(live_allocations) };
        Ok(OK)
    })
}
