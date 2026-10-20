//! docs/libxml2-api.tsv: every row has a status and a `rust` mapping, and
//! every Rust path claimed by an `implemented`/`partial` row exists in src/.
//! Mirrors `tooling/api_status.py --check` (which also regenerates the file).

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

const COLUMNS: [&str; 8] = [
    "header",
    "identifier",
    "kind",
    "deprecated_marker",
    "priority",
    "status",
    "upstream_ref",
    "rust",
];
const STATUSES: [&str; 5] = [
    "implemented",
    "partial",
    "not-started",
    "not-applicable",
    "removed-upstream",
];
const SCOPE_KW: [&str; 7] = ["fn", "struct", "enum", "trait", "type", "mod", "union"];
const ANY_KW: [&str; 9] = [
    "fn", "const", "static", "struct", "enum", "trait", "type", "mod", "union",
];

struct Src {
    files: Vec<(PathBuf, String)>,
}

fn walk(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "bin")
                && path.parent() == Some(Path::new("src"))
            {
                continue;
            }
            walk(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let text = fs::read_to_string(&path).unwrap();
            out.push((path, text));
        }
    }
}

fn ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Positions of whole-word `name` in `text`.
fn words<'a>(text: &'a str, name: &'a str) -> impl Iterator<Item = usize> + 'a {
    text.match_indices(name).map(|(i, _)| i).filter(move |&i| {
        !text[..i].chars().next_back().is_some_and(ident)
            && !text[i + name.len()..].chars().next().is_some_and(ident)
    })
}

/// The identifier before `at`, skipping whitespace.
fn previous_word(text: &str, at: usize) -> &str {
    let head = text[..at].trim_end();
    let start = head.rfind(|c| !ident(c)).map_or(0, |i| i + 1);
    &head[start..]
}

impl Src {
    fn load() -> Self {
        let mut files = Vec::new();
        walk(Path::new("src"), &mut files);
        Self { files }
    }

    fn all(&self) -> BTreeSet<usize> {
        (0..self.files.len()).collect()
    }

    fn index(&self, path: &Path) -> Option<usize> {
        self.files.iter().position(|(p, _)| p == path)
    }

    /// File `f` plus the files of the module it roots.
    fn family(&self, path: &Path) -> BTreeSet<usize> {
        let dir = path.with_extension("");
        let mut out: BTreeSet<usize> = self.index(path).into_iter().collect();
        let rooted = if path.file_name().is_some_and(|n| n == "mod.rs") {
            path.parent().map(Path::to_path_buf)
        } else {
            Some(dir)
        };
        if let Some(d) = rooted {
            out.extend((0..self.files.len()).filter(|&i| self.files[i].0.starts_with(&d)));
        }
        out
    }

    /// Files a declaration of `name` in `scope` makes the next lookup scope.
    fn scope_of(&self, name: &str, scope: &BTreeSet<usize>) -> BTreeSet<usize> {
        let mut out = BTreeSet::new();
        for &f in scope {
            let (path, text) = &self.files[f];
            for at in words(text, name) {
                let kw = previous_word(text, at);
                if !SCOPE_KW.contains(&kw) {
                    continue;
                }
                let external = kw == "mod" && text[at + name.len()..].trim_start().starts_with(';');
                if kw != "mod" {
                    out.extend(self.family(path));
                } else if external {
                    let base = if path
                        .file_name()
                        .is_some_and(|n| n == "lib.rs" || n == "mod.rs")
                    {
                        path.parent().unwrap().to_path_buf()
                    } else {
                        path.with_extension("")
                    };
                    out.extend(self.family(&base.join(format!("{name}.rs"))));
                    out.extend(self.family(&base.join(name).join("mod.rs")));
                    out.extend(self.index(&base.join(format!("{name}.rs"))));
                } else {
                    out.insert(f);
                }
            }
        }
        out
    }

    fn declared(&self, name: &str, scope: &BTreeSet<usize>) -> bool {
        let variant = name.starts_with(|c: char| c.is_uppercase());
        scope.iter().any(|&f| {
            let text = &self.files[f].1;
            words(text, name).any(|at| {
                let kw = previous_word(text, at);
                let after = text[at + name.len()..].trim_start_matches([' ', '\t']);
                let line_start = text[..at].rsplit('\n').next().unwrap().trim().is_empty();
                ANY_KW.contains(&kw)
                    || (kw == "pub" && text[at + name.len()..].trim_start().starts_with(':'))
                    || (variant
                        && line_start
                        && (after.is_empty()
                            || after.starts_with(['\n', '\r', ',', '(', '{', '='])))
            })
        })
    }

    fn resolves(&self, path: &str) -> bool {
        let mut segs: Vec<&str> = path.split("::").collect();
        while segs.first().is_some_and(|s| *s == "yaxml" || *s == "crate") {
            segs.remove(0);
        }
        let Some((last, owners)) = segs.split_last() else {
            return false;
        };
        let mut scope = self.all();
        for owner in owners {
            scope = self.scope_of(owner, &scope);
            if scope.is_empty() {
                return false;
            }
        }
        self.declared(last, &scope)
    }
}

fn is_path(p: &str) -> bool {
    !p.is_empty()
        && p.split("::")
            .all(|s| s.starts_with(|c: char| c.is_alphabetic() || c == '_') && s.chars().all(ident))
}

/// Problems with one row; empty when the row is sound.
fn row_problems(src: &Src, fields: &[&str]) -> Vec<String> {
    let (id, status, rust) = (fields[1], fields[5], fields[7]);
    if !STATUSES.contains(&status) {
        return vec![format!("{id}: bad status {status:?}")];
    }
    if rust.is_empty() {
        return vec![format!("{id}: empty rust")];
    }
    if !matches!(status, "implemented" | "partial") {
        return Vec::new();
    }
    rust.split(" - ")
        .next()
        .unwrap()
        .split(',')
        .map(str::trim)
        .filter_map(|p| match (is_path(p), is_path(p) && src.resolves(p)) {
            (false, _) => Some(format!("{id}: not a path {p:?}")),
            (true, false) => Some(format!("{id}: {p} not found in src/")),
            _ => None,
        })
        .collect()
}

#[test]
fn inventory_rows_are_classified_and_paths_exist() {
    let tsv = fs::read_to_string("docs/libxml2-api.tsv").unwrap();
    let mut lines = tsv.lines();
    assert_eq!(
        lines.next().unwrap().split('\t').collect::<Vec<_>>(),
        COLUMNS
    );
    let src = Src::load();
    let (mut rows, mut problems) = (0, Vec::new());
    for line in lines {
        let fields: Vec<&str> = line.split('\t').collect();
        assert_eq!(fields.len(), COLUMNS.len(), "{line}");
        rows += 1;
        problems.extend(row_problems(&src, &fields));
    }
    assert!(rows >= 4060, "{rows} rows");
    assert!(
        problems.is_empty(),
        "{} problems:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

#[test]
fn row_check_accepts_sound_rows_and_rejects_broken_ones() {
    let src = Src::load();
    let row = |status, rust| ["h", "id", "k", "m", "P0", status, "u", rust].map(String::from);
    let cases: [(&str, &str, usize); 12] = [
        ("implemented", "OwnedDocument::parse_with", 0),
        ("implemented", "options::NOENT", 0),
        (
            "implemented",
            "ErrorKind::MismatchedEndTag, xpath::XPath::compile - note",
            0,
        ),
        ("partial", "Editor::append_element", 0),
        ("implemented", "xsd::Code::CvcElt1", 0),
        ("not-started", "any free text, no path needed", 0),
        ("implemented", "OwnedDocument::no_such_method", 1),
        ("implemented", "NoSuchType::parse", 1),
        ("partial", "options::NOENT, options::NOPE", 1),
        ("implemented", "not a path", 1),
        ("unmapped", "x", 1),
        ("partial", "", 1),
    ];
    for (status, rust, expected) in cases {
        let fields = row(status, rust);
        let refs: Vec<&str> = fields.iter().map(String::as_str).collect();
        assert_eq!(row_problems(&src, &refs).len(), expected, "{status} {rust}");
    }
}
