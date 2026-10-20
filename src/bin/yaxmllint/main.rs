//! `yaxmllint`: xmllint's command line over yaxml. See `docs/yaxmllint.md`.

mod diag;
mod dump;
mod opts;

use std::io::{self, Read, Write};
use std::path::Path;
use std::time::Instant;
use std::{env, fs};

use diag::{report, report_with};
use opts::{Opts, *};
use yaxml::catalog::{Catalog, CatalogResolver};
use yaxml::xpath::{Context, XPath};
use yaxml::{
    ErrorKind, Limit, Limits, OwnedDocument, ParseOptions, Request, Resolver, c14n, relaxng, save,
    schematron, valid, xsd,
};

/// Compiled `--schematron`, `--relaxng` and `--schema` documents, shared by every input.
struct Schemas {
    schematron: Option<schematron::Schema>,
    relaxng: Option<relaxng::Schema>,
    xsd: Option<xsd::Schema>,
}

fn limits(o: &Opts) -> Limits {
    let base = Limits::default();
    let huge = |value: usize| if o.huge { value << 6 } else { value };
    Limits {
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
    let failed = |what: &str, path: &str| {
        eprintln!("{what} schema {path} failed to compile");
        SCHEMACOMP
    };
    let schematron = o.schematron.as_deref().map(|path| {
        let bytes = fs::read(path).unwrap_or_default();
        let schema = schematron::Schema::parse(&bytes, path, loader(&o.path), &Default::default());
        schema.map_err(|errors| {
            eprintln!("{errors}");
            failed("Schematron", path)
        })
    });
    let relaxng = o.relaxng.as_deref().map(|path| {
        let bytes = fs::read(path).map_err(|_| failed("Relax-NG", path))?;
        let schema = relaxng::Schema::parse(&bytes, path, loader(&o.path), &Default::default());
        schema.map_err(|errors| {
            errors
                .0
                .iter()
                .for_each(|e| report(&e.uri, &[], None, "Relax-NG parser error", &e.message));
            failed("Relax-NG", path)
        })
    });
    let xsd = o.schema.as_deref().map(|path| {
        let bytes = fs::read(path).map_err(|_| failed("WXS", path))?;
        let schema =
            xsd::Schema::with_resolver(&bytes, path, Some(loader(&o.path)), Default::default());
        schema.map_err(|error| {
            error
                .diagnostics
                .iter()
                .for_each(|d| report(&d.uri, &[], None, "Schemas parser error", &d.message));
            failed("WXS", path)
        })
    });
    Ok(Schemas {
        schematron: schematron.transpose()?,
        relaxng: relaxng.transpose()?,
        xsd: xsd.transpose()?,
    })
}

/// A validation diagnostic of any of the validators: offset, message, warning?
trait Finding {
    fn parts(&self) -> (Option<usize>, &str, bool);
}

macro_rules! finding {
    ($($module:ident),*) => {$(
        impl<Id> Finding for $module::Diagnostic<Id> {
            fn parts(&self) -> (Option<usize>, &str, bool) {
                (self.offset, &self.message, self.code.is_warning())
            }
        }
    )*};
}
finding!(valid, xsd);

impl<Id> Finding for relaxng::Diagnostic<Id> {
    fn parts(&self) -> (Option<usize>, &str, bool) {
        (self.offset, &self.message, false)
    }
}

/// Prints the diagnostics, with source context when `context`; true when none is an error.
fn print_found(
    context: bool,
    file: &str,
    input: &[u8],
    kind: &str,
    found: &[impl Finding],
) -> bool {
    found.iter().fold(true, |ok, finding| {
        let (offset, message, warning) = finding.parts();
        report_with(context, file, input, offset, kind, message);
        ok && warning
    })
}

fn print_all(file: &str, input: &[u8], kind: &str, found: &[impl Finding]) -> bool {
    print_found(true, file, input, kind, found)
}

/// xmllint's parser options for the command line.
fn flags(o: &Opts) -> u32 {
    use yaxml::options::*;
    let set = |on: bool, flag: u32| if on { flag } else { 0 };
    o.flags
        | set(o.huge, HUGE)
        | set(o.noblanks || o.format == 1, NOBLANKS)
        | set(o.valid, DTDVALID)
        | set(
            o.postvalid || o.dtdvalid.is_some() || o.schematron.is_some(),
            DTDLOAD,
        )
        | set(
            o.schematron.is_some() || o.relaxng.is_some() || o.schema.is_some(),
            NOENT | DTDLOAD,
        )
        | set(o.c14n.is_some(), NOENT | DTDATTR | DTDLOAD)
}

fn parse(o: &Opts, name: &str, input: &[u8]) -> Result<OwnedDocument, yaxml::Error> {
    let limits = limits(o);
    if o.html {
        let noblanks = o.noblanks || o.format == 1;
        let flags = o.html_flags | yaxml::html::COMPACT | yaxml::html::BIG_LINES;
        let flags = flags | if noblanks { yaxml::html::NOBLANKS } else { 0 };
        return Ok(yaxml::html::parse(input, yaxml::html::Options { flags, limits })?.document);
    }
    let options = ParseOptions::new(flags(o)).with_limits(limits);
    OwnedDocument::parse_with(input, options.with_resolver(name, resolver(o)))
}

fn save(o: &Opts, document: &OwnedDocument, out: &mut Vec<u8>) -> i32 {
    let indent = env::var("XMLLINT_INDENT").unwrap_or("  ".into());
    let flags = match o.format {
        1 => save::FORMAT,
        2 => save::WSNONSIG,
        _ => 0,
    } | if o.xmlout { save::AS_XML } else { 0 };
    let options = save::Options {
        flags,
        encoding: o.encode.as_deref(),
        indent: &indent,
    };
    let saved = match o.c14n {
        Some(mode) => {
            let options = c14n::Options {
                with_comments: true,
                ..c14n::Options::new(mode)
            };
            c14n::write(document, &options, &mut *out).map_err(|_| "Failed to canonicalize")
        }
        None => save::to_vec(document, &options)
            .map(|bytes| out.extend_from_slice(&bytes))
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

/// Validation after parsing: DTD (`--valid`, `--postvalid`, `--dtdvalid`),
/// then Schematron, RELAX NG and XML Schema; the last failing code wins.
/// Prints a schema verdict line; the exit code.
fn verdict(o: &Opts, file: &str, ok: bool) -> i32 {
    match (ok, o.quiet) {
        (true, false) => eprintln!("{file} validates"),
        (false, _) => eprintln!("{file} fails to validate"),
        _ => {}
    }
    if ok { 0 } else { VALID }
}

fn checks(o: &Opts, schemas: &Schemas, file: &str, input: &[u8], document: &OwnedDocument) -> i32 {
    let verdict = |ok| verdict(o, file, ok);
    let dtd_valid = (o.valid || o.postvalid).then(|| {
        let ok = print_all(file, input, "validity error", &document.validate());
        if !ok && o.postvalid {
            eprintln!("Document {file} does not validate");
        }
        // Parse-time `--valid` reports a missing DTD but does not fail on it.
        if ok || (!o.postvalid && document.dtd().is_none()) {
            0
        } else {
            VALID
        }
    });
    let dtdvalid = o.dtdvalid.as_deref().map(|path| {
        let Ok(dtd) = valid::Dtd::load(path, Default::default(), loader(&o.path)) else {
            eprintln!("Could not parse DTD {path}");
            return DTD;
        };
        if print_all(
            file,
            input,
            "validity error",
            &valid::validate(document, &dtd),
        ) {
            return 0;
        }
        eprintln!("Document {file} does not validate against {path}");
        VALID
    });
    let schematron = schemas.schematron.as_ref().map(|schema| {
        let quiet = if o.noout { schematron::OUT_QUIET } else { 0 };
        let validation = schema.validate(document, schematron::OUT_TEXT | quiet, input);
        eprint!("{}", validation.render(file));
        match validation.errors {
            Some(errors) => verdict(errors == 0),
            None => {
                eprintln!("{file} validation generated an internal error");
                VALID
            }
        }
    });
    let relaxng = schemas.relaxng.as_ref().map(|schema| {
        verdict(print_all(
            file,
            input,
            "Relax-NG validity error",
            &schema.validate(document),
        ))
    });
    let xsd = schemas.xsd.as_ref().map(|schema| {
        verdict(print_all(
            file,
            input,
            "Schemas validity error",
            &schema.validate(document),
        ))
    });
    last_failure(
        [dtd_valid, dtdvalid, schematron, relaxng, xsd]
            .into_iter()
            .flatten(),
    )
}

fn lint(o: &Opts, schemas: &Schemas, file: &str) -> i32 {
    let start = Instant::now();
    let input = match file {
        "-" => {
            let mut bytes = Vec::new();
            io::stdin().lock().read_to_end(&mut bytes).map(|_| bytes)
        }
        _ => fs::read(file),
    };
    let Ok(input) = input else {
        eprintln!("{file}: failed to load external entity \"{file}\"");
        return RDFILE;
    };
    if o.stream {
        return stream(o, schemas, file, &input);
    }
    let mut document = match parse(o, file, &input) {
        Ok(document) => {
            // `--recover` reports what it recovered from and keeps the tree.
            for error in document.errors() {
                let message = format!("{:?}", error.kind);
                report(file, &input, Some(error.offset), "parser error", &message);
            }
            document
        }
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
            parse_flags: flags(o) & !yaxml::options::XINCLUDE,
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
    let checked = checks(o, schemas, file, &input, &document);
    if o.timing {
        eprintln!("Total took {} ms", start.elapsed().as_millis());
    }
    let written = if flushed.is_err() { OUT } else { 0 };
    last_failure([included, queried, saved, written, checked])
}

/// `--stream`: read every node through the pull interface, printing nothing
/// but `--valid`, `--relaxng` and `--schema` validation, which the reader does
/// as it goes.
fn stream(o: &Opts, schemas: &Schemas, file: &str, input: &[u8]) -> i32 {
    // The reader does not recover; it prints nothing, so blanks do not matter.
    use yaxml::options::{NOBLANKS, RECOVER};
    let streamable = flags(o) & !(NOBLANKS | RECOVER);
    let options = ParseOptions::new(streamable).with_limits(limits(o));
    let reader = yaxml::TextReader::with_options(input, options.with_resolver(file, resolver(o)));
    let mut reader = match reader {
        Ok(reader) => reader,
        Err(error) => {
            eprintln!("{file} : failed to parse: {error:?}");
            return UNCLASS;
        }
    };
    let (rng, xsd) = (schemas.relaxng.as_ref(), schemas.xsd.as_ref());
    reader
        .relaxng_set_schema(rng)
        .and_then(|()| reader.xsd_set_schema(xsd))
        .expect("before the first read");
    let read = loop {
        match reader.read() {
            Ok(true) => {}
            done => break done,
        }
    };
    let dtd = reader.dtd_diagnostics();
    let dtd_valid = print_all(file, input, "validity error", dtd);
    let dtd_code = (o.valid && !dtd_valid).then(|| {
        eprintln!("Document {file} does not validate");
        VALID
    });
    print_found(
        false,
        file,
        input,
        "Relax-NG validity error",
        reader.relaxng_diagnostics(),
    );
    print_found(
        false,
        file,
        input,
        "Schemas validity error",
        reader.xsd_diagnostics(),
    );
    // The schema verdict, as libxml2's reader reports the last validation set.
    let schemas_valid =
        reader.relaxng_diagnostics().is_empty() && yaxml::xsd::is_valid(reader.xsd_diagnostics());
    let code = (rng.is_some() || xsd.is_some()).then(|| verdict(o, file, schemas_valid));
    match read {
        Err(error) => {
            eprintln!("{file} : failed to parse: {error:?}");
            UNCLASS
        }
        Ok(_) => last_failure([dtd_code, code].into_iter().flatten()),
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
