//! `yaxmllint`: xmllint's command line over yaxml. See `docs/yaxmllint.md`.

mod diag;
mod dump;
mod opts;

use std::io::{self, Read, Write};
use std::path::Path;
use std::time::Instant;
use std::{env, fs};

use diag::report;
use opts::{Opts, *};
use yaxml::catalog::{Catalog, CatalogResolver};
use yaxml::xpath::{Context, XPath};
use yaxml::{ErrorKind, Limit, Limits, OwnedDocument, Request, Resolver, relaxng, valid, xsd};

/// Compiled `--relaxng` and `--schema` documents, shared by every input.
struct Schemas {
    relaxng: Option<relaxng::Schema>,
    xsd: Option<xsd::Schema>,
}

fn limits(o: &Opts) -> Limits {
    let base = Limits::default();
    let huge = |value: usize| if o.huge { value << 6 } else { value };
    Limits {
        max_depth: if o.huge { 2048 } else { base.max_depth },
        max_token_bytes: huge(base.max_token_bytes),
        max_input_bytes: o
            .maxmem
            .map_or(base.max_input_bytes, |m| m.min(base.max_input_bytes)),
        max_owned_bytes: o.maxmem.unwrap_or_else(|| huge(base.max_owned_bytes)),
        max_entity_amplification: o.max_ampl.unwrap_or(base.max_entity_amplification),
        ..base
    }
}

/// Reads files from disk, trying the `--path` directories for the file name
/// when the path itself is missing. Anything but a file path is declined.
fn loader(paths: &[String]) -> Box<dyn Resolver> {
    let paths = paths.to_vec();
    Box::new(move |request: Request<'_>| {
        let uri = request.uri.strip_prefix("file://").unwrap_or(request.uri);
        let name = Path::new(uri).file_name().unwrap_or_default();
        let searched = paths.iter().map(|dir| Path::new(dir).join(name));
        let found = (!uri.contains("://")).then(|| {
            std::iter::once(uri.into())
                .chain(searched)
                .find_map(|p| fs::read(p).ok())
        });
        Ok(found.flatten())
    })
}

/// The loader, behind the catalogs named by `$XML_CATALOG_FILES` with `--catalogs`.
fn resolver(o: &Opts) -> Box<dyn Resolver> {
    let files = env::var("XML_CATALOG_FILES").unwrap_or_default();
    let entry = |file: &str| {
        let file = file
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('"', "&quot;");
        format!("<nextCatalog catalog=\"{file}\"/>")
    };
    let catalog = format!(
        "<catalog xmlns=\"urn:oasis:names:tc:entity:xmlns:xml:catalog\">{}</catalog>",
        files.split_whitespace().map(entry).collect::<String>()
    );
    match Catalog::parse(catalog.as_bytes(), Default::default(), "", loader(&o.path)) {
        Ok(catalog) if o.catalogs => Box::new(CatalogResolver::new(catalog, loader(&o.path))),
        _ => loader(&o.path),
    }
}

fn compile(o: &Opts) -> Result<Schemas, i32> {
    let read = |path: &str| fs::read(path).map_err(|_| SCHEMACOMP);
    let failed = |what: &str, path: &str| {
        eprintln!("{what} schema {path} failed to compile");
        SCHEMACOMP
    };
    let relaxng = o.relaxng.as_deref().map(|path| {
        let schema =
            relaxng::Schema::parse(&read(path)?, path, loader(&o.path), &Default::default());
        schema.map_err(|errors| {
            errors
                .0
                .iter()
                .for_each(|e| report(&e.uri, &[], None, "Relax-NG parser error", &e.message));
            failed("Relax-NG", path)
        })
    });
    let xsd = o.schema.as_deref().map(|path| {
        let schema = xsd::Schema::with_resolver(
            &read(path)?,
            path,
            Some(loader(&o.path)),
            Default::default(),
        );
        schema.map_err(|error| {
            error
                .diagnostics
                .iter()
                .for_each(|d| report(&d.uri, &[], None, "Schemas parser error", &d.message));
            failed("WXS", path)
        })
    });
    Ok(Schemas {
        relaxng: relaxng.transpose()?,
        xsd: xsd.transpose()?,
    })
}

/// Prints diagnostics `(offset, message, is_warning)`; true when none is an error.
fn print_all<'a>(
    file: &str,
    input: &[u8],
    kind: &str,
    diagnostics: &mut dyn Iterator<Item = (Option<usize>, &'a str, bool)>,
) -> bool {
    diagnostics.fold(true, |valid, (offset, message, warning)| {
        report(file, input, offset, kind, message);
        valid && warning
    })
}

fn parse(o: &Opts, name: &str, input: &[u8]) -> Result<OwnedDocument, yaxml::Error> {
    let limits = limits(o);
    if o.html {
        let flags = o.html_flags | yaxml::html::COMPACT | yaxml::html::BIG_LINES;
        let options = yaxml::html::Options { flags, limits };
        return Ok(yaxml::html::parse(input, options)?.document);
    }
    if o.load || o.valid || o.postvalid || o.relaxng.is_some() || o.schema.is_some() {
        OwnedDocument::with_resolver(input, limits, name, resolver(o))
    } else {
        OwnedDocument::with_limits(input, limits)
    }
}

fn save(o: &Opts, document: &OwnedDocument, out: &mut Vec<u8>) -> i32 {
    let saved = match o.c14n {
        Some(mode) => {
            let options = yaxml::c14n::Options {
                with_comments: true,
                ..yaxml::c14n::Options::new(mode)
            };
            yaxml::c14n::write(document, &options, &mut *out).map_err(|_| "Failed to canonicalize")
        }
        None if o.html => yaxml::html::save(document, o.format)
            .map(|bytes| out.extend_from_slice(&bytes))
            .map_err(|_| "failed save to -"),
        None => yaxml::Writer::new(&mut *out)
            .write_owned_document(document)
            .map_err(|_| "failed save to -"),
    };
    saved.map_or_else(
        |message| {
            eprintln!("{message}");
            OUT
        },
        |()| 0,
    )
}

fn query(document: &OwnedDocument, expression: &str, o: &Opts, out: &mut Vec<u8>) -> i32 {
    let Ok(compiled) = XPath::compile(expression) else {
        eprintln!("XPath compilation failure");
        return XPATH;
    };
    match compiled.evaluate(document, document.root().id(), &Context::new()) {
        Err(_) => {
            eprintln!("XPath evaluation failure");
            XPATH
        }
        Ok(value) if dump::value(document, &value, o.xpath_sep, out) => 0,
        Ok(_) => {
            if !o.quiet {
                eprintln!("XPath set is empty");
            }
            XPATH_EMPTY
        }
    }
}

/// Runs each check in order; the last failing code wins, as xmllint's `progresult`.
fn last_failure(codes: impl IntoIterator<Item = i32>) -> i32 {
    codes
        .into_iter()
        .filter(|&code| code != 0)
        .last()
        .unwrap_or(0)
}

fn lint(o: &Opts, schemas: &Schemas, file: &str) -> i32 {
    let start = Instant::now();
    let input = match file {
        "-" => io::stdin().lock().bytes().collect::<Result<Vec<_>, _>>(),
        _ => fs::read(file),
    };
    let Ok(input) = input else {
        eprintln!("{file}: failed to load external entity \"{file}\"");
        return RDFILE;
    };
    if o.stream {
        return stream(o, file, &input);
    }
    let mut document = match parse(o, file, &input) {
        Ok(document) => document,
        Err(error) => {
            report(
                file,
                &input,
                Some(error.offset),
                "parser error",
                &format!("{:?}", error.kind),
            );
            let memory = o.maxmem.is_some()
                && matches!(
                    error.kind,
                    ErrorKind::LimitExceeded(Limit::OwnedBytes | Limit::InputBytes)
                );
            return if memory { MEM } else { RDFILE };
        }
    };
    let mut out = Vec::new();
    let included = if o.xinclude {
        let options = yaxml::xinclude::Options {
            base_fixup: !o.nofixup_base,
            limits: limits(o),
            ..Default::default()
        };
        match yaxml::xinclude::process(&document, file, resolver(o), &options) {
            Ok(result) => {
                document = result;
                0
            }
            Err(error) => {
                eprintln!("{file}: {error}");
                UNCLASS
            }
        }
    } else {
        0
    };
    let queried = o
        .xpath
        .as_deref()
        .map_or(0, |expression| query(&document, expression, o, &mut out));
    let saved = if o.noout {
        0
    } else {
        save(o, &document, &mut out)
    };
    let flushed = match &o.output {
        Some(path) => fs::write(path, &out).map_err(|_| ()),
        None => io::stdout().lock().write_all(&out).map_err(|_| ()),
    };
    let check = |kind: &str, found: &mut dyn Iterator<Item = (Option<usize>, &str, bool)>| {
        print_all(file, &input, kind, found)
    };
    let dtd_valid = if o.valid || o.postvalid {
        let found = document.validate();
        let ok = check(
            "validity error",
            &mut found
                .iter()
                .map(|d| (d.offset, d.message.as_str(), d.code.is_warning())),
        );
        if !ok && o.postvalid {
            eprintln!("Document {file} does not validate");
        }
        if ok { 0 } else { VALID }
    } else {
        0
    };
    let dtdvalid = o.dtdvalid.as_deref().map_or(0, |path| {
        let Ok(dtd) = valid::Dtd::load(path, Default::default(), loader(&o.path)) else {
            eprintln!("Could not parse DTD {path}");
            return DTD;
        };
        let found = valid::validate(&document, &dtd);
        if check(
            "validity error",
            &mut found
                .iter()
                .map(|d| (d.offset, d.message.as_str(), d.code.is_warning())),
        ) {
            return 0;
        }
        eprintln!("Document {file} does not validate against {path}");
        VALID
    });
    let verdict = |ok: bool| {
        match (ok, o.quiet) {
            (true, false) => eprintln!("{file} validates"),
            (false, _) => eprintln!("{file} fails to validate"),
            _ => {}
        }
        if ok { 0 } else { VALID }
    };
    let relaxng = schemas.relaxng.as_ref().map_or(0, |schema| {
        let found = schema.validate(&document);
        verdict(check(
            "Relax-NG validity error",
            &mut found.iter().map(|d| (d.offset, d.message.as_str(), false)),
        ))
    });
    let xsd = schemas.xsd.as_ref().map_or(0, |schema| {
        let found = schema.validate(&document);
        verdict(check(
            "Schemas validity error",
            &mut found
                .iter()
                .map(|d| (d.offset, d.message.as_str(), d.code.is_warning())),
        ))
    });
    if o.timing {
        eprintln!("Total took {} ms", start.elapsed().as_millis());
    }
    let written = if flushed.is_err() { OUT } else { 0 };
    last_failure([
        included, queried, saved, written, dtd_valid, dtdvalid, relaxng, xsd,
    ])
}

/// `--stream`: read every node through the pull interface and print nothing.
fn stream(o: &Opts, file: &str, input: &[u8]) -> i32 {
    let mut reader =
        yaxml::TextReader::with_limits(input, limits(o)).with_resolver(file, resolver(o));
    loop {
        match reader.read() {
            Ok(true) => {}
            Ok(false) => return 0,
            Err(error) => {
                eprintln!("{file} : failed to parse: {error:?}");
                return UNCLASS;
            }
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let code = match opts::parse(&args).and_then(|o| {
        if o.version {
            println!(
                "yaxmllint using yaxml {} (libxml2 2.15.4 compatible)",
                env!("CARGO_PKG_VERSION")
            );
        }
        compile(&o).map(|schemas| last_failure(o.files.iter().map(|file| lint(&o, &schemas, file))))
    }) {
        Ok(code) | Err(code) => code,
    };
    std::process::exit(code);
}
