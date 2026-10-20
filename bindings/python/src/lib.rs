//! `yaxml._yaxml`: the native half of the `yaxml` Python package, an
//! lxml.etree-style API over the safe yaxml core. PyO3 contains every unsafe
//! line; this crate forbids `unsafe`.

use pyo3::prelude::*;

mod doc;
mod element;
mod errors;
mod parse;
mod query;
mod validate;

#[pymodule]
fn _yaxml(m: &Bound<'_, PyModule>) -> PyResult<()> {
    element::register(m)?;
    parse::register(m)?;
    query::register(m)?;
    validate::register(m)?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add("LIBXML2_COMPAT_VERSION", yaxml::LIBXML2_COMPAT_VERSION)?;
    m.add("YAXML_VERSION", yaxml::VERSION)?;
    Ok(())
}
