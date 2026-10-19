//! The one switch for allocator-aware document storage.
//!
//! With the `allocator_api` feature (Rust 1.100) this re-exports the standard
//! allocator API. Without it, a sealed stand-in keeps the same generic code
//! compiling on older compilers: `Global` is its only allocator and `Vec` wraps
//! an ordinary vector, so nothing outside the crate can name either.

#[cfg(feature = "allocator_api")]
pub(crate) use std::alloc::{Allocator, Global};

/// Vector storage charged to `A`.
#[cfg(feature = "allocator_api")]
pub(crate) type Vec<T, A> = std::vec::Vec<T, A>;

#[cfg(not(feature = "allocator_api"))]
pub(crate) use sealed::{Allocator, Global, Vec};

#[cfg(not(feature = "allocator_api"))]
mod sealed {
    use std::fmt;
    use std::marker::PhantomData;
    use std::ops::{Deref, DerefMut};

    pub trait Allocator {}

    #[derive(Clone, Copy, Debug, Default)]
    pub struct Global;

    impl Allocator for Global {}

    /// `std::vec::Vec<T, A>`'s subset that yaxml uses, over the global heap.
    pub struct Vec<T, A>(std::vec::Vec<T>, PhantomData<A>);

    impl<T, A> Vec<T, A> {
        pub fn new_in(_: A) -> Self {
            Self(std::vec::Vec::new(), PhantomData)
        }
    }

    impl<T, A> Deref for Vec<T, A> {
        type Target = std::vec::Vec<T>;

        #[inline]
        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }

    impl<T, A> DerefMut for Vec<T, A> {
        #[inline]
        fn deref_mut(&mut self) -> &mut Self::Target {
            &mut self.0
        }
    }

    impl<T: fmt::Debug, A> fmt::Debug for Vec<T, A> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.0.fmt(f)
        }
    }
}
