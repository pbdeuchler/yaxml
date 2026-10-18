use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::thread;
use std::time::Duration;

// Independent reviewed fixture literals: do not import production constants.
const PUBLIC_SOURCE_PATHS: [&str; 17] = [
    "src/buffered.rs",
    "src/dom.rs",
    "src/encoding.rs",
    "src/error.rs",
    "src/escape.rs",
    "src/lib.rs",
    "src/name.rs",
    "src/origin.rs",
    "src/owned.rs",
    "src/owned/construction.rs",
    "src/parser.rs",
    "src/push.rs",
    "src/reader.rs",
    "src/serialize.rs",
    "src/stream.rs",
    "src/tokenizer.rs",
    "src/writer.rs",
];
const PUBLIC_REPORT_NAMES: [&str; 16] = [
    "buffered.rs",
    "dom.rs",
    "encoding.rs",
    "error.rs",
    "escape.rs",
    "name.rs",
    "origin.rs",
    "owned.rs",
    "owned/construction.rs",
    "parser.rs",
    "push.rs",
    "reader.rs",
    "serialize.rs",
    "stream.rs",
    "tokenizer.rs",
    "writer.rs",
];

const STABLE_SOURCE_PATHS: [&str; 21] = [
    "src/dtd.rs",
    "src/dtd_catalog.rs",
    "src/dtd_controller.rs",
    "src/dtd_controller/content.rs",
    "src/dtd_frames.rs",
    "src/dtd_reference.rs",
    "src/dtd_scan.rs",
    "src/dtd_tag.rs",
    "src/encoding.rs",
    "src/error.rs",
    "src/escape.rs",
    "src/name.rs",
    "src/origin.rs",
    "src/parser.rs",
    "src/reader.rs",
    "src/reader/dtd.rs",
    "src/stream/dtd.rs",
    "src/tokenizer.rs",
    "src/tokenizer/local.rs",
    "src/tokenizer/reference.rs",
    "src/tokenizer/text.rs",
];
const STABLE_REPORT_NAMES: [&str; 21] = [
    "dtd.rs",
    "dtd_catalog.rs",
    "dtd_controller.rs",
    "dtd_controller/content.rs",
    "dtd_frames.rs",
    "dtd_reference.rs",
    "dtd_scan.rs",
    "dtd_tag.rs",
    "encoding.rs",
    "error.rs",
    "escape.rs",
    "name.rs",
    "origin.rs",
    "parser.rs",
    "reader.rs",
    "reader/dtd.rs",
    "stream/dtd.rs",
    "tokenizer.rs",
    "tokenizer/local.rs",
    "tokenizer/reference.rs",
    "tokenizer/text.rs",
];
const MASTER_SOURCE_PATHS: [&str; 32] = [
    "src/buffered.rs",
    "src/dom.rs",
    "src/dtd.rs",
    "src/dtd_catalog.rs",
    "src/dtd_controller.rs",
    "src/dtd_controller/content.rs",
    "src/dtd_frames.rs",
    "src/dtd_reference.rs",
    "src/dtd_scan.rs",
    "src/dtd_tag.rs",
    "src/dtd_writer.rs",
    "src/encoding.rs",
    "src/error.rs",
    "src/escape.rs",
    "src/lib.rs",
    "src/name.rs",
    "src/origin.rs",
    "src/owned.rs",
    "src/owned/construction.rs",
    "src/parser.rs",
    "src/push.rs",
    "src/reader.rs",
    "src/reader/dtd.rs",
    "src/serialize.rs",
    "src/stream.rs",
    "src/stream/dtd.rs",
    "src/stream/dtd_buffered.rs",
    "src/tokenizer.rs",
    "src/tokenizer/local.rs",
    "src/tokenizer/reference.rs",
    "src/tokenizer/text.rs",
    "src/writer.rs",
];
const MASTER_REPORT_NAMES: [&str; 31] = [
    "buffered.rs",
    "dom.rs",
    "dtd.rs",
    "dtd_catalog.rs",
    "dtd_controller.rs",
    "dtd_controller/content.rs",
    "dtd_frames.rs",
    "dtd_reference.rs",
    "dtd_scan.rs",
    "dtd_tag.rs",
    "dtd_writer.rs",
    "encoding.rs",
    "error.rs",
    "escape.rs",
    "name.rs",
    "origin.rs",
    "owned.rs",
    "owned/construction.rs",
    "parser.rs",
    "push.rs",
    "reader.rs",
    "reader/dtd.rs",
    "serialize.rs",
    "stream.rs",
    "stream/dtd.rs",
    "stream/dtd_buffered.rs",
    "tokenizer.rs",
    "tokenizer/local.rs",
    "tokenizer/reference.rs",
    "tokenizer/text.rs",
    "writer.rs",
];

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
        "target_change_self" => append_to_executable(),
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
    if mode == "profdata_large_output" {
        fs::write(output, vec![b'p'; 1024 * 1024]).unwrap();
    }
    if mode == "profdata_change_self" || mode == "profdata_change_self_fail" {
        append_to_executable();
    }
    if mode == "profdata_change_self_fail" {
        eprintln!("controlled merge failure after changing tool identity");
        process::exit(6);
    }
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
    if !env::args().any(|argument| argument == "--color=0") {
        eprintln!("expected LLVM boolean color option --color=0");
        process::exit(93);
    }
    let (source_paths, report_names, source_error): (&[&str], &[&str], &str) = match mode {
        "cov_stable_profile" | "cov_stable_profile_changed" => {
            (
                &STABLE_SOURCE_PATHS,
                &STABLE_REPORT_NAMES,
                "expected the exact StableReaderV1 21-file source set after --sources",
            )
        }
        "cov_master_profile" => (
            &MASTER_SOURCE_PATHS,
            &MASTER_REPORT_NAMES,
            "expected the exact MasterLibtestMapV1 32-file source set after --sources",
        ),
        _ => (
            &PUBLIC_SOURCE_PATHS,
            &PUBLIC_REPORT_NAMES,
            "expected the exact PublicReplayV2 17-file source set after --sources",
        ),
    };
    if !has_exact_source_list(source_paths) {
        eprintln!("{source_error}");
        process::exit(92);
    }
    if mode.starts_with("cov_change_source") {
        change_first_source(&mode);
    }
    if mode == "cov_change_self" || mode == "cov_change_self_fail" {
        append_to_executable();
    }
    if mode == "cov_change_self_fail" {
        println!("unexpected report schema");
        return;
    }
    if mode == "cov_change_source_fail" {
        println!("unexpected report schema");
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
        .find_map(|argument| argument.strip_prefix("--instr-profile=").map(str::to_owned))
        .expect("profile argument");
    if mode == "cov_post_commit_fail" {
        let root = Path::new(&profile)
            .parent()
            .and_then(Path::parent)
            .expect("output root");
        let counter_path = root.join("post-commit-cov-count");
        let count = fs::read_to_string(&counter_path)
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0)
            + 1;
        fs::write(counter_path, count.to_string()).unwrap();
        if count >= 3 {
            let prior_raw = root.join("profiles/final.profraw");
            fs::remove_file(&prior_raw).unwrap();
            fs::create_dir(prior_raw).unwrap();
            fs::create_dir(root.join("failures/first-failure.input")).unwrap();
        }
    }
    let profile = fs::read(profile).unwrap();
    let novel = profile.windows(4).any(|window| window == b"high");
    if mode == "cov_cumulative_commit_fail" && novel {
        let profile_path = env::args()
            .find_map(|argument| argument.strip_prefix("--instr-profile=").map(str::to_owned))
            .expect("profile argument");
        let root = Path::new(&profile_path)
            .parent()
            .and_then(Path::parent)
            .expect("output root");
        fs::copy(
            root.join("cases.tsv"),
            root.join("pre-commit-journal.snapshot"),
        )
        .unwrap();
        let cumulative = root.join("profiles/cumulative.profdata");
        fs::rename(&cumulative, root.join("profiles/cumulative.prior")).unwrap();
        fs::create_dir(cumulative).unwrap();
    }
    let drift = mode == "cov_drift" && novel;
    let per_file_drift = mode == "cov_per_file_drift" && novel;
    report(
        report_names,
        novel,
        drift,
        per_file_drift,
        mode == "cov_stable_profile_changed",
    );
}

fn has_exact_source_list(expected_paths: &[&str]) -> bool {
    let arguments: Vec<_> = env::args_os().skip(1).collect();
    let Some(sources_index) = arguments
        .iter()
        .position(|argument| argument == "--sources")
    else {
        return false;
    };
    let sources = &arguments[sources_index + 1..];
    if sources.len() != expected_paths.len()
        || !sources.iter().all(|source| {
            let path = Path::new(source);
            path.is_absolute() && path.is_file()
        })
    {
        return false;
    }
    let Ok(paths) = sources
        .iter()
        .map(|source| Path::new(source).canonicalize())
        .collect::<Result<Vec<_>, _>>()
    else {
        return false;
    };
    let Some(first_relative) = expected_paths.first() else {
        return false;
    };
    let mut source_root = paths[0].clone();
    for _ in Path::new(first_relative).components() {
        if !source_root.pop() {
            return false;
        }
    }
    let relative: BTreeSet<_> = paths
        .iter()
        .filter_map(|path| path.strip_prefix(&source_root).ok())
        .map(Path::to_path_buf)
        .collect();
    let expected: BTreeSet<_> = expected_paths.iter().map(PathBuf::from).collect();
    paths.iter().collect::<BTreeSet<_>>().len() == expected_paths.len()
        && relative.len() == paths.len()
        && relative == expected
}

fn change_first_source(mode: &str) {
    let arguments: Vec<_> = env::args_os().skip(1).collect();
    let index = arguments
        .iter()
        .position(|argument| argument == "--sources")
        .expect("sources argument");
    let source = arguments.get(index + 1).expect("first source path");
    fs::write(source, format!("// changed during campaign: {mode}\n")).unwrap();
}

fn append_to_executable() {
    let path = env::current_exe().expect("current executable");
    fs::OpenOptions::new()
        .append(true)
        .open(path)
        .expect("open running executable for identity mutation")
        .write_all(b"fixture identity mutation")
        .expect("append identity mutation");
}

fn report(
    names: &[&str],
    novel: bool,
    drift: bool,
    per_file_drift: bool,
    cross_report_drift: bool,
) {
    println!(
        "Filename Regions Missed Regions Cover Functions Missed Functions Executed Lines Missed Lines Cover Branches Missed Branches Cover"
    );
    println!(
        "-------------------------------------------------------------------------------------------------------------"
    );
    let mut totals = [0_u64; 8];
    for (index, name) in names.iter().enumerate() {
        let total = if drift
            || (per_file_drift && index == 0)
            || (cross_report_drift && *name == "reader/dtd.rs")
        {
            2
        } else if per_file_drift && index == 1 {
            0
        } else {
            1
        };
        let region_total = total;
        let region_covered = u64::from(index == 0 && novel);
        let function_total = total;
        let function_covered = u64::from(index == 0 && novel);
        let line_total = total;
        let line_covered = u64::from(index == 0 && novel);
        let branch_total = total;
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
            "{name} {region_total} {} {} {function_total} {} {} {line_total} {} {} {branch_total} {} {}",
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
