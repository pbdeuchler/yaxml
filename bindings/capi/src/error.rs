//! Failures, their libxml2 diagnostics, and the panic guard every exported
//! function runs in.

use std::panic::{AssertUnwindSafe, catch_unwind};

use yaxml::{EditError, EditErrorKind, Error, LineIndex, diagnostic::Domain};

use crate::ffi::Out;
use crate::registry::buffer;
use crate::types::{yaxml_diagnostic, yaxml_error, yaxml_str};
use crate::*;

/// A failed call: its status and the diagnostic `yaxml_error` carries.
#[derive(Debug)]
pub(crate) struct Fail {
    pub status: i32,
    pub offset: u64,
    pub diagnostic: Diagnostic,
}

/// A `yaxml_diagnostic` with an owned message.
#[derive(Debug, Default)]
pub(crate) struct Diagnostic {
    pub domain: i32,
    pub code: i32,
    pub level: i32,
    pub line: u64,
    pub column: u64,
    pub message: String,
}

impl From<&yaxml::Diagnostic> for Diagnostic {
    fn from(d: &yaxml::Diagnostic) -> Self {
        Self {
            domain: d.domain as i32,
            code: d.code.into(),
            level: d.level as i32,
            line: d.line as u64,
            column: d.column as u64,
            message: d.message.clone(),
        }
    }
}

impl Diagnostic {
    pub(crate) fn is_error(&self) -> bool {
        self.level >= yaxml::diagnostic::Level::Error as i32
    }

    /// The C record, its message pointing at `message`.
    pub(crate) fn record(&self, message: yaxml_str) -> yaxml_diagnostic {
        yaxml_diagnostic {
            domain: self.domain,
            code: self.code,
            level: self.level,
            reserved: 0,
            line: self.line,
            column: self.column,
            message,
        }
    }
}

const XPATH: i32 = Domain::XPath as i32;
const C14N: i32 = Domain::C14n as i32;
pub(crate) const EDIT: i32 = 1000;

impl Fail {
    pub(crate) fn new(status: i32, message: impl Into<String>) -> Self {
        Self::with(status, 0, 0, message)
    }

    pub(crate) fn with(status: i32, domain: i32, code: i32, message: impl Into<String>) -> Self {
        Self {
            status,
            offset: 0,
            diagnostic: Diagnostic {
                domain,
                code,
                level: 2,
                message: message.into(),
                ..Diagnostic::default()
            },
        }
    }

    pub(crate) fn null(what: &str) -> Self {
        Self::new(ERR_NULL, what)
    }

    pub(crate) fn argument(what: &str) -> Self {
        Self::new(ERR_ARGUMENT, what)
    }

    pub(crate) fn handle() -> Self {
        Self::new(ERR_HANDLE, "unknown, freed or stale handle")
    }

    /// A parse error, described from the input it occurred in.
    pub(crate) fn xml(error: Error, input: &[u8]) -> Self {
        Self::described(ERR_XML, error, &error.describe(&LineIndex::new(input)))
    }

    /// An XML error without its input: the context-free libxml2 code.
    pub(crate) fn kind(error: Error) -> Self {
        let (domain, code) = error.kind.libxml2_code();
        let mut fail = Self::with(ERR_XML, domain as i32, code.into(), error.to_string());
        fail.diagnostic.level = 3;
        fail.offset = error.offset as u64;
        fail
    }

    pub(crate) fn described(status: i32, error: Error, d: &yaxml::Diagnostic) -> Self {
        Self {
            status,
            offset: error.offset as u64,
            diagnostic: d.into(),
        }
    }

    pub(crate) fn edit(error: EditError) -> Self {
        use EditErrorKind::*;
        let code = match error.kind {
            InvalidHandle => return Self::handle(),
            WrongNodeKind => 2,
            ProtectedNode => 3,
            MissingAttribute => 4,
            NamespaceDeclaration => 5,
            InvalidCharacter => 6,
            InvalidValue => 7,
            InvalidName => 8,
            Namespace => 9,
            DuplicateAttribute => 10,
            Hierarchy => 11,
            IdentifierExhausted => 12,
            LimitExceeded(_) => 13,
            Allocation => 14,
            _ => 0,
        };
        let mut fail = Self::with(ERR_EDIT, EDIT, code, format!("{error:?}"));
        fail.diagnostic.column = error.value_offset.map_or(0, |at| at as u64 + 1);
        fail
    }

    pub(crate) fn xpath(error: yaxml::xpath::Error, expression: &str) -> Self {
        let (code, message) = error.libxml2(expression);
        let mut fail = Self::with(ERR_XPATH, XPATH, code.into(), message);
        fail.offset = error.offset.unwrap_or(0) as u64;
        fail
    }

    pub(crate) fn c14n(error: yaxml::c14n::Error) -> Self {
        use yaxml::c14n::Error::*;
        let code = match &error {
            InvalidUri(_) => 91,
            RelativeNamespace(_) => 1955,
            Io(_) => 1546,
            Allocation => 2,
            _ => 1953,
        };
        Self::with(ERR_C14N, C14N, code, error.to_string())
    }

    pub(crate) fn write(error: yaxml::WriteError) -> Self {
        match error {
            yaxml::WriteError::Xml(error) => Self::kind(error),
            other => Self::new(ERR_CALLBACK, other.to_string()),
        }
    }

    fn panic(payload: &(dyn std::any::Any + Send)) -> Self {
        let text = (payload.downcast_ref::<&str>().copied())
            .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
            .unwrap_or("panic");
        Self::new(ERR_PANIC, format!("internal error: {text}"))
    }

    /// The C record; its message is a new allocation.
    fn record(&self) -> yaxml_error {
        let message = buffer(self.diagnostic.message.clone().into_bytes());
        yaxml_error {
            status: self.status,
            reserved: 0,
            offset: self.offset,
            diagnostic: self.diagnostic.record(yaxml_str {
                ptr: message.ptr,
                len: message.len,
            }),
        }
    }
}

/// What a successful call returns: `YAXML_OK` or `YAXML_NONE`.
pub(crate) type Outcome = Result<i32, Fail>;

/// Runs an exported function's body: a panic becomes `YAXML_ERR_PANIC`
/// (the registry has poisoned the handle it held), and a failure is written
/// to `err` unless it is NULL or misaligned.
pub(crate) fn guard(err: *mut yaxml_error, body: impl FnOnce() -> Outcome) -> i32 {
    let fail = match catch_unwind(AssertUnwindSafe(body)) {
        Ok(Ok(status)) => return status,
        Ok(Err(fail)) => fail,
        Err(payload) => Fail::panic(payload.as_ref()),
    };
    if let Ok(Some(out)) = Out::optional(err) {
        // SAFETY: yaxml.h: `err` is NULL or valid for writing a yaxml_error.
        unsafe { out.put(fail.record()) };
    }
    fail.status
}

#[cfg(test)]
mod tests {
    use super::{C14N, Fail};
    use crate::ERR_C14N;
    use yaxml::c14n::Error;

    #[test]
    fn c14n_errors_map_to_libxml2_codes() {
        let cases = [
            (Error::Allocation, 2),
            (Error::InvalidUri("u".into()), 91),
            (Error::RelativeNamespace("r".into()), 1955),
            (Error::NotXml("x".into()), 1953),
        ];
        for (error, code) in cases {
            let fail = Fail::c14n(error);
            assert_eq!((fail.status, fail.diagnostic.domain), (ERR_C14N, C14N));
            assert_eq!(fail.diagnostic.code, code);
        }
    }
}
