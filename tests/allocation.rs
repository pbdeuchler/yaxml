//! How each subsystem reports a refused allocation: the error variants and
//! diagnostic codes, their libxml2 numbers and messages, and the conversions
//! from `TryReserveError`, next to the neighbouring variants they must not be
//! confused with (docs/security.md#allocation-failure). The refusals
//! themselves are injected by yaxml-harness's `allocfail` target.

use std::collections::TryReserveError;

use yaxml::xpath::{Error as XPathError, ErrorKind as XPathKind, Limit};
use yaxml::{c14n, catalog, regexp, relaxng, schematron, valid, xsd};

/// A real `TryReserveError`: a capacity no allocator can satisfy.
fn refused() -> TryReserveError {
    Vec::<u8>::new().try_reserve(usize::MAX).unwrap_err()
}

#[test]
fn xpath_allocation_has_libxml2_memory_error() {
    let cases = [
        (XPathKind::Allocation, 1215, "Memory allocation error"),
        (
            XPathKind::LimitExceeded(Limit::Nodes),
            1225,
            "Operation limit exceeded",
        ),
        (XPathKind::Expression, 1207, "Invalid expression"),
    ];
    for (kind, code, message) in cases {
        let error = XPathError::new(kind);
        assert_eq!(error.libxml2(""), (code, message.to_owned()), "{kind:?}");
    }
    assert_eq!(
        XPathError::from(refused()),
        XPathError::new(XPathKind::Allocation)
    );
}

#[test]
fn errors_display_allocation_failures() {
    let cases: [(&dyn std::error::Error, &str); 6] = [
        (&c14n::Error::Allocation, "C14N error: allocation failed"),
        (&c14n::Error::NotXml("x".into()), "C14N error: x"),
        (
            &regexp::Error::Allocation,
            "regular expression allocation failed",
        ),
        (
            &regexp::Error::TooComplex,
            "regular expression exceeds the compile budget",
        ),
        (&catalog::Error::Allocation, "catalog allocation failed"),
        (
            &catalog::Error::LimitExceeded,
            "catalog load limit exceeded",
        ),
    ];
    for (error, text) in cases {
        assert_eq!(error.to_string(), text);
        assert!(error.source().is_none(), "{text}");
    }
}

#[test]
fn try_reserve_errors_convert_to_allocation() {
    assert_eq!(regexp::Error::from(refused()), regexp::Error::Allocation);
    assert_eq!(catalog::Error::from(refused()), catalog::Error::Allocation);
}

#[test]
fn diagnostic_codes_carry_libxml2_memory_numbers() {
    // XML_ERR_NO_MEMORY, XML_RELAXNG_ERR_MEMORY, and a neighbour of each.
    let cases = [
        (valid::Code::Allocation as u16, 2),
        (valid::Code::UndeclaredEntity as u16, 26),
        (xsd::Code::Allocation as u16, 2),
        (xsd::Code::AttrFormDefaultValue as u16, 1701),
        (relaxng::Code::Allocation as u16, 1),
        (relaxng::Code::Datatype as u16, 2),
        (relaxng::SchemaCode::Allocation as u16, 2),
        (relaxng::SchemaCode::ResourceLimit as u16, 114),
        (schematron::Code::Allocation.number(), 2),
        (schematron::Code::Internal.number(), 1),
    ];
    for (index, (code, number)) in cases.into_iter().enumerate() {
        assert_eq!(code, number, "case {index}");
    }
    // An allocation failure is an error, never a warning.
    assert!(!valid::Code::Allocation.is_warning());
    assert!(!xsd::Code::Allocation.is_warning());
    assert!(valid::Code::AttributeRedefined.is_warning());
}
