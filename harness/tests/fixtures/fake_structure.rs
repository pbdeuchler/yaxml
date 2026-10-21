//! Trusted protocol double. This does not establish libxml2 behavior.
#![forbid(unsafe_code)]

use std::io::{self, Read, Write};

const MAGIC: &str = "YAXML-STRUCTURE-1";

fn frame(output: &mut Vec<u8>, value: &[u8]) {
    output.extend_from_slice(format!("{}:", value.len()).as_bytes());
    output.extend_from_slice(value);
    output.push(b'\n');
}

fn node(output: &mut Vec<u8>, kind: char, parent: usize, name: &str, value: &str) {
    output.extend_from_slice(format!("{kind}\n{parent}\n").as_bytes());
    frame(output, name.as_bytes());
    frame(output, b"");
    frame(output, value.as_bytes());
    output.extend_from_slice(b"0\n");
}

fn main() -> io::Result<()> {
    let executable = std::env::current_exe()?;
    let mode = executable
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap();
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args == ["--version"] {
        let version = if mode == "other-version" {
            "21503"
        } else {
            "21504"
        };
        let runtime = if mode == "runtime-mismatch" {
            "21503"
        } else {
            version
        };
        println!("{MAGIC} compiled={version} runtime={runtime} tree=1 output=1");
        return Ok(());
    }
    if !args.is_empty() {
        return Err(io::Error::other("unexpected fixture arguments"));
    }
    let mut input = Vec::new();
    io::stdin().take(4096).read_to_end(&mut input)?;
    if mode == "status" {
        std::process::exit(70);
    }
    let source = if input.starts_with(format!("{MAGIC}\n5\n").as_bytes()) {
        "<!--pre--><r><c/></r><!--post-->"
    } else if input.windows(8).any(|bytes| bytes == b"<r>a</r>") {
        "<r>a</r>"
    } else {
        "<r/>"
    };
    let mut expected = format!(
        "{MAGIC}\n{}\n{}\n",
        if source.starts_with("<!--") { 5 } else { 1 },
        if source.starts_with("<!--") { 2 } else { 1 }
    )
    .into_bytes();
    frame(&mut expected, source.as_bytes());
    frame(&mut expected, b"");
    frame(
        &mut expected,
        if source == "<r>a</r>" {
            b"b"
        } else if source == "<r/>" {
            b"x"
        } else {
            b""
        },
    );
    if input != expected {
        return Err(io::Error::other("unexpected structural fixture request"));
    }
    let mut output = format!("{MAGIC}\n").into_bytes();
    if source.starts_with("<!--") {
        output.extend_from_slice(b"5\n");
        node(&mut output, 'D', 0, "", "");
        node(&mut output, 'M', 1, "", "pre");
        node(&mut output, 'E', 1, "r", "");
        node(&mut output, 'E', 3, "c", "");
        node(&mut output, 'M', 1, "", "post");
        frame(&mut output, source.as_bytes());
        output.extend_from_slice(b"3\n3\n");
        node(&mut output, 'D', 0, "", "");
        node(&mut output, 'M', 1, "", "pre");
        node(&mut output, 'M', 1, "", "post");
        frame(&mut output, b"<!--pre--><!--post-->");
    } else {
        output.extend_from_slice(if source == "<r/>" { b"2\n" } else { b"3\n" });
        node(&mut output, 'D', 0, "", "");
        node(
            &mut output,
            'E',
            1,
            if mode == "before" { "s" } else { "r" },
            "",
        );
        if source != "<r/>" {
            node(&mut output, 'T', 2, "", "a");
        }
        frame(
            &mut output,
            if mode == "before-bytes" {
                b"<s/>"
            } else {
                source.as_bytes()
            },
        );
        output.extend_from_slice(if source != "<r/>" || mode == "returned" {
            b"1\n3\n"
        } else {
            b"0\n3\n"
        });
        node(&mut output, 'D', 0, "", "");
        node(&mut output, 'E', 1, "r", "");
        node(
            &mut output,
            'T',
            2,
            "",
            if mode == "after" {
                "y"
            } else if source == "<r/>" {
                "x"
            } else {
                "ab"
            },
        );
        frame(
            &mut output,
            if mode == "after-bytes" {
                b"<r>y</r>"
            } else if source == "<r/>" {
                b"<r>x</r>"
            } else {
                b"<r>ab</r>"
            },
        );
    }
    if mode == "truncated" {
        output.pop();
    }
    if mode == "trailing" {
        output.extend_from_slice(b"extra");
    }
    if mode == "diagnostics" {
        eprintln!("injected structural diagnostic");
    }
    io::stdout().write_all(&output)
}
