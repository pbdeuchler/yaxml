//! Standalone trusted oracle double for command-level replay regressions.
#![forbid(unsafe_code)]

use std::io::{self, Read, Write};

fn main() -> io::Result<()> {
    let executable = std::env::current_exe()?;
    let mode = executable
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::other("fixture executable has no mode"))?;
    if std::env::args()
        .skip(1)
        .any(|argument| argument == "--version")
    {
        let version = if mode == "other-version" {
            "21503"
        } else {
            "21504"
        };
        println!("xmllint: using libxml version {version}\n   compiled with: Tree XPath");
        return Ok(());
    }
    let mut input = Vec::new();
    io::stdin().take(1024).read_to_end(&mut input)?;
    if !matches!(input.as_slice(), b"<r/>" | b"<r></r>" | b"<r>") {
        return Err(io::Error::other(
            "fixture received unexpected testcase bytes",
        ));
    }
    match mode {
        "serialize-reject" if input == b"<r/>" => std::process::exit(4),
        "serialize-disagree" if input == b"<r/>" => {
            io::stdout().write_all(b"1:11:11:11:s1:s0:0:1:0\n")?;
        }
        "serialize-inconclusive" if input == b"<r/>" => std::process::exit(9),
        "agree" | "other-version" | "serialize-reject" | "serialize-disagree"
        | "serialize-inconclusive" => {
            io::stdout().write_all(b"1:11:11:11:r1:r0:0:1:0\n")?;
        }
        "disagree" => {
            io::stdout().write_all(b"1:11:11:11:s1:s0:0:1:0\n")?;
        }
        "reject" => {
            eprintln!("fixture parser rejection");
            std::process::exit(4);
        }
        "inconclusive" => std::process::exit(9),
        "diagnostics" => {
            io::stdout().write_all(b"1:11:11:11:r1:r0:0:1:0\n")?;
            eprintln!("parser error: injected diagnostic despite successful exit");
        }
        _ => return Err(io::Error::other("unknown fixture executable mode")),
    }
    Ok(())
}
