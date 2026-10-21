#![forbid(unsafe_code)]

#[path = "../examples/support/process.rs"]
mod process;

use process::{ProcessLimits, ProcessOutput};
use std::fs::{self, DirBuilder};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> io::Result<Self> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        for attempt in 0..128 {
            let path = std::env::temp_dir().join(format!(
                "yaxml-xmlconf-catalog-{}-{stamp}-{attempt}",
                std::process::id()
            ));
            let mut builder = DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not create unique XML catalog test directory",
        ))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn arg(path: &Path) -> &str {
    path.to_str().expect("temporary test path must be UTF-8")
}

fn helper() -> PathBuf {
    let value = std::env::var_os("YAXML_XMLCONF_HELPER")
        .expect("set YAXML_XMLCONF_HELPER to the pinned libxml2 catalog helper");
    let path = PathBuf::from(value);
    assert!(path.is_absolute(), "helper path must be absolute");
    assert!(
        path.is_file(),
        "helper executable does not exist: {}",
        path.display()
    );
    path
}

fn execute(helper: &Path, args: &[&str]) -> ProcessOutput {
    process::run(
        helper,
        args,
        b"",
        ProcessLimits {
            timeout: Duration::from_secs(30),
            max_output_bytes: 2 * 1024 * 1024,
        },
    )
    .unwrap_or_else(|error| panic!("failed to run catalog helper: {error}"))
}

fn write(path: &Path, value: &str) {
    fs::write(path, value).unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
}

fn catalog(body: &str) -> String {
    format!(
        "<!DOCTYPE TESTSUITE [\n\
         <!ELEMENT TESTSUITE (TESTCASES+)>\n\
         <!ELEMENT TESTCASES (TEST*)>\n\
         <!ATTLIST TESTCASES xml:base CDATA #IMPLIED>\n\
         <!ELEMENT TEST EMPTY>\n\
         <!ATTLIST TEST ID ID #REQUIRED TYPE (valid|invalid|not-wf|error) #REQUIRED\n\
         ENTITIES (both|none|parameter|general) \"none\"\n\
         RECOMMENDATION (XML1.0|XML1.1|NS1.0|NS1.1|XML1.0-errata2e|XML1.0-errata3e|XML1.0-errata4e|NS1.0-errata1e) \"XML1.0\"\n\
         VERSION NMTOKENS #IMPLIED EDITION NMTOKENS #IMPLIED\n\
         NAMESPACE (yes|no) \"yes\" URI CDATA #REQUIRED\n\
         OUTPUT CDATA #IMPLIED OUTPUT3 CDATA #IMPLIED SECTIONS CDATA #REQUIRED>\n\
         ]>\n<TESTSUITE>{body}</TESTSUITE>\n"
    )
}

fn write_catalog(root: &Path, name: &str, contents: &str) -> PathBuf {
    let path = root.join(name);
    write(&path, contents);
    path
}

fn assert_rejected(helper: &Path, args: &[&str], expected: &str) {
    let output = execute(helper, args);
    assert!(
        !output.status.success(),
        "accepted invalid helper invocation"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(expected),
        "missing diagnostic {expected:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "requires pinned libxml2 catalog helper; external oracle CI runs this"]
fn pinned_catalog_cli_emits_complete_rows_and_rejects_bad_manifests() {
    let helper = helper();
    let version = execute(&helper, &["--version"]);
    assert!(version.status.success());
    assert_eq!(version.stdout, b"compiled=21504 runtime=21504\n");
    assert!(version.stderr.is_empty());
    assert_rejected(&helper, &[], "usage:");

    let temp = TempDir::new().unwrap();
    let root = temp.0.join("suite");
    fs::create_dir(&root).unwrap();
    let cases = root.join("cases");
    fs::create_dir_all(cases.join("nested")).unwrap();
    write(
        &cases.join("nested/with-external.xml"),
        "<!DOCTYPE root [<!ENTITY probe SYSTEM \"file:///does-not-exist\">]><root>&probe;</root>",
    );
    let good = write_catalog(
        &root,
        "catalog.xml",
        &catalog(
            "<TESTCASES xml:base=\"cases/nested/\">\
             <TEST ID=\"v\" TYPE=\"valid\" URI=\"with-external.xml\" SECTIONS=\"well-formed\"/>\
             <TEST ID=\"i\" TYPE=\"invalid\" ENTITIES=\"general\" VERSION=\"1.0\" EDITION=\"5\" NAMESPACE=\"no\" URI=\"absent-i.xml\" OUTPUT=\"out.xml\" OUTPUT3=\"out3.xml\" SECTIONS=\"validation\"/>\
             <TEST ID=\"n\" TYPE=\"not-wf\" URI=\"absent-n.xml\" SECTIONS=\"syntax\"/>\
             <TEST ID=\"e\" TYPE=\"error\" URI=\"absent-e.xml\" SECTIONS=\"optional\"/>\
             </TESTCASES>\n",
        ),
    );
    let output = execute(&helper, &[arg(&good), arg(&root)]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    assert_eq!(
        output.stdout,
        concat!(
            "yaxml-xmlconf-v1\t4\n",
            "v\tvalid\tnone\tXML1.0\t\t\tyes\twith-external.xml\tcases/nested/with-external.xml\t\t\twell-formed\n",
            "i\tinvalid\tgeneral\tXML1.0\t1.0\t5\tno\tabsent-i.xml\tcases/nested/absent-i.xml\tout.xml\tout3.xml\tvalidation\n",
            "n\tnot-wf\tnone\tXML1.0\t\t\tyes\tabsent-n.xml\tcases/nested/absent-n.xml\t\t\tsyntax\n",
            "e\terror\tnone\tXML1.0\t\t\tyes\tabsent-e.xml\tcases/nested/absent-e.xml\t\t\toptional\n",
        )
        .as_bytes()
    );
    let slash_root = execute(&helper, &[arg(&good), "/"]);
    assert!(
        slash_root.status.success(),
        "{}",
        String::from_utf8_lossy(&slash_root.stderr)
    );
    let first_row = String::from_utf8(slash_root.stdout)
        .unwrap()
        .lines()
        .nth(1)
        .unwrap()
        .split('\t')
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let absolute_case = fs::canonicalize(cases.join("nested/with-external.xml")).unwrap();
    assert_eq!(first_row[8], arg(&absolute_case).trim_start_matches('/'));

    let malformed = write_catalog(&root, "malformed.xml", "<TESTSUITE>");
    assert_rejected(
        &helper,
        &[arg(&malformed), arg(&root)],
        "could not parse catalog",
    );
    let wrong_root = write_catalog(&root, "wrong-root.xml", "<OTHER/>");
    assert_rejected(
        &helper,
        &[arg(&wrong_root), arg(&root)],
        "root must be TESTSUITE",
    );
    let missing_required = write_catalog(
        &root,
        "missing-required.xml",
        "<TESTSUITE><TESTCASES><TEST TYPE=\"valid\" URI=\"x.xml\" SECTIONS=\"x\"/></TESTCASES></TESTSUITE>",
    );
    assert_rejected(
        &helper,
        &[arg(&missing_required), arg(&root)],
        "missing required ID",
    );
    let duplicate = write_catalog(
        &root,
        "duplicate.xml",
        &catalog(
            "<TESTCASES><TEST ID=\"same\" TYPE=\"valid\" URI=\"a.xml\" SECTIONS=\"x\"/>\
             <TEST ID=\"same\" TYPE=\"error\" URI=\"b.xml\" SECTIONS=\"x\"/></TESTCASES>",
        ),
    );
    assert_rejected(
        &helper,
        &[arg(&duplicate), arg(&root)],
        "duplicate ID: same",
    );

    for (name, uri, expected) in [
        ("outside.xml", "../../../outside.xml", "outside-root URI"),
        ("query.xml", "a.xml?q=1", "outside-root URI"),
        ("fragment.xml", "a.xml#part", "outside-root URI"),
    ] {
        let path = write_catalog(
            &root,
            name,
            &catalog(&format!(
                "<TESTCASES xml:base=\"cases/nested/\"><TEST ID=\"bad\" TYPE=\"valid\" URI=\"{uri}\" SECTIONS=\"x\"/></TESTCASES>"
            )),
        );
        assert_rejected(&helper, &[arg(&path), arg(&root)], expected);
    }

    let control = write_catalog(
        &root,
        "control.xml",
        &catalog(
            "<TESTCASES><TEST ID=\"control\" TYPE=\"valid\" URI=\"x.xml\" SECTIONS=\"x&#xA;y\"/></TESTCASES>",
        ),
    );
    assert_rejected(&helper, &[arg(&control), arg(&root)], "TSV control byte");
    let long_field = write_catalog(
        &root,
        "long-field.xml",
        &catalog(&format!(
            "<TESTCASES><TEST ID=\"long\" TYPE=\"valid\" URI=\"x.xml\" SECTIONS=\"{}\"/></TESTCASES>",
            "s".repeat(1025)
        )),
    );
    assert_rejected(
        &helper,
        &[arg(&long_field), arg(&root)],
        "exceeds 1024 bytes",
    );
    let long_base = "a".repeat(900);
    let long_uri = format!("{}.xml", "b".repeat(200));
    let long_path = write_catalog(
        &root,
        "long-path.xml",
        &catalog(&format!(
            "<TESTCASES xml:base=\"{long_base}/\"><TEST ID=\"long-path\" TYPE=\"valid\" URI=\"{long_uri}\" SECTIONS=\"x\"/></TESTCASES>"
        )),
    );
    assert_rejected(
        &helper,
        &[arg(&long_path), arg(&root)],
        "resolved path exceeds 1024 bytes",
    );

    let many_tests = format!("<TESTSUITE>{}</TESTSUITE>", "<TEST/>".repeat(10_001));
    let too_many = write_catalog(&root, "too-many.xml", &many_tests);
    assert_rejected(&helper, &[arg(&too_many), arg(&root)], "exceeds 10000");

    let outside_catalog = write_catalog(&temp.0, "outside-catalog.xml", "<TESTSUITE/>");
    assert_rejected(
        &helper,
        &[arg(&outside_catalog), arg(&root)],
        "catalog must be inside suite root",
    );
    let root_file = write_catalog(&temp.0, "not-a-directory", "root");
    assert_rejected(
        &helper,
        &[arg(&good), arg(&root_file)],
        "suite root must be a directory",
    );
}
