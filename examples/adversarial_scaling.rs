#![forbid(unsafe_code)]
//! Adversarial size series: `adversarial_scaling [generator|all] [mode] [MiB...]`.
//! Parses each generated input with every ceiling raised (so the cost of the
//! construct, not the limit, is measured) and prints time and log-log slopes.
//! Modes: dom, owned, reader. Build with --release; see docs/security.md.

use std::fmt::Write;
use std::time::Instant;
use yaxml::{Document, Limits, OwnedDocument, Reader};

type Fill = (&'static str, fn(usize) -> String);

const FILLS: &[Fill] = &[
    ("flat", flat),
    ("attrs", attrs),
    ("attrs8", attrs8),
    ("nsdecls", nsdecls),
    ("deep_prefixes", deep_prefixes),
    ("nsattrs", nsattrs),
    ("long_names", long_names),
    ("ids", ids),
    ("tiny_text", tiny_text),
    ("entity_refs", entity_refs),
    ("deep", deep),
    ("comments", comments),
    ("many_entities", many_entities),
    ("default_attrs", default_attrs),
    ("many_elements_decl", many_elements_decl),
    ("rebind", rebind),
    ("attrs_dense", attrs_dense),
    ("attr_refs", attr_refs),
    ("text_refs", text_refs),
    ("cdata", cdata),
    ("ws_attr", ws_attr),
    ("content_model", content_model),
    ("attlist_many", attlist_many),
    ("pe_chain", pe_chain),
    ("idrefs", idrefs),
    ("blanks", blanks),
    ("nsclean", nsclean),
    ("dup_decl", dup_decl),
    ("mixed_valid", mixed_valid),
    ("req_attrs", req_attrs),
    ("enum_attr", enum_attr),
    ("seq_model", seq_model),
    ("html_p", |n| {
        fill(n, "", "", |_| "<p>x<li>y<td>z<b>w".into())
    }),
    ("html_end", |n| {
        fill(n, "<div>", "", |_| "<b></z></b>".into())
    }),
    ("html_deep", |n| {
        fill(n, "", "", |_| "<div><span><b><i>".into())
    }),
    ("html_attrs", |n| fill(n, "<a", ">", |i| format!(" a{i}=v"))),
    ("ent_fanout", ent_fanout),
];

fn fill(bytes: usize, head: &str, tail: &str, unit: impl Fn(usize) -> String) -> String {
    let mut s = String::from(head);
    let mut i = 0;
    while s.len() < bytes {
        s.push_str(&unit(i));
        i += 1;
    }
    s.push_str(tail);
    s
}
fn flat(n: usize) -> String {
    fill(n, "<r>", "</r>", |_| "<a/>".into())
}
fn attrs(n: usize) -> String {
    // one element, distinct attributes
    fill(n, "<r", "/>", |i| format!(" a{i}=\"v\""))
}
fn attrs8(n: usize) -> String {
    fill(n, "<r>", "</r>", |_| {
        "<e a=\"1\" b=\"2\" c=\"3\" d=\"4\" e=\"5\" f=\"6\" g=\"7\" h=\"8\"/>".into()
    })
}
fn nsdecls(n: usize) -> String {
    fill(n, "<r", "/>", |i| format!(" xmlns:p{i}=\"u{i}\""))
}
fn deep_prefixes(n: usize) -> String {
    // 200 in-scope prefixes at the root, then nesting to depth 250 repeatedly
    let mut h = String::from("<r");
    for i in 0..200 {
        write!(h, " xmlns:p{i}=\"u{i}\"").unwrap();
    }
    h.push('>');
    fill(n, &h, "</r>", |i| {
        let mut s = String::new();
        for d in 0..50 {
            write!(s, "<p{}:x>", (i + d) % 200).unwrap();
        }
        for d in (0..50).rev() {
            write!(s, "</p{}:x>", (i + d) % 200).unwrap();
        }
        s
    })
}
fn nsattrs(n: usize) -> String {
    // attributes with one local name under many prefixes
    let mut h = String::from("<r");
    for i in 0..1000 {
        write!(h, " xmlns:p{i}=\"u{i}\"").unwrap();
    }
    fill(n, &h, "/>", |i| {
        format!(" p{}:x{}=\"v\"", i % 1000, i / 1000)
    })
}
fn long_names(n: usize) -> String {
    let name = "n".repeat(900_000);
    fill(n, "<r>", "</r>", |_| format!("<{name}/>"))
}
fn ids(n: usize) -> String {
    fill(
        n,
        "<!DOCTYPE r [<!ELEMENT r ANY><!ELEMENT a EMPTY><!ATTLIST a id ID #IMPLIED>]><r>",
        "</r>",
        |i| format!("<a id=\"i{i}\"/>"),
    )
}
fn tiny_text(n: usize) -> String {
    fill(n, "<r>", "</r>", |_| "x<!---->".into())
}
fn comments(n: usize) -> String {
    fill(n, "<r>", "</r>", |_| "<?p?>".into())
}
fn entity_refs(n: usize) -> String {
    fill(n, "<!DOCTYPE r [<!ENTITY e \"x\">]><r>", "</r>", |_| {
        "&e;".into()
    })
}
fn deep(n: usize) -> String {
    let d = 200;
    fill(n, "<r>", "</r>", |_| {
        format!("{}{}", "<a>".repeat(d), "</a>".repeat(d))
    })
}
fn many_entities(n: usize) -> String {
    fill(n, "<!DOCTYPE r [", "]><r/>", |i| {
        format!("<!ENTITY e{i} \"x\">")
    })
}
fn default_attrs(n: usize) -> String {
    let mut h = String::from("<!DOCTYPE r [<!ATTLIST a");
    for i in 0..1000 {
        write!(h, " d{i} CDATA \"v\"").unwrap();
    }
    h.push_str(">]><r>");
    fill(n, &h, "</r>", |_| "<a/>".into())
}
fn many_elements_decl(n: usize) -> String {
    fill(n, "<!DOCTYPE r [", "]><r/>", |i| {
        format!("<!ELEMENT e{i} (#PCDATA)><!ATTLIST e{i} a CDATA #IMPLIED>")
    })
}
fn rebind(n: usize) -> String {
    fill(n, "<r>", "</r>", |i| {
        format!("<a xmlns=\"u{i}\" xmlns:p=\"v{i}\"><p:b p:c=\"1\"/></a>")
    })
}

fn attrs_dense(n: usize) -> String {
    fill(n, "<r>", "</r>", |_| {
        "<e a=\"\" b=\"\" c=\"\" d=\"\" e=\"\" f=\"\" g=\"\" h=\"\" i=\"\" j=\"\"/>".into()
    })
}
fn ent_fanout(n: usize) -> String {
    let body = "<a/>".repeat(1000);
    fill(
        n,
        &format!("<!DOCTYPE r [<!ENTITY e \"{body}\">]><r>"),
        "</r>",
        |_| "&e;".into(),
    )
}

fn attr_refs(n: usize) -> String {
    fill(n, "<r a=\"", "\"/>", |_| "&amp;&#65;x".into())
}
fn text_refs(n: usize) -> String {
    fill(n, "<r>", "</r>", |_| "&amp;&#65;x&lt;".into())
}
fn cdata(n: usize) -> String {
    fill(n, "<r>", "</r>", |_| "<![CDATA[x]]>".into())
}
fn ws_attr(n: usize) -> String {
    fill(n, "<r a=\"", "\"/>", |_| " \t\n x".into())
}
fn content_model(n: usize) -> String {
    let mut s = String::from("<!DOCTYPE r [<!ELEMENT r (a");
    let mut i = 0;
    while s.len() < n {
        write!(s, "|b{i}").unwrap();
        i += 1;
    }
    s.push_str(")*>]><r/>");
    s
}
fn attlist_many(n: usize) -> String {
    fill(n, "<!DOCTYPE r [", "]><r/>", |i| {
        format!("<!ATTLIST e a{i} CDATA #IMPLIED>")
    })
}
fn pe_chain(n: usize) -> String {
    fill(
        n,
        "<!DOCTYPE r [<!ENTITY % p \"<!-- x -->\">",
        "]><r/>",
        |_| "%p;".into(),
    )
}
fn idrefs(n: usize) -> String {
    fill(
        n,
        "<!DOCTYPE r [<!ELEMENT r ANY><!ELEMENT a EMPTY><!ATTLIST a id ID #IMPLIED ref IDREF #IMPLIED>]><r>",
        "</r>",
        |i| format!("<a id=\"i{i}\" ref=\"i{}\"/>", i / 2),
    )
}
fn blanks(n: usize) -> String {
    fill(n, "<r>", "</r>", |_| " <a/> ".into())
}
fn nsclean(n: usize) -> String {
    fill(n, "<r xmlns:p=\"u\">", "</r>", |_| {
        "<a xmlns:p=\"u\"/>".into()
    })
}
fn dup_decl(n: usize) -> String {
    fill(n, "<!DOCTYPE r [", "]><r/>", |_| {
        "<!ENTITY e \"x\"><!ELEMENT a ANY><!ATTLIST a b CDATA #IMPLIED>".into()
    })
}

fn mixed_valid(n: usize) -> String {
    let mut h = String::from("<!DOCTYPE r [<!ELEMENT r (#PCDATA");
    for i in 0..mixk() {
        write!(h, "|a{i}").unwrap();
    }
    h.push_str(")*>");
    for i in 0..mixk() {
        write!(h, "<!ELEMENT a{i} EMPTY>").unwrap();
    }
    h.push_str("]><r>");
    fill(n, &h, "</r>", |i| format!("<a{}/>t", (i * 7) % mixk()))
}

fn req_attrs(n: usize) -> String {
    let k = mixk().min(1000);
    let mut h = String::from("<!DOCTYPE r [<!ELEMENT r ANY><!ELEMENT e EMPTY><!ATTLIST e");
    for i in 0..k {
        write!(h, " r{i} CDATA #REQUIRED").unwrap();
    }
    h.push_str(">]><r>");
    let unit = {
        let mut u = String::from("<e");
        for i in 0..k {
            write!(u, " r{i}=\"v\"").unwrap();
        }
        u.push_str("/>");
        u
    };
    fill(n, &h, "</r>", |_| unit.clone())
}
fn enum_attr(n: usize) -> String {
    fill(
        n,
        "<!DOCTYPE r [<!ATTLIST r a (",
        ")  #IMPLIED>]><r/>",
        |i| format!("v{i}|"),
    )
    .replace("|)  ", "|z)  ")
}
fn seq_model(n: usize) -> String {
    fill(n, "<!DOCTYPE r [<!ELEMENT r (a", ")>]><r/>", |i| {
        format!(",b{i}")
    })
}

fn limits() -> Limits {
    if std::env::var_os("DEFAULTS").is_some() {
        return Limits::DEFAULT;
    }
    Limits::DEFAULT
        .max_input_bytes(1usize << 40)
        .max_token_bytes(1usize << 40)
        .max_attributes(1usize << 40)
        .max_total_attributes(1usize << 40)
        .max_events(1usize << 40)
        .max_owned_bytes(1usize << 40)
        .max_dtd_bytes(1usize << 40)
        .max_entity_expansion_bytes(1usize << 40)
        .max_entity_references(1usize << 40)
}

fn run(mode: &str, input: &[u8], limits: Limits) -> Result<usize, String> {
    match mode {
        "dom" => Document::parse_with(
            input,
            yaxml::ParseOptions::DEFAULT
                .flags(flags())
                .limits(limits)
                .decode_limits(dl()),
        )
        .map(|d| d.nodes().count())
        .map_err(|e| format!("{e:?}")),
        "owned" => OwnedDocument::parse_with(
            input,
            yaxml::ParseOptions::DEFAULT
                .flags(flags())
                .limits(limits)
                .decode_limits(dl()),
        )
        .map(|d| d.nodes().count())
        .map_err(|e| format!("{e:?}")),
        "push" => {
            let chunk: usize = std::env::var("CHUNK")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1);
            let mut p = yaxml::PushParser::with_options(
                yaxml::ParseOptions::DEFAULT
                    .flags(flags())
                    .limits(limits)
                    .decode_limits(dl()),
            )
            .map_err(|e| format!("{e:?}"))?;
            let mut n = 0;
            for c in input.chunks(chunk) {
                p.feed(c, |_, _| {
                    n += 1;
                    std::ops::ControlFlow::Continue(())
                })
                .map_err(|e| format!("{e:?}"))?;
            }
            p.finish(|_, _| {
                n += 1;
                std::ops::ControlFlow::Continue(())
            })
            .map_err(|e| format!("{e:?}"))?;
            Ok(n)
        }
        "html" => {
            let p = yaxml::html::parse(
                input,
                yaxml::html::Options::DEFAULT.flags(flags()).limits(limits),
            )
            .map_err(|e| format!("{e:?}"))?;
            Ok(p.document.nodes().count() + usize::from(p.stopped) * 1_000_000_000)
        }
        "text" => {
            let mut r = yaxml::TextReader::with_options(
                input,
                yaxml::ParseOptions::DEFAULT
                    .flags(flags())
                    .limits(limits)
                    .decode_limits(dl()),
            )
            .map_err(|e| format!("{e:?}"))?;
            let mut n = 0;
            while r.read().map_err(|e| format!("{e:?}"))? {
                n += 1;
            }
            Ok(n)
        }
        "buffered" => {
            struct Tiny<'a>(&'a [u8], usize);
            impl std::io::Read for Tiny<'_> {
                fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
                    let k = self.0.len().min(self.1).min(b.len());
                    b[..k].copy_from_slice(&self.0[..k]);
                    self.0 = &self.0[k..];
                    Ok(k)
                }
            }
            let chunk: usize = std::env::var("CHUNK")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1);
            let mut r = yaxml::BufferedReader::with_options(
                std::io::BufReader::with_capacity(chunk, Tiny(input, chunk)),
                yaxml::ParseOptions::DEFAULT
                    .flags(flags())
                    .limits(limits)
                    .decode_limits(dl()),
            )
            .map_err(|e| format!("{e:?}"))?;
            let mut n = 0;
            while r.read_event().map_err(|e| format!("{e:?}"))?.is_some() {
                n += 1;
            }
            Ok(n)
        }
        _ => {
            let mut r = Reader::with_limits(input, limits).map_err(|e| format!("{e:?}"))?;
            let mut n = 0;
            while r.read_event().map_err(|e| format!("{e:?}"))?.is_some() {
                n += 1;
            }
            Ok(n)
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let which = args.first().map_or("all", String::as_str);
    let mode = args.get(1).map_or("dom", String::as_str);
    let sizes: Vec<f64> = if args.len() > 2 {
        args[2..]
            .iter()
            .map(|s| s.parse::<f64>().unwrap())
            .collect()
    } else {
        vec![1.0, 4.0, 16.0]
    };
    for (name, gen_) in FILLS.iter().filter(|(n, _)| which == "all" || *n == which) {
        let mut prev: Option<(usize, f64)> = None;
        for mib in &sizes {
            let input = gen_((mib * 1048576.0) as usize).into_bytes();
            let t = Instant::now();
            let r = run(mode, &input, limits());
            let secs = t.elapsed().as_secs_f64();
            let slope = prev.map_or(String::from("-"), |(b, s)| {
                format!(
                    "{:.2}",
                    (secs / s).ln() / (input.len() as f64 / b as f64).ln()
                )
            });
            println!(
                "{name:18} {mode:6} {:>9} B {secs:8.3} s slope {slope:>5} {}",
                input.len(),
                match r {
                    Ok(n) => format!("ok nodes={n}"),
                    Err(e) => format!("ERR {e}"),
                }
            );
            prev = Some((input.len(), secs));
        }
    }
}

fn flags() -> u32 {
    std::env::var("FLAGS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}
fn dl() -> yaxml::DecodeLimits {
    if std::env::var_os("DEFAULTS").is_some() {
        return yaxml::DecodeLimits::DEFAULT;
    }
    yaxml::DecodeLimits::DEFAULT
        .max_decoded_bytes(1usize << 40)
        .max_decoded_token_bytes(1usize << 40)
}

fn mixk() -> usize {
    std::env::var("MIXK")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2000)
}
