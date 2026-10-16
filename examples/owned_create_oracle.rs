//! Fixed rooted-construction comparisons against the external libxml2 helper.
#![forbid(unsafe_code)]

#[path = "support/process.rs"]
mod process;

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;
use yaxml::{
    AttributeSpec, EditError, EditLimits, ElementItem, ElementSpec, NameSpec, NamespaceSpec,
    OwnedDocument, PrefixPolicy, Writer,
};

const VERSION: &[u8] = b"YAXML-CREATE-1 compiled=21504 runtime=21504 tree=1 output=1\n";
const XML: &str = "http://www.w3.org/XML/1998/namespace";
const CASES: &[(&str, &str)] = &[
    ("plain", "<root/>"),
    (
        "attributes",
        "<root id=\"A&amp;B\" quote=\"&quot;&lt;&gt;\"/>",
    ),
    ("prefixed-root", "<p:root xmlns:p=\"urn:p\"/>"),
    ("default-root", "<root xmlns=\"urn:d\"/>"),
    (
        "inherited-prefix",
        "<root xmlns:p=\"urn:p\"><p:child p:id=\"17\"/></root>",
    ),
    (
        "default-undeclaration",
        "<root xmlns=\"urn:d\"><child xmlns=\"\"/></root>",
    ),
    (
        "prefix-shadow",
        "<p:root xmlns:p=\"urn:old\"><p:child xmlns:p=\"urn:new\" p:id=\"17\"/></p:root>",
    ),
    ("xml-attribute", "<root xml:lang=\"en\"/>"),
    ("unicode", "<élem clé=\"café🙂\"/>"),
    ("literal-text", "<root>A&amp;B &lt;x&gt; 🙂</root>"),
    (
        "automatic-element",
        "<root xmlns:p=\"urn:p\"><child xmlns=\"urn:p\"/></root>",
    ),
    (
        "automatic-attribute",
        "<root xmlns:p=\"urn:p\"><child p:id=\"17\"/></root>",
    ),
];

fn name<'a>(local: &'a str, namespace: Option<&'a str>, prefix: Option<&'a str>) -> NameSpec<'a> {
    NameSpec {
        local,
        namespace,
        prefix: PrefixPolicy::Exact(prefix),
    }
}

fn namespace<'a>(prefix: Option<&'a str>, uri: &'a str) -> ElementItem<'a> {
    ElementItem::Namespace(NamespaceSpec { prefix, uri })
}

fn attribute<'a>(name: NameSpec<'a>, value: &'a str) -> ElementItem<'a> {
    ElementItem::Attribute(AttributeSpec { name, value })
}

fn build_case(index: usize) -> Result<OwnedDocument, EditError> {
    let mut root = name("root", None, None);
    let mut items = Vec::new();
    match index {
        1 => items.extend([
            attribute(name("id", None, None), "A&B"),
            attribute(name("quote", None, None), "\"<>"),
        ]),
        2 => {
            root = name("root", Some("urn:p"), Some("p"));
            items.push(namespace(Some("p"), "urn:p"));
        }
        3 | 5 => {
            root = name("root", Some("urn:d"), None);
            items.push(namespace(None, "urn:d"));
        }
        4 | 10 | 11 => items.push(namespace(Some("p"), "urn:p")),
        6 => {
            root = name("root", Some("urn:old"), Some("p"));
            items.push(namespace(Some("p"), "urn:old"));
        }
        7 => items.push(attribute(name("lang", Some(XML), Some("xml")), "en")),
        8 => {
            root = name("élem", None, None);
            items.push(attribute(name("clé", None, None), "café🙂"));
        }
        _ => {}
    }
    let mut document = OwnedDocument::create(
        ElementSpec {
            name: root,
            items: &items,
        },
        EditLimits::default(),
    )?;
    let parent = document.root_element().id();
    let mut edit = document.edit(EditLimits::default())?;
    match index {
        4 => {
            edit.append_element(
                parent,
                ElementSpec {
                    name: name("child", Some("urn:p"), Some("p")),
                    items: &[attribute(name("id", Some("urn:p"), Some("p")), "17")],
                },
            )?;
        }
        5 => {
            edit.append_element(
                parent,
                ElementSpec {
                    name: name("child", None, None),
                    items: &[],
                },
            )?;
        }
        6 => {
            edit.append_element(
                parent,
                ElementSpec {
                    name: name("child", Some("urn:new"), Some("p")),
                    items: &[
                        namespace(Some("p"), "urn:new"),
                        attribute(name("id", Some("urn:new"), Some("p")), "17"),
                    ],
                },
            )?;
        }
        9 => {
            edit.append_text(parent, "A&B <x> 🙂")?;
        }
        10 => {
            edit.append_element(
                parent,
                ElementSpec {
                    name: NameSpec {
                        local: "child",
                        namespace: Some("urn:p"),
                        prefix: PrefixPolicy::Automatic,
                    },
                    items: &[],
                },
            )?;
        }
        11 => {
            edit.append_element(
                parent,
                ElementSpec {
                    name: name("child", None, None),
                    items: &[attribute(
                        NameSpec {
                            local: "id",
                            namespace: Some("urn:p"),
                            prefix: PrefixPolicy::Automatic,
                        },
                        "17",
                    )],
                },
            )?;
        }
        _ => {}
    }
    Ok(document)
}

fn frame(output: &mut Vec<u8>, label: &str, bytes: &[u8]) {
    const HEX: &[u8] = b"0123456789abcdef";
    output.extend_from_slice(label.as_bytes());
    output.push(b'\t');
    for byte in bytes {
        output.push(HEX[usize::from(byte >> 4)]);
        output.push(HEX[usize::from(byte & 15)]);
    }
    output.push(b'\n');
}

fn mismatch() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "construction oracle bytes differ",
    )
}

fn verify(actual: &[u8], expected: &[u8]) -> io::Result<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(mismatch())
    }
}

fn parse_args(args: impl IntoIterator<Item = OsString>) -> io::Result<PathBuf> {
    let mut args = args.into_iter();
    match (args.next(), args.next(), args.next()) {
        (Some(flag), Some(path), None) if flag == "--libxml2-helper" && !path.is_empty() => {
            Ok(path.into())
        }
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected --libxml2-helper PATH",
        )),
    }
}

fn compare(helper: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let limits = process::ProcessLimits {
        timeout: Duration::from_secs(5),
        max_output_bytes: 65536,
    };
    let version = process::run(helper, &["--version"], b"", limits)?;
    if !version.status.success() || !version.stderr.is_empty() {
        return Err("construction oracle version command failed".into());
    }
    verify(&version.stdout, VERSION)?;
    let mut expected = Vec::new();
    for (index, (label, literal)) in CASES.iter().enumerate() {
        let document = build_case(index)?;
        let mut writer = Writer::new(Vec::new());
        writer.write_owned_document(&document)?;
        let actual = writer.into_inner();
        verify(&actual, literal.as_bytes())?;
        frame(&mut expected, label, &actual);
    }
    let actual = process::run(helper, &[], b"", limits)?;
    if !actual.status.success() || !actual.stderr.is_empty() {
        return Err("construction oracle comparison command failed".into());
    }
    verify(&actual.stdout, &expected)?;
    Ok(expected)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let helper = parse_args(std::env::args_os().skip(1))?;
    let table = compare(&helper)?;
    io::stdout().lock().write_all(&table)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_require_one_explicit_helper_and_no_extra_fields() {
        assert_eq!(
            parse_args(["--libxml2-helper", "oracle"].map(OsString::from)).unwrap(),
            PathBuf::from("oracle")
        );
        for args in [
            vec![],
            vec!["oracle"],
            vec!["--libxml2-helper"],
            vec!["--other", "oracle"],
            vec!["--libxml2-helper", ""],
            vec!["--libxml2-helper", "oracle", "extra"],
        ] {
            assert!(parse_args(args.into_iter().map(OsString::from)).is_err());
        }
    }

    #[test]
    fn frame_is_literal_and_comparison_rejects_every_changed_byte() {
        let mut output = Vec::new();
        frame(&mut output, "x", b"\0\n\xff");
        assert_eq!(output, b"x\t000aff\n");
        verify(&output, b"x\t000aff\n").unwrap();
        for index in 0..output.len() {
            let mut changed = output.clone();
            changed[index] ^= 1;
            assert!(verify(&changed, &output).is_err());
        }
        assert!(verify(&output[..output.len() - 1], &output).is_err());
        assert!(
            verify(
                VERSION,
                b"YAXML-CREATE-1 compiled=21504 runtime=21505 tree=1 output=1\n"
            )
            .is_err()
        );
    }
}
