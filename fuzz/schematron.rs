//! Schematron schemas and instances. A testcase is NUL-separated: the schema,
//! then the instance, read with `XML_PARSE_NOENT` for half of the schemas
//! and keeping entity references otherwise. A schema piece that is not
//! markup is wrapped as the content of a rule whose context is `*`. Checks: `Schema::from_tree` over
//! both DOMs reports what `Schema::parse` does; every output mode gives the
//! same result over both DOMs and on reuse; the text and error modes report
//! the same messages; the one-pass mode counts what the error mode counts;
//! lines are known; and a smaller operation budget either runs out or
//! changes nothing.

use yaxml::schematron::{
    Code, Diagnostic, Limits, OUT_ERROR, OUT_QUIET, OUT_TEXT, OUT_XML, Output, Schema,
    SchemaErrors, Validation,
};
use yaxml::{Document, OwnedDocument, ParseOptions, Request, Resolver, options};

/// The schema document as `Schema::parse` reads it (`XML_PARSE_NOENT`).
fn schema_options() -> ParseOptions {
    let options = ParseOptions::new(options::NOENT).with_limits(limits(0).xml);
    options.with_resolver("s.sct", none())
}

const MODES: [u32; 6] = [
    0,
    OUT_QUIET,
    OUT_TEXT,
    OUT_XML,
    OUT_ERROR,
    OUT_ERROR | OUT_QUIET,
];

fn limits(max_operations: u64) -> Limits {
    let mut limits = Limits {
        max_operations,
        ..Limits::default()
    };
    limits.xpath.max_operations = 100_000;
    limits.xml.max_external_loads = 0;
    limits
}

fn none() -> Box<dyn Resolver> {
    Box::new(|_: Request<'_>| Ok(None))
}

fn parse(schema: &[u8], max_operations: u64) -> Result<Schema, SchemaErrors> {
    Schema::parse(schema, "s.sct", none(), &limits(max_operations))
}

fn text(result: &Result<Schema, SchemaErrors>) -> String {
    match result {
        Ok(_) => String::new(),
        Err(errors) => {
            assert!(!errors.errors.is_empty() && !errors.to_string().is_empty());
            errors.to_string()
        }
    }
}

fn messages<'a, I: 'a>(diagnostics: impl Iterator<Item = &'a Diagnostic<I>>) -> Vec<String> {
    diagnostics
        .filter(|d| matches!(d.code, Code::Assert | Code::Report))
        .map(|d| d.message.clone())
        .collect()
}

fn errors<I>(validation: &Validation<I>) -> impl Iterator<Item = &Diagnostic<I>> + '_ {
    validation.output.iter().filter_map(|output| match output {
        Output::Error(diagnostic) => Some(diagnostic),
        Output::Text(_) => None,
    })
}

pub fn exercise(input: &[u8]) {
    let mut pieces = input.splitn(2, |byte| *byte == 0);
    let schema = pieces.next().unwrap_or_default();
    let instance = pieces.next().unwrap_or_default();
    let wrapped;
    let schema = match schema.trim_ascii_start().first() {
        Some(b'<') => schema,
        _ => {
            wrapped = [
                b"<s:schema xmlns:s='http://purl.oclc.org/dsdl/schematron'><s:pattern id='p'><s:rule context='*'>".as_slice(),
                schema,
                b"</s:rule></s:pattern></s:schema>",
            ]
            .concat();
            &wrapped
        }
    };
    let compiled = parse(schema, 2_000_000);
    if let (Ok(borrowed), Ok(owned)) = (
        Document::parse_with(schema, schema_options()),
        OwnedDocument::parse_with(schema, schema_options()),
    ) {
        let from = |result| text(&result);
        let expected = text(&compiled);
        let limits = limits(2_000_000);
        assert_eq!(
            from(Schema::from_tree(&borrowed, "s.sct", schema, &limits)),
            expected
        );
        assert_eq!(
            from(Schema::from_tree(&owned, "s.sct", schema, &limits)),
            expected
        );
    }
    let Ok(compiled) = compiled else {
        return;
    };
    let (Ok(document), Ok(owned)) = (
        Document::parse_with(instance, instance_options(schema)),
        OwnedDocument::parse_with(instance, instance_options(schema)),
    ) else {
        return;
    };
    let results: Vec<_> = MODES
        .iter()
        .map(|&flags| {
            let validation = compiled.validate(&document, flags, instance);
            let rendered = validation.render("d.xml");
            assert_eq!(
                rendered,
                compiled.validate(&owned, flags, instance).render("d.xml"),
                "DOMs disagree"
            );
            assert_eq!(
                validation,
                compiled.validate(&document, flags, instance),
                "reuse changed the result"
            );
            for diagnostic in errors(&validation) {
                assert!(diagnostic.node.is_some() || diagnostic.line == 0);
            }
            validation
        })
        .collect();
    let [_, quiet, text, _, error, _] = &results[..] else {
        unreachable!()
    };
    // Text and error mode walk alike; only the channel differs.
    let reported: Vec<_> = (text.output.iter())
        .filter_map(|output| match output {
            Output::Text(line) if !line.starts_with("Pattern: ") => Some(line.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(reported, messages(errors(error)));
    assert_eq!(text.errors, error.errors);
    let internal = errors(error).any(|d| d.code == Code::Internal);
    if let (Some(count), false) = (error.errors, internal) {
        assert_eq!(count, messages(errors(error)).len());
        assert_eq!(quiet.errors, Some(count), "one pass counts differently");
    }
    // A smaller budget runs out or changes nothing.
    let hash = schema.iter().fold(0u64, |hash, &byte| {
        hash.wrapping_mul(31).wrapping_add(byte.into())
    });
    if let Ok(small) = parse(schema, hash % 5_000) {
        let validation = small.validate(&document, OUT_ERROR, instance);
        if validation.errors.is_some() {
            assert_eq!(&validation, error, "budget changed the result");
        }
    }
}

/// The instance keeps entity references (libxml2's default) or, for half
/// of the schemas, substitutes them (`XML_PARSE_NOENT`).
fn instance_options(schema: &[u8]) -> ParseOptions {
    let noent = schema.iter().fold(0u8, |hash, &byte| hash ^ byte) & 1 == 1;
    ParseOptions::new(if noent { options::NOENT } else { 0 })
}
