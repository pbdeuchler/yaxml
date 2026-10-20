//! Validators: XML Schema, RELAX NG, DTD and Schematron. Each compiles once
//! and validates documents with the GIL released, returning
//! `(valid, error-log rows)`.

use pyo3::prelude::*;
use yaxml::diagnostic::{Diagnostic, Domain, Level};
use yaxml::{LineIndex, relaxng, schematron, valid, xsd};

use crate::doc::Tree;
use crate::errors::{Row, raise, row};
use crate::parse::{or_none, resolver};
use crate::query::target;

fn rows<D>(
    tree: &Tree,
    diagnostics: &[D],
    describe: impl Fn(&D, &LineIndex<'_>) -> Diagnostic,
) -> Vec<Row> {
    let empty = LineIndex::new(b"");
    let lines = tree.lines.as_ref().unwrap_or(&empty);
    let url = tree.doc.url();
    diagnostics
        .iter()
        .map(|d| row(&describe(d, lines), url))
        .collect()
}

/// A schema-parse row from a message and an offset in the schema text.
fn schema_row(
    domain: Domain,
    code: u16,
    warning: bool,
    message: &str,
    offset: Option<usize>,
    lines: &LineIndex<'_>,
    uri: &str,
) -> Row {
    let level = if warning {
        Level::Warning
    } else {
        Level::Error
    };
    let line = offset.map_or(0, |offset| lines.line(offset));
    let message = message.trim_end_matches('\n').to_owned();
    (
        message,
        domain as u8,
        code,
        level as u8,
        line,
        0,
        Some(uri.to_owned()),
    )
}

fn parse_error(py: Python<'_>, class: &str, rows: Vec<Row>) -> PyErr {
    let message = (rows.iter())
        .find(|r| r.3 >= Level::Error as u8)
        .or(rows.first())
        .map_or_else(|| "invalid schema".to_owned(), |r| r.0.clone());
    raise(py, class, (message, rows))
}

fn validate_with<F>(
    py: Python<'_>,
    document: &Bound<'_, PyAny>,
    run: F,
) -> PyResult<(bool, Vec<Row>)>
where
    F: FnOnce(&Tree) -> (bool, Vec<Row>) + Send,
{
    let (doc, _) = target(py, document)?;
    let tree = doc.read(py);
    let tree = &*tree;
    Ok(py.detach(|| run(tree)))
}

#[pyclass(frozen, module = "yaxml", name = "_XMLSchema")]
pub struct XmlSchema {
    schema: xsd::Schema,
}

#[pymethods]
impl XmlSchema {
    #[new]
    #[pyo3(signature = (data, uri, resolver = None))]
    fn new(
        py: Python<'_>,
        data: Vec<u8>,
        uri: &str,
        resolver: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        let (resolver, failure) = self::resolver(resolver)?;
        let built =
            py.detach(|| xsd::Schema::with_resolver(&data, uri, resolver, xsd::Limits::DEFAULT));
        let schema = built.map_err(|e| {
            if let Some(error) = failure.lock().ok().and_then(|mut f| f.take()) {
                return error;
            }
            let lines = LineIndex::new(&data);
            let rows = (e.diagnostics.iter())
                .map(|d| {
                    let lines = if d.uri == uri {
                        &lines
                    } else {
                        &LineIndex::new(b"")
                    };
                    schema_row(
                        Domain::SchemasParser,
                        d.code as u16,
                        d.code.is_warning(),
                        &d.message,
                        d.offset,
                        lines,
                        &d.uri,
                    )
                })
                .collect();
            parse_error(py, "XMLSchemaParseError", rows)
        })?;
        Ok(Self { schema })
    }

    fn validate(&self, py: Python<'_>, document: &Bound<'_, PyAny>) -> PyResult<(bool, Vec<Row>)> {
        validate_with(py, document, |tree| {
            let diagnostics = self.schema.validate(&tree.doc);
            (
                xsd::is_valid(&diagnostics),
                rows(tree, &diagnostics, |d, l| d.describe(l)),
            )
        })
    }
}

#[pyclass(frozen, module = "yaxml", name = "_RelaxNG")]
pub struct RelaxNg {
    schema: relaxng::Schema,
}

#[pymethods]
impl RelaxNg {
    #[new]
    #[pyo3(signature = (data, uri, resolver = None))]
    fn new(
        py: Python<'_>,
        data: Vec<u8>,
        uri: &str,
        resolver: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        let (resolver, failure) = self::resolver(resolver)?;
        let resolver = or_none(resolver);
        let built =
            py.detach(|| relaxng::Schema::parse(&data, uri, resolver, &relaxng::Limits::DEFAULT));
        let schema = built.map_err(|e| {
            if let Some(error) = failure.lock().ok().and_then(|mut f| f.take()) {
                return error;
            }
            let lines = LineIndex::new(&data);
            let rows = (e.0.iter())
                .map(|d| {
                    let lines = if d.uri == uri {
                        &lines
                    } else {
                        &LineIndex::new(b"")
                    };
                    schema_row(
                        Domain::SchemasParser,
                        d.code as u16,
                        false,
                        &d.message,
                        d.offset,
                        lines,
                        &d.uri,
                    )
                })
                .collect();
            parse_error(py, "RelaxNGParseError", rows)
        })?;
        Ok(Self { schema })
    }

    fn validate(&self, py: Python<'_>, document: &Bound<'_, PyAny>) -> PyResult<(bool, Vec<Row>)> {
        validate_with(py, document, |tree| {
            let diagnostics = self.schema.validate(&tree.doc);
            (
                diagnostics.is_empty(),
                rows(tree, &diagnostics, |d, l| d.describe(l)),
            )
        })
    }
}

#[pyclass(frozen, module = "yaxml", name = "_DTD")]
pub struct Dtd {
    dtd: valid::Dtd,
}

#[pymethods]
impl Dtd {
    #[new]
    fn new(py: Python<'_>, data: Vec<u8>) -> PyResult<Self> {
        let parsed = py.detach(|| valid::Dtd::parse(&data));
        let dtd = parsed.map_err(|e| {
            let d = e.describe(&LineIndex::new(&data));
            raise(
                py,
                "DTDParseError",
                (d.message.clone(), vec![row(&d, None)]),
            )
        })?;
        Ok(Self { dtd })
    }

    fn validate(&self, py: Python<'_>, document: &Bound<'_, PyAny>) -> PyResult<(bool, Vec<Row>)> {
        validate_with(py, document, |tree| {
            let diagnostics = valid::validate(&tree.doc, &self.dtd);
            (
                valid::is_valid(&diagnostics),
                rows(tree, &diagnostics, |d, l| d.describe(l)),
            )
        })
    }

    #[getter]
    fn name(&self) -> Option<String> {
        self.dtd.name().map(str::to_owned)
    }

    /// Declared element names, in declaration order.
    fn elements(&self) -> Vec<String> {
        self.dtd.elements().map(|e| e.name.to_owned()).collect()
    }
}

#[pyclass(frozen, module = "yaxml", name = "_Schematron")]
pub struct Schematron {
    schema: schematron::Schema,
}

#[pymethods]
impl Schematron {
    #[new]
    #[pyo3(signature = (data, uri, resolver = None))]
    fn new(
        py: Python<'_>,
        data: Vec<u8>,
        uri: &str,
        resolver: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        let (resolver, _) = self::resolver(resolver)?;
        let resolver = or_none(resolver);
        let built = py.detach(|| {
            schematron::Schema::parse(&data, uri, resolver, &schematron::Limits::DEFAULT)
        });
        let schema = built.map_err(|e| {
            let rows = (e.errors.iter())
                .map(|d| row(&d.describe(), Some(&e.uri)))
                .collect();
            parse_error(py, "SchematronParseError", rows)
        })?;
        Ok(Self { schema })
    }

    fn validate(&self, py: Python<'_>, document: &Bound<'_, PyAny>) -> PyResult<(bool, Vec<Row>)> {
        validate_with(py, document, |tree| {
            let source = tree.lines.as_ref().map_or(&[][..], LineIndex::input);
            let flags = schematron::OUT_QUIET | schematron::OUT_ERROR;
            let result = self.schema.validate(&tree.doc, flags, source);
            let rows = (result.output.iter())
                .filter_map(|output| match output {
                    schematron::Output::Error(d) => Some(row(&d.describe(), tree.doc.url())),
                    schematron::Output::Text(_) => None,
                })
                .collect();
            (result.is_valid(), rows)
        })
    }
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<XmlSchema>()?;
    m.add_class::<RelaxNg>()?;
    m.add_class::<Dtd>()?;
    m.add_class::<Schematron>()
}
