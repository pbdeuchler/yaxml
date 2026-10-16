#[path = "../benches/support/structural_mutation.rs"]
mod support;

use support::{AccountingError, CaseKind, MatchedRequest, OperationCounts, removal_order};

#[test]
fn case_labels_parse_and_unknown_labels_do_not() {
    for case in CaseKind::ALL {
        assert_eq!(CaseKind::parse(case.label()), Some(case));
        assert!(!case.reference_model().is_empty());
        assert_eq!(case.is_removal(), case.label().starts_with("remove_"));
    }
    assert_eq!(CaseKind::parse(""), None);
    assert_eq!(CaseKind::parse("append_element"), None);
}

#[test]
fn accounting_tracks_append_remove_and_churn_without_refunds() {
    let mut counts = OperationCounts::default();
    counts.request().unwrap();
    counts.complete_append().unwrap();
    counts.request().unwrap();
    counts.complete_append().unwrap();
    counts.request().unwrap();
    counts.complete_removal(1).unwrap();

    assert_eq!(counts.requested, 3);
    assert_eq!(counts.completed, 3);
    assert_eq!(counts.created_nodes, 2);
    assert_eq!(counts.removed_nodes, 1);
    assert_eq!(counts.final_live(4), Ok(5));
}

#[test]
fn accounting_reports_each_overflow_and_underflow_without_partial_publication() {
    let cases = [
        (
            "request_overflow",
            OperationCounts {
                requested: usize::MAX,
                ..OperationCounts::default()
            },
            "request",
            AccountingError::Overflow,
        ),
        (
            "append_completed_overflow",
            OperationCounts {
                completed: usize::MAX,
                ..OperationCounts::default()
            },
            "append",
            AccountingError::Overflow,
        ),
        (
            "append_created_overflow",
            OperationCounts {
                created_nodes: usize::MAX,
                ..OperationCounts::default()
            },
            "append",
            AccountingError::Overflow,
        ),
        (
            "remove_completed_overflow",
            OperationCounts {
                completed: usize::MAX,
                ..OperationCounts::default()
            },
            "remove",
            AccountingError::Overflow,
        ),
        (
            "remove_count_overflow",
            OperationCounts {
                removed_nodes: usize::MAX,
                ..OperationCounts::default()
            },
            "remove",
            AccountingError::Overflow,
        ),
    ];

    for (name, mut counts, operation, expected) in cases {
        let original = counts;
        let actual = match operation {
            "request" => counts.request(),
            "append" => counts.complete_append(),
            "remove" => counts.complete_removal(1),
            _ => unreachable!(),
        };
        assert_eq!(actual, Err(expected), "{name}");
        assert_eq!(counts, original, "{name} partially published accounting");
    }

    assert_eq!(
        OperationCounts {
            removed_nodes: 2,
            ..OperationCounts::default()
        }
        .final_live(1),
        Err(AccountingError::RemovedMoreThanLive)
    );
    assert_eq!(
        OperationCounts {
            created_nodes: usize::MAX,
            ..OperationCounts::default()
        }
        .final_live(1),
        Err(AccountingError::Overflow)
    );
}

#[test]
fn removal_orders_cover_first_middle_last_and_deep_without_duplicates() {
    let cases = [
        (CaseKind::RemoveFirst, vec![0, 1, 2, 3, 4]),
        (CaseKind::RemoveMiddle, vec![2, 3, 1, 4, 0]),
        (CaseKind::RemoveLast, vec![4, 3, 2, 1, 0]),
        (CaseKind::RemoveDeep, vec![2, 3, 1, 4, 0]),
    ];
    for (case, expected) in cases {
        assert_eq!(removal_order(5, case), expected, "{}", case.label());
        assert_eq!(removal_order(0, case), Vec::<usize>::new());
    }
}

#[test]
#[should_panic(expected = "removal order requires a removal case")]
fn removal_order_rejects_append_cases() {
    removal_order(1, CaseKind::AppendText);
}

#[test]
fn matched_request_preserves_binary_frames_and_all_dimensions() {
    let request = MatchedRequest {
        case: CaseKind::AppendPi,
        encoding: "utf16le",
        width: 3,
        depth: 4,
        payload: b"x\ny",
        iterations: 5,
        operations: 6,
        traversal_passes: 7,
        serialization_passes: 8,
        input: b"\xff\xfe\0<\n",
        expected: b"<r><?bench x\ny?></r>",
    };
    let mut output = Vec::new();
    request.write(&mut output).unwrap();
    assert_eq!(
        output,
        b"YAXML-STRUCTURE-BENCH-1\nappend_pi\nutf16le\n3\n4\n3\n5\n6\n7\n8\n5:\xff\xfe\0<\n\n3:x\ny\n20:<r><?bench x\ny?></r>\n"
    );
    let path = std::env::temp_dir().join(format!(
        "yaxml-structure-request-test-{}",
        std::process::id()
    ));
    request.write_file(&path).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), output);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn matched_request_propagates_output_failure() {
    struct Broken;
    impl std::io::Write for Broken {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("broken benchmark fixture sink"))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let request = MatchedRequest {
        case: CaseKind::Churn,
        encoding: "utf8",
        width: 1,
        depth: 1,
        payload: b"",
        iterations: 1,
        operations: 1,
        traversal_passes: 1,
        serialization_passes: 1,
        input: b"<r/>",
        expected: b"<r/>",
    };
    assert_eq!(
        request.write(&mut Broken).unwrap_err().kind(),
        std::io::ErrorKind::Other
    );
}
