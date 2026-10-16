//! Controlled executable for testing the driver's process and publication gates.
#![forbid(unsafe_code)]

use std::{env, fs, process, thread, time::Duration};

fn main() {
    let executable = env::current_exe().unwrap();
    let directory = executable.parent().unwrap();
    let c = executable.file_stem().unwrap() == "c-helper";
    let engine = if c { "libxml2" } else { "yaxml" };
    let args: Vec<_> = env::args().skip(1).collect();
    if args == ["--version"] {
        let version = if c { "21504" } else { "0.1.0" };
        println!("YAXML-PARSE-BENCH-1\tengine={engine}\tversion={version}");
        return;
    }
    assert_eq!(args.len(), 4);
    let counter = directory.join(format!("{engine}.calls"));
    let call = fs::read_to_string(&counter).unwrap_or_default().parse::<usize>().unwrap_or(0) + 1;
    fs::write(counter, call.to_string()).unwrap();
    let behavior = fs::read_to_string(directory.join("behavior")).unwrap();
    let active = c && if behavior == "late-exit" { call == 3 } else { call == 2 };
    let mut bytes = fs::read(&args[1]).unwrap().len();
    let mut hash = "0123456789abcdef";
    if active {
        match behavior.as_str() {
            "schema" => { println!("wrong schema"); return; }
            "wrong-work" => bytes += 1,
            "semantic" => hash = "0123456789abcdee",
            "exit" | "late-exit" => { eprintln!("controlled failure"); process::exit(2); }
            "large-output" => { println!("{}", "x".repeat(5000)); return; }
            "input-change" => { fs::write(&args[1], b"changed").unwrap(); }
            "timeout" => { thread::sleep(Duration::from_secs(30)); }
            "stderr" => eprintln!("unexpected successful diagnostics"),
            _ => {}
        }
    }
    println!("YAXML-PARSE-BENCH-1\tengine={engine}\tmode={}\tbytes={bytes}\titerations={}\tchunk={}\tparse_ns=1\tobserve_ns=0\tdrop_ns=1\telements=1\tattributes=0\tnamespaces=0\ttext_bytes=0\tcomments=0\tpis=0\thash={hash}", args[0], args[2], args[3]);
}
