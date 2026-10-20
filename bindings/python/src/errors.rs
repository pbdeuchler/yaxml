//! Host exceptions. The classes live in `yaxml/_errors.py` (Python needs
//! multiple inheritance for `XMLSyntaxError(Error, SyntaxError)`), so Rust
//! raises instances of them.

use pyo3::call::PyCallArgs;
use pyo3::exceptions::{PyKeyError, PyMemoryError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyList;
use yaxml::diagnostic::{Diagnostic, Domain, Level};
use yaxml::{EditError, EditErrorKind, LineIndex};

/// An instance of `yaxml._errors.<name>(*args)` as a `PyErr`.
pub fn raise<'py>(py: Python<'py>, name: &str, args: impl PyCallArgs<'py>) -> PyErr {
    let made = py
        .import("yaxml._errors")
        .and_then(|m| m.getattr(name))
        .and_then(|class| class.call1(args));
    match made {
        Ok(value) => PyErr::from_value(value),
        Err(error) => error,
    }
}

pub fn stale(py: Python<'_>) -> PyErr {
    raise(
        py,
        "StaleElementError",
        ("the element was removed from its document",),
    )
}

/// One error-log row: `(message, domain, type, level, line, column, filename)`,
/// the fields of lxml's `_LogEntry`.
pub type Row = (String, u8, u16, u8, usize, usize, Option<String>);

pub fn row(d: &Diagnostic, filename: Option<&str>) -> Row {
    (
        d.message.clone(),
        d.domain as u8,
        d.code,
        d.level as u8,
        d.line,
        d.column,
        filename.map(str::to_owned),
    )
}

/// `XMLSyntaxError` for a failed parse, described from its input.
pub fn syntax(py: Python<'_>, error: yaxml::Error, input: &[u8], filename: Option<&str>) -> PyErr {
    let d = error.describe(&LineIndex::new(input));
    syntax_from(py, &[row(&d, filename)])
}

/// `XMLSyntaxError` for the first error-level row; every row goes to its log.
pub fn syntax_from(py: Python<'_>, rows: &[Row]) -> PyErr {
    let first = rows
        .iter()
        .find(|r| r.3 >= Level::Error as u8)
        .or(rows.first())
        .cloned()
        .unwrap_or_else(|| {
            (
                "unknown error".into(),
                Domain::Parser as u8,
                1,
                3,
                0,
                0,
                None,
            )
        });
    let message = match first.4 {
        0 => first.0.clone(),
        line => format!("{}, line {line}, column {}", first.0, first.5),
    };
    let log = match PyList::new(py, rows) {
        Ok(log) => log,
        Err(error) => return error,
    };
    raise(
        py,
        "XMLSyntaxError",
        (message, first.2, first.4, first.5, first.6, first.1, log),
    )
}

/// libxml2 domain and code for an error raised without its input.
pub fn bare(py: Python<'_>, error: yaxml::Error) -> PyErr {
    let (domain, code) = error.kind.libxml2_code();
    let row = (
        error.to_string(),
        domain as u8,
        code,
        Level::Fatal as u8,
        0,
        0,
        None,
    );
    syntax_from(py, &[row])
}

pub fn edit(py: Python<'_>, error: EditError) -> PyErr {
    use EditErrorKind::*;
    let what = match error.kind {
        InvalidHandle => return stale(py),
        MissingAttribute => return PyKeyError::new_err("no such attribute"),
        WrongNodeKind => return PyTypeError::new_err("operation not supported on this node type"),
        Allocation => return PyMemoryError::new_err("allocation failed"),
        InvalidName => "Invalid name",
        InvalidCharacter => {
            "All strings must be XML compatible: Unicode or ASCII, no NULL bytes or control characters"
        }
        InvalidValue => "Invalid value for this node type",
        Hierarchy => {
            "Cannot insert there: an element cannot contain itself, and a document holds one element and no text"
        }
        ProtectedNode => "The document element and entity content cannot be moved or removed",
        Namespace => "Namespace conflict",
        NamespaceDeclaration => "Namespace declarations are not attributes",
        DuplicateAttribute => "Duplicate attribute or namespace declaration",
        LimitExceeded(_) | IdentifierExhausted => "Edit limit exceeded",
        _ => "Edit failed",
    };
    PyValueError::new_err(format!("{what} ({error})"))
}

pub trait EditResult<T> {
    fn py(self, py: Python<'_>) -> PyResult<T>;
}

impl<T> EditResult<T> for Result<T, EditError> {
    fn py(self, py: Python<'_>) -> PyResult<T> {
        self.map_err(|e| edit(py, e))
    }
}
