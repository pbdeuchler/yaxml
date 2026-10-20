//! Handle and allocation tables. A C handle is a token, never a Rust
//! pointer: it indexes a table of live objects, so a freed, stale or invented
//! handle is an error rather than a use-after-free, and a second free finds
//! nothing. Tokens come from one counter and are never reused. Allocations
//! handed to C are kept here by address until `yaxml_free`.

use std::any::Any;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock, TryLockError};

use crate::error::Fail;
use crate::types::yaxml_buffer;
use crate::{ERR_BUSY, ERR_NULL, ERR_POISONED};

static NEXT: AtomicUsize = AtomicUsize::new(1);

/// One live object behind a handle.
pub(crate) struct Entry<T> {
    poisoned: AtomicBool,
    value: RwLock<T>,
}

/// Live objects of one handle type.
pub(crate) struct Registry<T> {
    map: RwLock<BTreeMap<usize, Arc<Entry<T>>>>,
}

impl<T> Registry<T> {
    pub(crate) const fn new() -> Self {
        Self {
            map: RwLock::new(BTreeMap::new()),
        }
    }

    /// Stores `value` and returns its token, a C pointer value.
    pub(crate) fn insert<H>(&self, value: T) -> *mut H {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let entry = Arc::new(Entry {
            poisoned: AtomicBool::new(false),
            value: RwLock::new(value),
        });
        self.map
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, entry);
        std::ptr::without_provenance_mut(id)
    }

    pub(crate) fn len(&self) -> usize {
        self.map
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    pub(crate) fn get<H>(&self, handle: *mut H) -> Result<Arc<Entry<T>>, Fail> {
        if handle.is_null() {
            return Err(Fail::new(ERR_NULL, "NULL handle"));
        }
        let map = self.map.read().unwrap_or_else(PoisonError::into_inner);
        map.get(&handle.addr()).cloned().ok_or_else(Fail::handle)
    }

    /// Drops the table's reference; a call in flight keeps its own.
    pub(crate) fn remove<H>(&self, handle: *mut H) -> Result<(), Fail> {
        if handle.is_null() {
            return Err(Fail::new(ERR_NULL, "NULL handle"));
        }
        let entry =
            (self.map.write().unwrap_or_else(PoisonError::into_inner)).remove(&handle.addr());
        entry.map(drop).ok_or_else(Fail::handle)
    }
}

/// Marks the entry poisoned if the access unwinds.
struct PoisonOnUnwind<'a>(&'a AtomicBool);

impl Drop for PoisonOnUnwind<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.0.store(true, Ordering::Relaxed);
        }
    }
}

impl<T> Entry<T> {
    /// Shared access; never waits: an exclusive holder makes it `BUSY`.
    pub(crate) fn read<R>(&self, f: impl FnOnce(&T) -> Result<R, Fail>) -> Result<R, Fail> {
        self.check()?;
        let guard = self.value.try_read().map_err(busy)?;
        let _poison = PoisonOnUnwind(&self.poisoned);
        f(&guard)
    }

    /// Exclusive access; never waits: any other holder makes it `BUSY`.
    pub(crate) fn write<R>(&self, f: impl FnOnce(&mut T) -> Result<R, Fail>) -> Result<R, Fail> {
        self.check()?;
        let mut guard = self.value.try_write().map_err(busy)?;
        let _poison = PoisonOnUnwind(&self.poisoned);
        f(&mut guard)
    }

    fn check(&self) -> Result<(), Fail> {
        match self.poisoned.load(Ordering::Relaxed) {
            true => Err(Fail::new(
                ERR_POISONED,
                "an earlier call panicked on this handle",
            )),
            false => Ok(()),
        }
    }
}

fn busy<G>(error: TryLockError<G>) -> Fail {
    match error {
        TryLockError::WouldBlock => Fail::new(ERR_BUSY, "handle in use by another call"),
        // Only a panic inside `write` poisons the lock, and that also set the flag.
        TryLockError::Poisoned(_) => Fail::new(ERR_POISONED, "an earlier call panicked"),
    }
}

type Owner = Box<dyn Any + Send>;

static ALLOCATIONS: Mutex<BTreeMap<usize, Owner>> = Mutex::new(BTreeMap::new());

/// Keeps `owner` alive until `yaxml_free(address)`.
pub(crate) fn hand_out(address: *const u8, owner: Owner) {
    (ALLOCATIONS.lock().unwrap_or_else(PoisonError::into_inner)).insert(address.addr(), owner);
}

/// Frees what `hand_out` kept at `address`; anything else is ignored.
pub(crate) fn release(address: usize) {
    let owner = (ALLOCATIONS.lock().unwrap_or_else(PoisonError::into_inner)).remove(&address);
    drop(owner);
}

/// Bytes handed to C; empty is the NULL buffer, which owns nothing.
pub(crate) fn buffer(mut bytes: Vec<u8>) -> yaxml_buffer {
    if bytes.is_empty() {
        return yaxml_buffer::default();
    }
    let (ptr, len) = (bytes.as_mut_ptr(), bytes.len());
    hand_out(ptr, Box::new(bytes));
    yaxml_buffer { ptr, len }
}

/// Live allocations, for leak tests.
#[doc(hidden)]
pub fn live_allocations() -> usize {
    (ALLOCATIONS.lock().unwrap_or_else(PoisonError::into_inner)).len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::guard;
    use crate::types::yaxml_error;
    use crate::{ERR_HANDLE, ERR_PANIC, OK};

    static TABLE: Registry<i32> = Registry::new();

    fn status(err: &mut yaxml_error, body: impl FnOnce() -> Result<i32, Fail>) -> i32 {
        guard(err, body)
    }

    #[test]
    fn a_panic_becomes_an_error_and_poisons_the_handle() {
        for exclusive in [false, true] {
            let handle: *mut u8 = TABLE.insert(7);
            let mut err = yaxml_error::default();
            let panicked = status(&mut err, || {
                let entry = TABLE.get(handle)?;
                match exclusive {
                    true => entry.write(|_| -> Result<i32, Fail> { panic!("boom") }),
                    false => entry.read(|_| -> Result<i32, Fail> { panic!("boom") }),
                }
            });
            assert_eq!((panicked, err.status), (ERR_PANIC, ERR_PANIC));
            let text = err.diagnostic.message;
            // SAFETY: the guard wrote a live message buffer.
            let bytes = unsafe { std::slice::from_raw_parts(text.ptr, text.len) };
            assert_eq!(bytes, b"internal error: boom");
            release(text.ptr.addr());
            let read = || TABLE.get(handle)?.read(|v| Ok(*v));
            assert_eq!(status(&mut err, read), ERR_POISONED);
            release(err.diagnostic.message.ptr.addr());
            assert!(TABLE.remove(handle).is_ok());
            assert_eq!(TABLE.remove(handle).unwrap_err().status, ERR_HANDLE);
        }
    }

    #[test]
    fn contended_access_is_busy_rather_than_blocking() {
        let handle: *mut u8 = TABLE.insert(1);
        let entry = TABLE.get(handle).unwrap();
        let nested = entry.read(|_| Ok(entry.write(|_| Ok(OK)).unwrap_err().status));
        assert_eq!(nested.unwrap(), ERR_BUSY);
        let shared = entry.read(|a| entry.read(|b| Ok(a + b)));
        assert_eq!(shared.unwrap(), 2);
        let nested = entry.write(|_| Ok(entry.read(|_| Ok(OK)).unwrap_err().status));
        assert_eq!(nested.unwrap(), ERR_BUSY);
        assert!(TABLE.remove(handle).is_ok());
        // A call in flight keeps its entry after the handle is freed.
        assert_eq!(entry.read(|v| Ok(*v)).unwrap(), 1);
    }

    #[test]
    fn buffers_are_released_once() {
        let empty = buffer(Vec::new());
        assert!(empty.ptr.is_null());
        let full = buffer(b"abc".to_vec());
        let held = || ALLOCATIONS.lock().unwrap().contains_key(&full.ptr.addr());
        assert!(held());
        release(full.ptr.addr());
        assert!(!held());
        release(full.ptr.addr());
    }
}
