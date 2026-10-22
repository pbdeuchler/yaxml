//! The command line: a table of option names, whether they take a value, and
//! how they update the options.

use yaxml::c14n::Mode;
use yaxml::options;

pub const UNCLASS: i32 = 1;
pub const DTD: i32 = 2;
pub const VALID: i32 = 3;
pub const RDFILE: i32 = 4;
pub const SCHEMACOMP: i32 = 5;
pub const OUT: i32 = 6;
pub const MEM: i32 = 9;
pub const XPATH: i32 = 10;
pub const XPATH_EMPTY: i32 = 11;

#[derive(Default)]
pub struct Opts {
    pub noout: bool,
    /// `--debug`: `xmlDebugDumpDocument` instead of the saved document.
    pub debug: bool,
    pub quiet: bool,
    pub timing: bool,
    pub version: bool,
    pub html: bool,
    /// `--noblanks`; `--format` implies it.
    pub noblanks: bool,
    pub stream: bool,
    /// `xmlsave` layout: 0 as is, 1 `--format`, 2 whitespace inside tags.
    pub format: u8,
    pub xmlout: bool,
    pub valid: bool,
    pub postvalid: bool,
    pub xinclude: bool,
    pub nofixup_base: bool,
    /// Parser options set directly: `--loaddtd`, `--dtdattr`, `--noent`,
    /// `--recover`, `--nocdata`, `--nsclean`, `--noenc`.
    pub flags: u32,
    pub catalogs: bool,
    pub huge: bool,
    pub html_flags: u32,
    pub c14n: Option<Mode>,
    pub xpath: Option<String>,
    pub xpath_sep: &'static str,
    pub dtdvalid: Option<String>,
    pub relaxng: Option<String>,
    pub schema: Option<String>,
    pub schematron: Option<String>,
    pub output: Option<String>,
    pub encode: Option<String>,
    pub path: Vec<String>,
    pub maxmem: Option<usize>,
    pub max_ampl: Option<usize>,
    pub files: Vec<String>,
}

impl Opts {
    fn with(self, flag: u32) -> Self {
        Self {
            flags: self.flags | flag,
            ..self
        }
    }
}

type Set = fn(Opts, &str) -> Opts;

/// An accepted option; `arg` options consume the next argument.
struct Spec(&'static str, bool, Set);

const fn flag(name: &'static str, set: Set) -> Spec {
    Spec(name, false, set)
}
const fn arg(name: &'static str, set: Set) -> Spec {
    Spec(name, true, set)
}

fn number(value: &str) -> Option<usize> {
    value.parse().ok()
}

const TABLE: &[Spec] = &[
    flag("noout", |o, _| Opts { noout: true, ..o }),
    flag("debug", |o, _| Opts { debug: true, ..o }),
    flag("quiet", |o, _| Opts { quiet: true, ..o }),
    flag("timing", |o, _| Opts { timing: true, ..o }),
    flag("version", |o, _| Opts { version: true, ..o }),
    flag("html", |o, _| Opts { html: true, ..o }),
    flag("stream", |o, _| Opts { stream: true, ..o }),
    flag("format", |o, _| Opts { format: 1, ..o }),
    flag("xmlout", |o, _| Opts { xmlout: true, ..o }),
    arg("pretty", |o, v| Opts {
        format: v.parse().unwrap_or(0),
        ..o
    }),
    flag("valid", |o, _| Opts { valid: true, ..o }),
    flag("postvalid", |o, _| Opts {
        postvalid: true,
        ..o
    }),
    flag("xinclude", |o, _| Opts {
        xinclude: true,
        ..o
    }),
    flag("noxincludenode", |o, _| Opts {
        xinclude: true,
        ..o
    }),
    flag("nofixup-base-uris", |o, _| Opts {
        nofixup_base: true,
        ..o
    }),
    flag("loaddtd", |o, _| o.with(options::DTDLOAD)),
    flag("dtdattr", |o, _| o.with(options::DTDATTR)),
    flag("noent", |o, _| o.with(options::NOENT)),
    flag("oldxml10", |o, _| o.with(options::OLD10)),
    flag("recover", |o, _| o.with(options::RECOVER)),
    flag("nocdata", |o, _| o.with(options::NOCDATA)),
    flag("nsclean", |o, _| o.with(options::NSCLEAN)),
    flag("noenc", |o, _| o.with(options::IGNORE_ENC)),
    flag("catalogs", |o, _| Opts {
        catalogs: true,
        ..o
    }),
    flag("nocatalogs", |o, _| Opts {
        catalogs: false,
        ..o
    }),
    flag("huge", |o, _| Opts { huge: true, ..o }),
    flag("nodefdtd", |o, _| Opts {
        html_flags: o.html_flags | yaxml::html::NODEFDTD,
        ..o
    }),
    flag("noblanks", |o, _| Opts {
        noblanks: true,
        ..o
    }),
    flag("c14n", |o, _| Opts {
        c14n: Some(Mode::C14n10),
        ..o
    }),
    flag("c14n11", |o, _| Opts {
        c14n: Some(Mode::C14n11),
        ..o
    }),
    flag("exc-c14n", |o, _| Opts {
        c14n: Some(Mode::Exclusive10),
        ..o
    }),
    arg("xpath", |o, v| Opts {
        xpath: Some(v.into()),
        xpath_sep: "\n",
        noout: true,
        ..o
    }),
    arg("xpath0", |o, v| Opts {
        xpath: Some(v.into()),
        xpath_sep: "\0",
        noout: true,
        ..o
    }),
    arg("dtdvalid", |o, v| Opts {
        dtdvalid: Some(v.into()),
        ..o
    }),
    arg("relaxng", |o, v| Opts {
        relaxng: Some(v.into()),
        ..o
    }),
    arg("schematron", |o, v| Opts {
        schematron: Some(v.into()),
        ..o
    }),
    arg("schema", |o, v| Opts {
        schema: Some(v.into()),
        ..o
    }),
    arg("output", |o, v| Opts {
        output: Some(v.into()),
        ..o
    }),
    arg("o", |o, v| Opts {
        output: Some(v.into()),
        ..o
    }),
    arg("encode", |o, v| Opts {
        encode: Some(v.into()),
        ..o
    }),
    arg("path", |o, v| Opts {
        path: v.split([' ', ':']).map(String::from).collect(),
        ..o
    }),
    arg("maxmem", |o, v| Opts {
        maxmem: number(v),
        ..o
    }),
    arg("max-ampl", |o, v| Opts {
        max_ampl: number(v),
        ..o
    }),
];

/// Accepted and ignored: yaxml never touches the network, builds one kind of
/// tree, and has no other parser modes to select.
const NO_OPS: &[&str] = &[
    "nonet",
    "nowarning",
    "pedantic",
    "nodict",
    "nocompact",
    "push",
    "memory",
    "repeat",
    "strict-namespace",
    "load-trace",
];

/// xmllint options with no yaxml counterpart. The value-taking ones come last.
const UNSUPPORTED: &[&str] = &[
    "debug",
    "shell",
    "copy",
    "sax",
    "sax1",
    "insert",
    "walker",
    "dropdtd",
    "auto",
    "compress",
    "pattern",
    "dtdvalidfpi",
];

fn usage() -> String {
    let names: Vec<_> = TABLE
        .iter()
        .filter(|spec| spec.0.len() > 1)
        .map(|spec| format!("--{}", spec.0))
        .collect();
    format!(
        "Usage: yaxmllint [options] XMLfiles ...\n\t{}\n",
        names.join(" ")
    )
}

/// Parses `args` (without the program name), or returns the exit code after
/// reporting why not.
pub fn parse(args: &[String]) -> Result<Opts, i32> {
    let fail = |message: String| {
        eprint!("{message}");
        Err(UNCLASS)
    };
    let mut opts = Opts::default();
    let mut rest = args.iter();
    while let Some(argument) = rest.next() {
        let name = argument.trim_start_matches('-');
        if !argument.starts_with('-') || name.is_empty() {
            opts.files.push(argument.clone());
        } else if let Some(Spec(_, takes, set)) = TABLE.iter().find(|spec| spec.0 == name) {
            let value = if *takes { rest.next() } else { Some(argument) };
            let Some(value) = value else {
                return fail(format!("{name}: missing value\n"));
            };
            if matches!(name, "maxmem" | "max-ampl") && number(value).is_none() {
                return fail(format!("{name}: invalid integer: {value}\n"));
            }
            opts = set(opts, value);
        } else if UNSUPPORTED.contains(&name) {
            return fail(format!("yaxmllint: unsupported option {argument}\n"));
        } else if NO_OPS.contains(&name) {
            continue;
        } else {
            return fail(format!("Unknown option {argument}\n{}", usage()));
        }
    }
    match opts.files.is_empty() && !opts.version {
        true => fail(usage()),
        false => Ok(opts),
    }
}
