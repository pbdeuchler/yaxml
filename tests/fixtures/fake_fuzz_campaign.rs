use std::env;
use std::fs;
use std::io::{self, Read};
use std::path::Path;
use std::process;
use std::thread;
use std::time::Duration;

fn main() {
    let mode = env::args_os()
        .next()
        .and_then(|name| {
            Path::new(&name)
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        })
        .unwrap_or_default();
    if mode.starts_with("target") {
        target(&mode);
    } else if mode.starts_with("profdata") {
        profdata(&mode);
    } else if mode.starts_with("cov") {
        cov(&mode);
    } else {
        process::exit(90);
    }
}

fn target(mode: &str) {
    match mode {
        "target_crash" => process::exit(7),
        "target_stderr" => {
            eprintln!("controlled target diagnostic");
            process::exit(0);
        }
        "target_timeout" => thread::sleep(Duration::from_secs(2)),
        "target_output" => print!("{}", "x".repeat(4096)),
        "target_large_profile" => {
            let path = env::var_os("LLVM_PROFILE_FILE").expect("profile path is set");
            fs::write(path, vec![b'x'; 1024 * 1024]).unwrap();
            return;
        }
        _ => {}
    }
    let mut input = Vec::new();
    io::stdin().read_to_end(&mut input).unwrap();
    match mode {
        "target_missing" => return,
        "target_corrupt" => write_profile(b"not a valid profile"),
        _ => write_profile(if input == b"x" { b"low" } else { b"high" }),
    }
}

fn write_profile(contents: &[u8]) {
    let path = env::var_os("LLVM_PROFILE_FILE").expect("profile path is set");
    fs::write(path, contents).unwrap();
}

fn profdata(mode: &str) {
    if env::args().any(|arg| arg == "--version") {
        println!("LLVM version 22.1.6-rust-fixture");
        return;
    }
    if mode == "profdata_fail" {
        eprintln!("controlled merge failure");
        process::exit(4);
    }
    let args: Vec<_> = env::args_os().skip(1).collect();
    let output = args
        .windows(2)
        .find(|pair| pair[0] == "-o")
        .map(|pair| &pair[1])
        .expect("merge output path");
    let mut combined = Vec::new();
    for path in args.iter().skip_while(|arg| *arg != output).skip(1) {
        let bytes = fs::read(path).unwrap();
        if !bytes.windows(3).any(|window| window == b"low")
            && !bytes.windows(4).any(|window| window == b"high")
        {
            eprintln!("controlled corrupt profile");
            process::exit(5);
        }
        if !combined.is_empty() {
            combined.push(b' ');
        }
        combined.extend(bytes);
    }
    if mode == "profdata_diag" {
        eprintln!("controlled merge diagnostic");
    }
    fs::write(output, combined).unwrap();
}

fn cov(mode: &str) {
    if env::args().any(|arg| arg == "--version") {
        let version = if mode == "cov_version_mismatch" {
            "22.2.0"
        } else {
            "22.1.6"
        };
        println!("LLVM version {version}-rust-fixture");
        return;
    }
    if mode == "cov_malformed" {
        println!("unexpected report schema");
        return;
    }
    if mode == "cov_pause" {
        thread::sleep(Duration::from_millis(500));
    }
    let profile = env::args()
        .find_map(|argument| {
            argument
                .strip_prefix("--instr-profile=")
                .map(str::to_owned)
        })
        .expect("profile argument");
    let profile = fs::read(profile).unwrap();
    let novel = profile.windows(4).any(|window| window == b"high");
    let drift = mode == "cov_drift" && novel;
    report(novel, drift);
}

fn report(novel: bool, drift: bool) {
    println!("Filename Regions Missed Regions Cover Functions Missed Functions Executed Lines Missed Lines Cover Branches Missed Branches Cover");
    println!("-------------------------------------------------------------------------------------------------------------");
    let mut totals = [0_u64; 8];
    for index in 0..13 {
        let region_total = if drift { 2 } else { 1 };
        let region_covered = u64::from(index == 0 && novel);
        let function_total = 1;
        let function_covered = u64::from(index == 0 && novel);
        let line_total = 1;
        let line_covered = u64::from(index == 0 && novel);
        let branch_total = 1;
        let branch_covered = u64::from(index == 0 && novel);
        totals[0] += region_total;
        totals[1] += region_total - region_covered;
        totals[2] += function_total;
        totals[3] += function_total - function_covered;
        totals[4] += line_total;
        totals[5] += line_total - line_covered;
        totals[6] += branch_total;
        totals[7] += branch_total - branch_covered;
        println!(
            "m{index:02}.rs {region_total} {} {} {function_total} {} {} {line_total} {} {} {branch_total} {} {}",
            region_total - region_covered,
            percent(region_covered, region_total),
            function_total - function_covered,
            percent(function_covered, function_total),
            line_total - line_covered,
            percent(line_covered, line_total),
            branch_total - branch_covered,
            percent(branch_covered, branch_total),
        );
    }
    println!(
        "TOTAL {} {} {} {} {} {} {} {} {} {} {} {}",
        totals[0],
        totals[1],
        percent(totals[0] - totals[1], totals[0]),
        totals[2],
        totals[3],
        percent(totals[2] - totals[3], totals[2]),
        totals[4],
        totals[5],
        percent(totals[4] - totals[5], totals[4]),
        totals[6],
        totals[7],
        percent(totals[6] - totals[7], totals[6]),
    );
}

fn percent(covered: u64, total: u64) -> String {
    if total == 0 {
        "-".to_owned()
    } else {
        format!("{:.1}%", covered as f64 * 100.0 / total as f64)
    }
}
