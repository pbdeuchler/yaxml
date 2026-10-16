//! Strict metadata and result policy for the external XML conformance suite.

use std::collections::HashSet;
use std::path::{Component, Path};

pub const MAX_MANIFEST_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_CASES: usize = 10_000;
pub const MAX_FIELD_BYTES: usize = 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Case<'a> {
    pub id: &'a str,
    pub kind: &'a str,
    pub entities: &'a str,
    pub recommendation: &'a str,
    pub version: &'a str,
    pub edition: &'a str,
    pub namespace: &'a str,
    pub uri: &'a str,
    pub path: &'a str,
    pub output: &'a str,
    pub output3: &'a str,
    pub sections: &'a str,
}

/// The descriptor oracle emits paths, not arbitrary URIs, in the path column.
pub fn relative_path(value: &str) -> bool {
    !value.is_empty()
        && !value
            .chars()
            .any(|c| c.is_control() || ":\\%?#".contains(c))
        && value
            .split('/')
            .all(|part| !matches!(part, "" | "." | ".."))
        && Path::new(value)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}

fn tokens(value: &str, allowed: &[&str]) -> bool {
    value.is_empty()
        || value.split(' ').all(|token| allowed.contains(&token))
            && value.split(' ').collect::<HashSet<_>>().len() == value.split(' ').count()
}

pub fn parse(input: &[u8], expected: usize) -> Result<Vec<Case<'_>>, String> {
    if !(1..=MAX_CASES).contains(&expected) || input.len() > MAX_MANIFEST_BYTES {
        return Err("catalog count or byte ceiling exceeded".into());
    }
    let text = std::str::from_utf8(input).map_err(|_| "catalog is not UTF-8")?;
    if !text.ends_with('\n') || text.contains('\r') {
        return Err("catalog must have LF-terminated records".into());
    }
    let mut lines = text.lines();
    let header = format!("yaxml-xmlconf-v1\t{expected}");
    if lines.next() != Some(header.as_str()) {
        return Err("catalog header/count differs from requested inventory".into());
    }
    let mut cases = Vec::new();
    let mut identifiers = HashSet::new();
    for (index, line) in lines.enumerate() {
        if cases.len() >= expected {
            return Err("catalog contains extra records".into());
        }
        let fields: Vec<_> = line.split('\t').collect();
        if fields.len() != 12
            || fields
                .iter()
                .any(|value| value.len() > MAX_FIELD_BYTES || value.chars().any(char::is_control))
        {
            return Err(format!("catalog row {} has invalid fields", index + 1));
        }
        let case = Case {
            id: fields[0],
            kind: fields[1],
            entities: fields[2],
            recommendation: fields[3],
            version: fields[4],
            edition: fields[5],
            namespace: fields[6],
            uri: fields[7],
            path: fields[8],
            output: fields[9],
            output3: fields[10],
            sections: fields[11],
        };
        if case.id.is_empty()
            || !identifiers.insert(case.id)
            || !matches!(case.kind, "valid" | "invalid" | "not-wf" | "error")
            || !matches!(case.entities, "none" | "general" | "parameter" | "both")
            || !matches!(
                case.recommendation,
                "XML1.0"
                    | "XML1.1"
                    | "NS1.0"
                    | "NS1.1"
                    | "XML1.0-errata2e"
                    | "XML1.0-errata3e"
                    | "XML1.0-errata4e"
                    | "NS1.0-errata1e"
            )
            || !tokens(case.version, &["1.0", "1.1"])
            || !tokens(case.edition, &["1", "2", "3", "4", "5"])
            || !matches!(case.namespace, "yes" | "no")
            || case.uri.is_empty()
            || case.sections.is_empty()
            || !relative_path(case.path)
        {
            return Err(format!(
                "catalog row {} has invalid metadata or a duplicate ID",
                index + 1
            ));
        }
        cases.push(case);
    }
    if cases.len() != expected {
        return Err("catalog is missing records".into());
    }
    Ok(cases)
}

impl Case<'_> {
    /// The pinned 20130923 archive's master catalog has this nine-file base typo.
    /// Preserve the catalog path in every result, and expose the override separately.
    pub fn run_path(&self) -> (&str, &'static str) {
        const CORRECTIONS: [(&str, &str, &str); 9] = [
            (
                "hst-bh-001",
                "eduni/namespaces/misc/001.xml",
                "eduni/misc/001.xml",
            ),
            (
                "hst-bh-002",
                "eduni/namespaces/misc/002.xml",
                "eduni/misc/002.xml",
            ),
            (
                "hst-bh-003",
                "eduni/namespaces/misc/003.xml",
                "eduni/misc/003.xml",
            ),
            (
                "hst-bh-004",
                "eduni/namespaces/misc/004.xml",
                "eduni/misc/004.xml",
            ),
            (
                "hst-bh-005",
                "eduni/namespaces/misc/005.xml",
                "eduni/misc/005.xml",
            ),
            (
                "hst-bh-006",
                "eduni/namespaces/misc/006.xml",
                "eduni/misc/006.xml",
            ),
            (
                "hst-lhs-007",
                "eduni/namespaces/misc/007.xml",
                "eduni/misc/007.xml",
            ),
            (
                "hst-lhs-008",
                "eduni/namespaces/misc/008.xml",
                "eduni/misc/008.xml",
            ),
            (
                "hst-lhs-009",
                "eduni/namespaces/misc/009.xml",
                "eduni/misc/009.xml",
            ),
        ];
        for (id, original, corrected) in CORRECTIONS {
            if self.id == id && self.path == original {
                return (corrected, "archive-eduni-misc-base");
            }
        }
        (self.path, "none")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Verdict {
    pub status: &'static str,
    pub reason: &'static str,
}

/// Score terminal acceptance, never DTD validity, output canonicalization or offsets.
pub fn classify(case: Case<'_>, error: Option<&str>) -> Verdict {
    let (status, reason) = if error.is_some_and(|kind| {
        matches!(
            kind,
            "Allocation" | "IdentifierExhausted" | "InvalidState" | "Aborted"
        )
    }) {
        ("inconclusive", "internal-or-resource-failure")
    } else if error.is_some_and(|kind| kind.starts_with("LimitExceeded(")) {
        ("gap", "resource-ceiling")
    } else if !case.version.is_empty() && !case.version.split(' ').any(|v| v == "1.0")
        || matches!(case.recommendation, "XML1.1" | "NS1.1")
    {
        ("unscored", "unsupported-recommendation-or-version")
    } else if !case.edition.is_empty() && !case.edition.split(' ').any(|v| v == "5") {
        ("unscored", "historical-edition")
    } else if case.namespace == "no" {
        ("gap", "namespace-off-profile")
    } else if case.kind == "error" {
        ("unscored", "discretionary-error")
    } else if case.entities != "none" {
        ("gap", "external-entity-processing")
    } else if error == Some("UnsupportedDtd") {
        ("gap", "dtd-processing")
    } else if error == Some("UnsupportedEncoding") {
        ("gap", "encoding-support")
    } else if case.kind == "not-wf" {
        if error.is_some() {
            ("pass", "well-formedness-rejection")
        } else {
            ("mismatch", "unexpected-acceptance")
        }
    } else if error.is_none() {
        ("pass", "well-formedness-acceptance")
    } else {
        ("mismatch", "unexpected-rejection")
    };
    Verdict { status, reason }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROW: &str = "one\tvalid\tnone\tXML1.0\t1.0\t5\tyes\ta.xml\ta.xml\t\t\t2.1";
    fn literal() -> Case<'static> {
        Case {
            id: "one",
            kind: "valid",
            entities: "none",
            recommendation: "XML1.0",
            version: "",
            edition: "",
            namespace: "yes",
            uri: "a.xml",
            path: "a.xml",
            output: "",
            output3: "",
            sections: "2.1",
        }
    }

    #[test]
    fn catalog_inventory_fields_and_path_boundaries_are_strict() {
        let valid = format!("yaxml-xmlconf-v1\t1\n{ROW}\n");
        let cases = parse(valid.as_bytes(), 1).unwrap();
        assert_eq!(cases[0].id, "one");
        assert_eq!(cases[0].edition, "5");
        assert_eq!(cases[0].sections, "2.1");
        assert_eq!(cases[0].output, "");
        for bad in [
            valid.trim_end().to_owned(),
            valid.replace("valid", "other"),
            valid.replace("none", "unknown"),
            valid.replace("XML1.0", "OTHER"),
            valid.replace("1.0\t5", "1.0 1.0\t5"),
            valid.replace("\t5\t", "\t6\t"),
            valid.replace("yes", "maybe"),
            valid.replace("a.xml\ta.xml", "a.xml\t../a.xml"),
            valid.replace("\n", "\r\n"),
            format!("{valid}{ROW}\n"),
            "yaxml-xmlconf-v1\t1\n".into(),
            valid.replace("\t2.1", ""),
            valid.replace("one", &"n".repeat(MAX_FIELD_BYTES + 1)),
            format!("yaxml-xmlconf-v1\t2\n{ROW}\n{ROW}\n"),
        ] {
            assert!(parse(bad.as_bytes(), 1).is_err(), "{bad:?}");
        }
        assert!(parse(format!("yaxml-xmlconf-v1\t2\n{ROW}\n{ROW}\n").as_bytes(), 2).is_err());
        assert!(parse(&[0xff, b'\n'], 1).is_err());
        assert!(parse(&vec![b'x'; MAX_MANIFEST_BYTES + 1], 1).is_err());
        assert!(parse(valid.as_bytes(), 0).is_err());
        assert!(parse(valid.as_bytes(), MAX_CASES + 1).is_err());
        for path in ["a.xml", "suite/é.xml", "a-b_1.2.xml"] {
            assert!(relative_path(path));
        }
        for path in [
            "", "/a", "../a", "a/../b", "./a", "a//b", "a/", "C:/a", "a\\b", "a%20b", "a?b", "a#b",
            "a\0b",
        ] {
            assert!(!relative_path(path), "{path:?}");
        }
    }

    #[test]
    fn classification_preserves_spec_and_capability_distinctions() {
        let base = literal();
        for (case, error, expected) in [
            (base, None, ("pass", "well-formedness-acceptance")),
            (
                Case {
                    kind: "invalid",
                    ..base
                },
                None,
                ("pass", "well-formedness-acceptance"),
            ),
            (
                Case {
                    kind: "invalid",
                    ..base
                },
                Some("Syntax"),
                ("mismatch", "unexpected-rejection"),
            ),
            (base, Some("Syntax"), ("mismatch", "unexpected-rejection")),
            (
                Case {
                    kind: "not-wf",
                    ..base
                },
                Some("Syntax"),
                ("pass", "well-formedness-rejection"),
            ),
            (
                Case {
                    kind: "not-wf",
                    ..base
                },
                None,
                ("mismatch", "unexpected-acceptance"),
            ),
            (base, Some("UnsupportedDtd"), ("gap", "dtd-processing")),
            (
                base,
                Some("UnsupportedEncoding"),
                ("gap", "encoding-support"),
            ),
            (
                base,
                Some("LimitExceeded(Depth)"),
                ("gap", "resource-ceiling"),
            ),
            (
                base,
                Some("Allocation"),
                ("inconclusive", "internal-or-resource-failure"),
            ),
            (
                Case {
                    recommendation: "XML1.1",
                    ..base
                },
                None,
                ("unscored", "unsupported-recommendation-or-version"),
            ),
            (
                Case {
                    recommendation: "NS1.1",
                    ..base
                },
                None,
                ("unscored", "unsupported-recommendation-or-version"),
            ),
            (
                Case {
                    version: "1.1",
                    ..base
                },
                None,
                ("unscored", "unsupported-recommendation-or-version"),
            ),
            (
                Case {
                    edition: "1 2 3 4",
                    ..base
                },
                None,
                ("unscored", "historical-edition"),
            ),
            (
                Case {
                    namespace: "no",
                    ..base
                },
                Some("Namespace"),
                ("gap", "namespace-off-profile"),
            ),
            (
                Case {
                    kind: "error",
                    ..base
                },
                Some("Syntax"),
                ("unscored", "discretionary-error"),
            ),
            (
                Case {
                    entities: "general",
                    ..base
                },
                Some("Syntax"),
                ("gap", "external-entity-processing"),
            ),
        ] {
            assert_eq!(
                classify(case, error),
                Verdict {
                    status: expected.0,
                    reason: expected.1
                }
            );
        }
        for error in [None, Some("Syntax")] {
            for (case, status, reason) in [
                (
                    Case {
                        kind: "not-wf",
                        entities: "general",
                        ..base
                    },
                    "gap",
                    "external-entity-processing",
                ),
                (
                    Case {
                        kind: "not-wf",
                        namespace: "no",
                        ..base
                    },
                    "gap",
                    "namespace-off-profile",
                ),
                (
                    Case {
                        kind: "not-wf",
                        edition: "1 2 3 4",
                        ..base
                    },
                    "unscored",
                    "historical-edition",
                ),
                (
                    Case {
                        namespace: "no",
                        ..base
                    },
                    "gap",
                    "namespace-off-profile",
                ),
            ] {
                assert_eq!(classify(case, error), Verdict { status, reason });
            }
        }
        // Unsupported, historical and arbitrary-failure outcomes never become
        // successful negative conformance through a permissive fallback.
        for kind in ["valid", "invalid", "not-wf", "error"] {
            for error in [
                "UnsupportedDtd",
                "UnsupportedEncoding",
                "Allocation",
                "LimitExceeded(OwnedBytes)",
            ] {
                assert_ne!(classify(Case { kind, ..base }, Some(error)).status, "pass");
            }
        }
    }

    #[test]
    fn archive_path_corrections_are_exact_and_visible() {
        let base = literal();
        for (id, number) in [
            ("hst-bh-001", 1),
            ("hst-bh-002", 2),
            ("hst-bh-003", 3),
            ("hst-bh-004", 4),
            ("hst-bh-005", 5),
            ("hst-bh-006", 6),
            ("hst-lhs-007", 7),
            ("hst-lhs-008", 8),
            ("hst-lhs-009", 9),
        ] {
            let original = format!("eduni/namespaces/misc/{number:03}.xml");
            let corrected = format!("eduni/misc/{number:03}.xml");
            let case = Case {
                id,
                path: &original,
                ..base
            };
            assert_eq!(
                case.run_path(),
                (corrected.as_str(), "archive-eduni-misc-base")
            );
            assert_eq!(
                Case {
                    id: "different",
                    ..case
                }
                .run_path(),
                (original.as_str(), "none")
            );
            assert_eq!(
                Case {
                    path: "other.xml",
                    ..case
                }
                .run_path(),
                ("other.xml", "none")
            );
        }
        assert_eq!(base.run_path(), ("a.xml", "none"));
    }
}
