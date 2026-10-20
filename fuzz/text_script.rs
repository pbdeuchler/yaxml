//! `TextWriter` call scripts, shared by the `writer` fuzz target and the
//! libxml2 differential (tests/text_writer_oracle.rs). A call is an
//! operation code of `tooling/libxml2_textwriter.c` and its arguments.
// Each user takes the parts it needs.
#![allow(dead_code)]

use std::fmt::Write as _;
use std::io::Write;

use yaxml::{TextWriter, WriteError};

/// Argument kinds, each drawn from its own pool.
#[derive(Clone, Copy)]
pub enum Kind {
    Name,
    Prefix,
    Uri,
    Text,
    OptText,
    Target,
    Id,
    Flag,
    Bytes,
    Version,
    Encoding,
    Standalone,
    Indent,
    Quote,
}

use Kind::*;

/// Every operation with its argument kinds, in the helper's order.
pub const OPERATIONS: &[(&str, &[Kind])] = &[
    ("SD", &[Version, Encoding, Standalone]),
    ("ED", &[]),
    ("SE", &[Name]),
    ("SN", &[Prefix, Name, Uri]),
    ("EE", &[]),
    ("FE", &[]),
    ("SA", &[Name]),
    ("SB", &[Prefix, Name, Uri]),
    ("EA", &[]),
    ("WA", &[Name, Text]),
    ("WB", &[Prefix, Name, Uri, Text]),
    ("WE", &[Name, OptText]),
    ("WF", &[Prefix, Name, Uri, Text]),
    ("WS", &[Text]),
    ("WR", &[Text]),
    ("W6", &[Bytes]),
    ("WH", &[Bytes]),
    ("SC", &[]),
    ("EC", &[]),
    ("WC", &[Text]),
    ("SP", &[Target]),
    ("EP", &[]),
    ("WP", &[Target, OptText]),
    ("SX", &[]),
    ("EX", &[]),
    ("WX", &[OptText]),
    ("ST", &[Name, Id, Id]),
    ("ET", &[]),
    ("WT", &[Name, Id, Id, OptText]),
    ("SL", &[Name]),
    ("EL", &[]),
    ("WL", &[Name, Text]),
    ("SK", &[Name]),
    ("EK", &[]),
    ("WK", &[Name, Text]),
    ("SY", &[Flag, Name]),
    ("EY", &[]),
    ("WI", &[Flag, Name, Text]),
    ("WJ", &[Flag, Name, Id, Id, Id]),
    ("WG", &[Id, Id, Id]),
    ("WY", &[Flag, Name, Id, Id, Id, OptText]),
    ("WN", &[Name, Id, Id]),
    ("IN", &[Flag]),
    ("IS", &[Indent]),
    ("QC", &[Quote]),
    ("FL", &[]),
];

fn pool(kind: Kind) -> &'static [Option<&'static str>] {
    match kind {
        Name => &[Some("a"), Some("b"), Some("p:c"), Some("\u{e9}"), Some("")],
        Prefix => &[None, Some("p"), Some("q"), None],
        Uri => &[None, Some("urn:p"), Some("urn:q"), Some("a\"&<\u{e9}")],
        Text | OptText => &[
            Some("t"),
            Some("x<y&z>w"),
            Some("\"q' \t\n\r"),
            Some("\u{e9}\u{20ac}\u{1f600}"),
            Some("]]>--?>"),
            Some(""),
            Some("\u{1}\u{1f}"),
            Some("<a/>"),
            None,
        ],
        Target => &[Some("pi"), Some("xml"), Some("XmL"), Some("t"), Some("")],
        Id => &[None, Some("pub"), Some("s.dtd"), Some("a\"b"), None],
        Flag => &[Some("0"), Some("1")],
        Bytes => &[
            Some(""),
            Some("a"),
            Some("ab"),
            Some("abc"),
            Some("\u{ff}\0x"),
        ],
        Version => &[None, Some("1.0"), Some("1.1")],
        Encoding => &[
            None,
            Some("UTF-8"),
            Some("utf8"),
            Some("ISO-8859-1"),
            Some("latin1"),
            Some("UTF-16"),
            Some("UTF-16BE"),
            Some("US-ASCII"),
            Some("windows-1252"),
            Some("x-bogus"),
        ],
        Standalone => &[None, Some("yes"), Some("no")],
        Indent => &[Some(" "), Some("  "), Some("\t"), Some(""), Some("ab")],
        Quote => &[Some("\""), Some("'"), Some("x")],
    }
}

/// One call: an index into [`OPERATIONS`] and its arguments.
#[derive(Clone, Debug)]
pub struct Call {
    pub operation: usize,
    pub args: Vec<Option<String>>,
}

impl Call {
    /// Draws the operation and each argument from `next(bound)`. Text
    /// arguments come from `text` when it yields one.
    pub fn draw(
        operation: usize,
        next: &mut impl FnMut(usize) -> usize,
        text: &mut impl FnMut() -> Option<String>,
    ) -> Self {
        let (_, kinds) = OPERATIONS[operation];
        let args = kinds
            .iter()
            .map(|&kind| {
                let options = pool(kind);
                let pick = options[next(options.len())].map(str::to_owned);
                match kind {
                    // Text is never NULL; optional text may be.
                    Text => text().or(pick).or(Some(String::new())),
                    OptText => pick.and_then(|pick| text().or(Some(pick))),
                    _ => pick,
                }
            })
            .collect();
        Self { operation, args }
    }

    pub fn code(&self) -> &'static str {
        OPERATIONS[self.operation].0
    }

    /// The helper's spelling: `OP` or `OP:arg,...` with hex, `.` or `-`.
    pub fn encode(&self, line: &mut String) {
        line.push_str(self.code());
        for (index, arg) in self.args.iter().enumerate() {
            line.push(if index == 0 { ':' } else { ',' });
            match arg {
                None => line.push('-'),
                Some(arg) if arg.is_empty() => line.push('.'),
                Some(arg) => arg.bytes().for_each(|b| write!(line, "{b:02x}").unwrap()),
            }
        }
    }

    /// Replays the call; `Err` where libxml2 returns -1.
    pub fn apply<W: Write>(&self, writer: &mut TextWriter<W>) -> Result<(), WriteError> {
        let o = |i: usize| self.args[i].as_deref();
        let s = |i: usize| o(i).unwrap_or_default();
        let flag = |i: usize| s(i) == "1";
        match self.code() {
            "SD" => writer.start_document(o(0), o(1), o(2)),
            "ED" => writer.end_document(),
            "SE" => writer.start_element(s(0)),
            "SN" => writer.start_element_ns(o(0), s(1), o(2)),
            "EE" => writer.end_element(),
            "FE" => writer.full_end_element(),
            "SA" => writer.start_attribute(s(0)),
            "SB" => writer.start_attribute_ns(o(0), s(1), o(2)),
            "EA" => writer.end_attribute(),
            "WA" => writer.write_attribute(s(0), s(1)),
            "WB" => writer.write_attribute_ns(o(0), s(1), o(2), s(3)),
            "WE" => writer.write_element(s(0), o(1)),
            "WF" => writer.write_element_ns(o(0), s(1), o(2), s(3)),
            "WS" => writer.write_string(s(0)),
            "WR" => writer.write_raw(s(0)),
            "W6" => writer.write_base64(s(0).as_bytes()),
            "WH" => writer.write_bin_hex(s(0).as_bytes()),
            "SC" => writer.start_comment(),
            "EC" => writer.end_comment(),
            "WC" => writer.write_comment(s(0)),
            "SP" => writer.start_pi(s(0)),
            "EP" => writer.end_pi(),
            "WP" => writer.write_pi(s(0), o(1)),
            "SX" => writer.start_cdata(),
            "EX" => writer.end_cdata(),
            "WX" => writer.write_cdata(o(0)),
            "ST" => writer.start_dtd(s(0), o(1), o(2)),
            "ET" => writer.end_dtd(),
            "WT" => writer.write_dtd(s(0), o(1), o(2), o(3)),
            "SL" => writer.start_dtd_element(s(0)),
            "EL" => writer.end_dtd_element(),
            "WL" => writer.write_dtd_element(s(0), s(1)),
            "SK" => writer.start_dtd_attlist(s(0)),
            "EK" => writer.end_dtd_attlist(),
            "WK" => writer.write_dtd_attlist(s(0), s(1)),
            "SY" => writer.start_dtd_entity(flag(0), s(1)),
            "EY" => writer.end_dtd_entity(),
            "WI" => writer.write_dtd_internal_entity(flag(0), s(1), s(2)),
            "WJ" => writer.write_dtd_external_entity(flag(0), s(1), o(2), o(3), o(4)),
            "WG" => writer.write_dtd_external_entity_contents(o(0), o(1), o(2)),
            "WY" => writer.write_dtd_entity(flag(0), s(1), o(2), o(3), o(4), o(5)),
            "WN" => writer.write_dtd_notation(s(0), o(1), o(2)),
            "IN" => {
                writer.set_indent(flag(0));
                Ok(())
            }
            "IS" => {
                writer.set_indent_string(s(0));
                Ok(())
            }
            "QC" => writer.set_quote_char(s(0).chars().next().unwrap_or('\0')),
            "FL" => writer.flush(),
            code => unreachable!("{code}"),
        }
    }
}

/// A random script: misuse included, `SD` only first (an encoding switched
/// mid-stream converts libxml2's unflushed bytes too).
pub fn random(
    next: &mut impl FnMut(usize) -> usize,
    text: &mut impl FnMut() -> Option<String>,
    calls: usize,
) -> Vec<Call> {
    // Structure-building calls are drawn more often.
    const FAVORED: [&str; 10] = ["SE", "SN", "EE", "WA", "WB", "WS", "WE", "FE", "SA", "EA"];
    let mut script = Vec::new();
    if next(2) == 0 {
        script.push(Call::draw(0, next, &mut || None));
    }
    while script.len() < calls {
        let operation = if next(2) == 0 {
            let code = FAVORED[next(FAVORED.len())];
            OPERATIONS.iter().position(|(c, _)| *c == code).unwrap()
        } else {
            1 + next(OPERATIONS.len() - 1)
        };
        script.push(Call::draw(operation, next, text));
    }
    script
}

fn call(code: &str, args: &[Option<&str>]) -> Call {
    Call {
        operation: OPERATIONS.iter().position(|(c, _)| *c == code).unwrap(),
        args: args.iter().map(|arg| arg.map(str::to_owned)).collect(),
    }
}

/// A script whose output is a namespace-well-formed document: a balanced
/// element tree with distinct attributes, declared prefixes and comment,
/// PI and CDATA content that cannot close them early.
pub fn well_formed(next: &mut impl FnMut(usize) -> usize, calls: usize) -> Vec<Call> {
    const NAMES: [&str; 4] = ["a", "b", "c", "\u{e9}"];
    const TEXT: [&str; 6] = [
        "t",
        "x<y&z>w",
        "\"q' \t\n\r",
        "\u{e9}\u{20ac}\u{1f600}",
        "",
        "]]",
    ];
    let mut script = Vec::new();
    if next(2) == 0 {
        let encoding = [None, Some("UTF-8"), Some("ISO-8859-1"), Some("UTF-16")][next(4)];
        script.push(call("SD", &[None, encoding, None]));
    }
    if next(2) == 0 {
        script.push(call("IN", &[Some("1")]));
    }
    if next(3) == 0 {
        script.push(call("WT", &[Some("a"), None, Some("s.dtd"), None]));
    }
    // Open elements, each with the attribute names it has used.
    let mut open: Vec<Vec<&str>> = Vec::new();
    let mut started = false;
    let mut steps = 0;
    while steps < calls || !open.is_empty() {
        steps += 1;
        let in_tag = open.last().is_some_and(|_| next(2) == 0);
        let text = TEXT[next(TEXT.len())];
        let choice = if steps > calls { 99 } else { next(10) };
        match choice {
            0..=2 if !(started && open.is_empty()) => {
                let name = NAMES[next(NAMES.len())];
                match next(3) {
                    0 => script.push(call("SN", &[Some("p"), Some(name), Some("urn:p")])),
                    _ => script.push(call("SE", &[Some(name)])),
                }
                open.push(Vec::new());
                started = true;
            }
            3 | 4 if in_tag => {
                let names = open.last_mut().unwrap();
                let name = NAMES[next(NAMES.len())];
                if !names.contains(&name) {
                    names.push(name);
                    script.push(call("WA", &[Some(name), Some(text)]));
                }
            }
            5 if !open.is_empty() => script.push(call("WS", &[Some(text)])),
            6 => script.push(call("WC", &[Some(&text.replace('-', ""))])),
            7 => script.push(call("WP", &[Some("pi"), Some(&text.replace('?', ""))])),
            8 if !open.is_empty() => {
                script.push(call("WX", &[Some(&text.replace(']', ""))]));
            }
            _ if !open.is_empty() => {
                open.pop();
                script.push(call(["EE", "FE"][next(2)], &[]));
            }
            _ => {}
        }
    }
    if !started {
        script.push(call("WE", &[Some("r"), Some("t")]));
    }
    script.push(call("ED", &[]));
    script
}
