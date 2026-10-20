//! `setters!` generates one chainable setter per field, named after the field
//! (fields and methods live in different namespaces). The field list is the
//! single place a config struct's settable fields are written down; the
//! exhaustive destructuring below fails to compile when a field is missing.
//! `const` setters need fields without drop glue; `fn` ones take any field.

macro_rules! setters {
    ($kw:ident $name:ident $([$($g:tt),*])? { $($f:ident : $t:ty),* $(,)? }) => {
        impl $(<$($g),*>)? $name $(<$($g),*>)? {
            $( setters!(@one $kw $f $t); )*
        }
        const _: () = {
            #[allow(dead_code)]
            fn every_field_has_a_setter $(<$($g),*>)? (s: $name $(<$($g),*>)?) {
                let $name { $($f: _),* } = s;
            }
        };
    };
    (@one const $f:ident $t:ty) => {
        #[doc = concat!("Sets [`", stringify!($f), "`](#structfield.", stringify!($f), ").")]
        #[must_use]
        pub const fn $f(mut self, $f: $t) -> Self {
            self.$f = $f;
            self
        }
    };
    (@one fn $f:ident $t:ty) => {
        #[doc = concat!("Sets [`", stringify!($f), "`](#structfield.", stringify!($f), ").")]
        #[must_use]
        pub fn $f(mut self, $f: $t) -> Self {
            self.$f = $f;
            self
        }
    };
}
