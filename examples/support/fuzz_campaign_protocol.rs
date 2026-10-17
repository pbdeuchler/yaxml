//! Pure policy for subprocess protocols used by the fuzz campaign.
#![forbid(unsafe_code)]

use std::fmt;

pub const LIBTEST_EXACT_V1: &str = "libtest-exact-v1";
pub const LIBTEST_EXACT_V1_TEST_NAME: &str = "reader::dtd_fuzz_tests::replay_stdin";
pub const LIBTEST_EXACT_V1_ARGS: [&str; 4] = [
    "--exact",
    LIBTEST_EXACT_V1_TEST_NAME,
    "--ignored",
    "--test-threads=1",
];

pub const MAX_LIBTEST_LF_STDOUT_BYTES: usize = 186;
pub const MAX_LIBTEST_CRLF_STDOUT_BYTES: usize = 192;
pub const MAX_LIBTEST_PARSE_BYTES: usize = 1024;
pub const MAX_FILTERED_OUT_DIGITS: usize = 10;
pub const MAX_ELAPSED_WHOLE_DIGITS: usize = 10;
pub const MAX_ELAPSED_FRACTION_DIGITS: usize = 9;
pub const MAX_ELAPSED_DECIMAL_BYTES: usize =
    MAX_ELAPSED_WHOLE_DIGITS + 1 + MAX_ELAPSED_FRACTION_DIGITS;

const BETWEEN_NUMBERS: &str = " filtered out; finished in ";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LibtestSuccess {
    pub filtered_out: u32,
    pub elapsed_decimal: String,
    pub elapsed_whole_seconds: u32,
    pub elapsed_fraction_digits: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EncodingReason {
    Bom,
    NonAsciiOrInvalidUtf8,
    AnsiEscape,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LineEndingReason {
    LoneCr,
    MixedLfCrLf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShapeReason {
    MissingLine,
    DuplicateLine,
    OutOfOrder,
    ExtraLine,
    UnexpectedBlankLine,
    Truncated,
    BadFinalTerminator,
    FrameTooLong,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TestNameReason {
    Wrong,
    Shortened,
    SubstringOnly,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FilteredDecimalReason {
    Empty,
    Sign,
    NonDecimal,
    TooManyDigits,
    LeadingZero,
    ValueOverflow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ElapsedDecimalReason {
    Empty,
    MissingWhole,
    EmptyFraction,
    MultipleDots,
    Sign,
    Exponent,
    Special,
    NonDecimal,
    LeadingZero,
    WholeTooManyDigits,
    FractionTooManyDigits,
    TokenTooLong,
    WholeValueOverflow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtocolError {
    Encoding(EncodingReason),
    LineEnding(LineEndingReason),
    Shape(ShapeReason),
    RunningCount,
    TestName(TestNameReason),
    TestStatus,
    SummaryStatus,
    SummaryCounts,
    FilteredDecimal(FilteredDecimalReason),
    ElapsedDecimal(ElapsedDecimalReason),
    Whitespace,
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid {LIBTEST_EXACT_V1} stdout: {self:?}")
    }
}

impl std::error::Error for ProtocolError {}

/// Parse one successful invocation of the frozen private libtest entry.
///
/// The wire form must use either all LF or all CRLF separators. Every other
/// byte, line, space, and blank line must match the pinned grammar exactly.
/// Filtered and elapsed whole values reject leading zeroes except for the
/// single digit `0`; the returned elapsed text preserves fractional scale.
pub fn parse_libtest_exact_v1_stdout(stdout: &[u8]) -> Result<LibtestSuccess, ProtocolError> {
    if stdout.len() > MAX_LIBTEST_PARSE_BYTES {
        return Err(ProtocolError::Shape(ShapeReason::FrameTooLong));
    }
    if stdout.starts_with(b"\xef\xbb\xbf") {
        return Err(ProtocolError::Encoding(EncodingReason::Bom));
    }
    if stdout.contains(&b'\x1b') {
        return Err(ProtocolError::Encoding(EncodingReason::AnsiEscape));
    }
    if !stdout.is_ascii() {
        return Err(ProtocolError::Encoding(
            EncodingReason::NonAsciiOrInvalidUtf8,
        ));
    }
    let normalized = normalize_line_endings(stdout)?;
    let text = std::str::from_utf8(&normalized)
        .map_err(|_| ProtocolError::Encoding(EncodingReason::NonAsciiOrInvalidUtf8))?;
    let lines: Vec<_> = text.split('\n').collect();
    let (running, test, summary) = protocol_lines(text, &lines)?;
    check_running_line(running)?;
    check_test_line(test)?;
    let success = parse_summary_line(summary)?;
    if normalized.len() > MAX_LIBTEST_LF_STDOUT_BYTES {
        return Err(ProtocolError::Shape(ShapeReason::FrameTooLong));
    }
    Ok(success)
}

fn normalize_line_endings(bytes: &[u8]) -> Result<Vec<u8>, ProtocolError> {
    let mut normalized = Vec::with_capacity(bytes.len());
    let mut index = 0;
    let mut saw_lf = false;
    let mut saw_crlf = false;
    while index < bytes.len() {
        if bytes[index] == b'\r' {
            if bytes.get(index + 1) != Some(&b'\n') {
                return Err(ProtocolError::LineEnding(LineEndingReason::LoneCr));
            }
            saw_crlf = true;
            normalized.push(b'\n');
            index += 2;
        } else {
            if bytes[index] == b'\n' {
                saw_lf = true;
            }
            normalized.push(bytes[index]);
            index += 1;
        }
    }
    if saw_lf && saw_crlf {
        return Err(ProtocolError::LineEnding(LineEndingReason::MixedLfCrLf));
    }
    Ok(normalized)
}

fn protocol_lines<'a>(
    text: &str,
    lines: &'a [&'a str],
) -> Result<(&'a str, &'a str, &'a str), ProtocolError> {
    let running_positions: Vec<_> = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| line.starts_with("running").then_some(index))
        .collect();
    let test_positions: Vec<_> = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            (line.starts_with("test ") && !line.starts_with("test result:")).then_some(index)
        })
        .collect();
    let summary_positions: Vec<_> = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            line.trim_start_matches([' ', '\t'])
                .starts_with("test result:")
                .then_some(index)
        })
        .collect();
    if running_positions.len() > 1 || test_positions.len() > 1 || summary_positions.len() > 1 {
        return Err(ProtocolError::Shape(ShapeReason::DuplicateLine));
    }
    let recognized = |index| {
        running_positions.contains(&index)
            || test_positions.contains(&index)
            || summary_positions.contains(&index)
    };
    if lines
        .iter()
        .enumerate()
        .any(|(index, line)| !line.is_empty() && !recognized(index))
    {
        return Err(ProtocolError::Shape(ShapeReason::ExtraLine));
    }
    let Some(&running) = running_positions.first() else {
        return Err(ProtocolError::Shape(ShapeReason::MissingLine));
    };
    let Some(&test) = test_positions.first() else {
        return Err(ProtocolError::Shape(ShapeReason::MissingLine));
    };
    let Some(&summary) = summary_positions.first() else {
        return Err(ProtocolError::Shape(ShapeReason::MissingLine));
    };
    if !(running < test && test < summary) {
        return Err(ProtocolError::Shape(ShapeReason::OutOfOrder));
    }
    if text.ends_with("\n\n\n") {
        return Err(ProtocolError::Shape(ShapeReason::UnexpectedBlankLine));
    }
    if !text.ends_with("\n\n") {
        return if lines[summary].ends_with('s')
            && lines[summary].contains(" filtered out; finished in ")
        {
            Err(ProtocolError::Shape(ShapeReason::BadFinalTerminator))
        } else {
            Err(ProtocolError::Shape(ShapeReason::Truncated))
        };
    }
    if (running, test, summary) != (1, 2, 4) || lines.len() != 7 {
        let blank_count = lines.iter().filter(|line| line.is_empty()).count();
        return Err(ProtocolError::Shape(if blank_count > 4 {
            ShapeReason::UnexpectedBlankLine
        } else {
            ShapeReason::MissingLine
        }));
    }
    Ok((lines[running], lines[test], lines[summary]))
}

fn check_running_line(line: &str) -> Result<(), ProtocolError> {
    if line.contains('\t') || line.contains("  ") || line.ends_with(' ') {
        return Err(ProtocolError::Whitespace);
    }
    if line != "running 1 test" {
        return Err(ProtocolError::RunningCount);
    }
    Ok(())
}

fn check_test_line(line: &str) -> Result<(), ProtocolError> {
    if line.contains('\t') || line.contains("  ") || line.ends_with(' ') {
        return Err(ProtocolError::Whitespace);
    }
    let Some(body) = line.strip_prefix("test ") else {
        return Err(ProtocolError::TestStatus);
    };
    let Some((name, status)) = body.split_once(" ... ") else {
        return Err(ProtocolError::TestStatus);
    };
    if name != LIBTEST_EXACT_V1_TEST_NAME {
        let reason = if LIBTEST_EXACT_V1_TEST_NAME.starts_with(name) {
            TestNameReason::Shortened
        } else if name.contains(LIBTEST_EXACT_V1_TEST_NAME) {
            TestNameReason::SubstringOnly
        } else {
            TestNameReason::Wrong
        };
        return Err(ProtocolError::TestName(reason));
    }
    if status != "ok" {
        return Err(ProtocolError::TestStatus);
    }
    Ok(())
}

fn parse_summary_line(line: &str) -> Result<LibtestSuccess, ProtocolError> {
    check_summary_whitespace(line)?;
    let Some(body) = line.strip_prefix("test result: ") else {
        return Err(ProtocolError::Whitespace);
    };
    let Some(body) = body.strip_prefix("ok. ") else {
        return Err(ProtocolError::SummaryStatus);
    };
    let Some((passed, body)) = body.split_once("; ") else {
        return Err(ProtocolError::Shape(ShapeReason::Truncated));
    };
    let Some((failed, body)) = body.split_once("; ") else {
        return Err(ProtocolError::Shape(ShapeReason::Truncated));
    };
    let Some((ignored, body)) = body.split_once("; ") else {
        return Err(ProtocolError::Shape(ShapeReason::Truncated));
    };
    let Some((measured, body)) = body.split_once("; ") else {
        return Err(ProtocolError::Shape(ShapeReason::Truncated));
    };
    if (passed, failed, ignored, measured) != ("1 passed", "0 failed", "0 ignored", "0 measured") {
        return Err(ProtocolError::SummaryCounts);
    }
    let Some((filtered, elapsed)) = body.split_once(BETWEEN_NUMBERS) else {
        return Err(ProtocolError::Shape(ShapeReason::Truncated));
    };
    let Some(elapsed) = elapsed.strip_suffix('s') else {
        return Err(ProtocolError::Shape(ShapeReason::Truncated));
    };
    let filtered_out = parse_filtered(filtered).map_err(ProtocolError::FilteredDecimal)?;
    let (elapsed_whole_seconds, elapsed_fraction_digits) =
        parse_elapsed(elapsed).map_err(ProtocolError::ElapsedDecimal)?;
    Ok(LibtestSuccess {
        filtered_out,
        elapsed_decimal: elapsed.to_owned(),
        elapsed_whole_seconds,
        elapsed_fraction_digits: elapsed_fraction_digits.map(str::to_owned),
    })
}

fn check_summary_whitespace(line: &str) -> Result<(), ProtocolError> {
    if line.contains('\t') {
        return Err(ProtocolError::Whitespace);
    }

    const COMPACT_PREFIX: &str = "testresult:ok.1passed;0failed;0ignored;0measured;";
    const COMPACT_MIDDLE: &str = "filteredout;finishedin";
    let compact: String = line.chars().filter(|character| *character != ' ').collect();
    let Some(body) = compact.strip_prefix(COMPACT_PREFIX) else {
        return Ok(());
    };
    let Some((filtered, elapsed)) = body.split_once(COMPACT_MIDDLE) else {
        return Ok(());
    };
    let Some(elapsed) = elapsed.strip_suffix('s') else {
        return Ok(());
    };
    let canonical = format!(
        "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; {filtered} filtered out; finished in {elapsed}s"
    );
    if line != canonical {
        return Err(ProtocolError::Whitespace);
    }
    Ok(())
}

fn parse_elapsed(text: &str) -> Result<(u32, Option<&str>), ElapsedDecimalReason> {
    if text.is_empty() {
        return Err(ElapsedDecimalReason::Empty);
    }
    if text.starts_with(['-', '+']) {
        return Err(ElapsedDecimalReason::Sign);
    }
    if matches!(text, "NaN" | "Inf" | "infinity") {
        return Err(ElapsedDecimalReason::Special);
    }
    if text.contains(['e', 'E']) {
        return Err(ElapsedDecimalReason::Exponent);
    }
    if text.matches('.').count() > 1 {
        return Err(ElapsedDecimalReason::MultipleDots);
    }
    let (whole, fraction) = match text.split_once('.') {
        Some(("", _)) => return Err(ElapsedDecimalReason::MissingWhole),
        Some((_, "")) => return Err(ElapsedDecimalReason::EmptyFraction),
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (text, None),
    };
    if !whole.bytes().all(|byte| byte.is_ascii_digit())
        || fraction.is_some_and(|digits| !digits.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err(ElapsedDecimalReason::NonDecimal);
    }
    if text.len() > MAX_ELAPSED_DECIMAL_BYTES {
        return Err(ElapsedDecimalReason::TokenTooLong);
    }
    if whole.len() > MAX_ELAPSED_WHOLE_DIGITS {
        return Err(ElapsedDecimalReason::WholeTooManyDigits);
    }
    if fraction.is_some_and(|digits| digits.len() > MAX_ELAPSED_FRACTION_DIGITS) {
        return Err(ElapsedDecimalReason::FractionTooManyDigits);
    }
    if whole.len() > 1 && whole.starts_with('0') {
        return Err(ElapsedDecimalReason::LeadingZero);
    }
    let whole_seconds = checked_u32(whole).ok_or(ElapsedDecimalReason::WholeValueOverflow)?;
    Ok((whole_seconds, fraction))
}

fn parse_filtered(text: &str) -> Result<u32, FilteredDecimalReason> {
    if text.is_empty() {
        return Err(FilteredDecimalReason::Empty);
    }
    if text.starts_with(['-', '+']) {
        return Err(FilteredDecimalReason::Sign);
    }
    if !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(FilteredDecimalReason::NonDecimal);
    }
    if text.len() > MAX_FILTERED_OUT_DIGITS {
        return Err(FilteredDecimalReason::TooManyDigits);
    }
    if text.len() > 1 && text.starts_with('0') {
        return Err(FilteredDecimalReason::LeadingZero);
    }
    checked_u32(text).ok_or(FilteredDecimalReason::ValueOverflow)
}

fn checked_u32(text: &str) -> Option<u32> {
    text.bytes().try_fold(0_u32, |value, byte| {
        value
            .checked_mul(10)
            .and_then(|value| value.checked_add(u32::from(byte - b'0')))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_independent_literal_success_streams() {
        struct Case {
            name: &'static str,
            stdout: &'static [u8],
            expected: LibtestSuccess,
        }
        let cases = [
            Case {
                name: "canonical_zero",
                stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n\n",
                expected: LibtestSuccess {
                    filtered_out: 0,
                    elapsed_decimal: "0.00".to_owned(),
                    elapsed_whole_seconds: 0,
                    elapsed_fraction_digits: Some("00".to_owned()),
                },
            },
            Case {
                name: "variable_count_and_time",
                stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1234 filtered out; finished in 1.23s\n\n",
                expected: LibtestSuccess {
                    filtered_out: 1234,
                    elapsed_decimal: "1.23".to_owned(),
                    elapsed_whole_seconds: 1,
                    elapsed_fraction_digits: Some("23".to_owned()),
                },
            },
            Case {
                name: "integer_time",
                stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 12s\n\n",
                expected: LibtestSuccess {
                    filtered_out: 7,
                    elapsed_decimal: "12".to_owned(),
                    elapsed_whole_seconds: 12,
                    elapsed_fraction_digits: None,
                },
            },
            Case {
                name: "maximum_elapsed_components",
                stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 4294967295 filtered out; finished in 4294967295.999999999s\n\n",
                expected: LibtestSuccess {
                    filtered_out: u32::MAX,
                    elapsed_decimal: "4294967295.999999999".to_owned(),
                    elapsed_whole_seconds: u32::MAX,
                    elapsed_fraction_digits: Some("999999999".to_owned()),
                },
            },
            Case {
                name: "maximum_crlf_frame",
                stdout: b"\r\nrunning 1 test\r\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\r\n\r\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 4294967295 filtered out; finished in 4294967295.999999999s\r\n\r\n",
                expected: LibtestSuccess {
                    filtered_out: u32::MAX,
                    elapsed_decimal: "4294967295.999999999".to_owned(),
                    elapsed_whole_seconds: u32::MAX,
                    elapsed_fraction_digits: Some("999999999".to_owned()),
                },
            },
            Case {
                name: "fraction_preserves_leading_zeroes",
                stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 802 filtered out; finished in 0.001s\n\n",
                expected: LibtestSuccess {
                    filtered_out: 802,
                    elapsed_decimal: "0.001".to_owned(),
                    elapsed_whole_seconds: 0,
                    elapsed_fraction_digits: Some("001".to_owned()),
                },
            },
            Case {
                name: "complete_crlf_form",
                stdout: b"\r\nrunning 1 test\r\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\r\n\r\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 42 filtered out; finished in 3.4s\r\n\r\n",
                expected: LibtestSuccess {
                    filtered_out: 42,
                    elapsed_decimal: "3.4".to_owned(),
                    elapsed_whole_seconds: 3,
                    elapsed_fraction_digits: Some("4".to_owned()),
                },
            },
            Case {
                name: "nine_fraction_digits",
                stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.000000000s\n\n",
                expected: LibtestSuccess {
                    filtered_out: 1,
                    elapsed_decimal: "0.000000000".to_owned(),
                    elapsed_whole_seconds: 0,
                    elapsed_fraction_digits: Some("000000000".to_owned()),
                },
            },
            Case {
                name: "observed_pinned_run",
                stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 802 filtered out; finished in 0.00s\n\n",
                expected: LibtestSuccess {
                    filtered_out: 802,
                    elapsed_decimal: "0.00".to_owned(),
                    elapsed_whole_seconds: 0,
                    elapsed_fraction_digits: Some("00".to_owned()),
                },
            },
        ];

        for case in &cases {
            assert_eq!(
                parse_libtest_exact_v1_stdout(case.stdout),
                Ok(case.expected.clone()),
                "{}",
                case.name
            );
        }
        assert_eq!(cases[3].stdout.len(), MAX_LIBTEST_LF_STDOUT_BYTES);
        assert_eq!(cases[4].stdout.len(), MAX_LIBTEST_CRLF_STDOUT_BYTES);
    }

    #[test]
    fn rejects_wrong_names_counts_status_order_and_extra_lines() {
        struct Case {
            name: &'static str,
            stdout: &'static [u8],
            expected: ProtocolError,
        }
        let cases = [
            Case { name: "wrong_name", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::wrong ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\n\n", expected: ProtocolError::TestName(TestNameReason::Wrong) },
            Case { name: "short_name", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\n\n", expected: ProtocolError::TestName(TestNameReason::Shortened) },
            Case { name: "name_as_substring", stdout: b"\nrunning 1 test\ntest prefix_reader::dtd_fuzz_tests::replay_stdin_suffix ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\n\n", expected: ProtocolError::TestName(TestNameReason::SubstringOnly) },
            Case { name: "duplicate_test_line", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\n\n", expected: ProtocolError::Shape(ShapeReason::DuplicateLine) },
            Case { name: "missing_test_line", stdout: b"\nrunning 1 test\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\n\n", expected: ProtocolError::Shape(ShapeReason::MissingLine) },
            Case { name: "running_zero", stdout: b"\nrunning 0 tests\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\n\n", expected: ProtocolError::RunningCount },
            Case { name: "running_two", stdout: b"\nrunning 2 tests\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\n\n", expected: ProtocolError::RunningCount },
            Case { name: "ignored_status", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ignored\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\n\n", expected: ProtocolError::TestStatus },
            Case { name: "skipped_status", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... skipped\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\n\n", expected: ProtocolError::TestStatus },
            Case { name: "failed_test_status", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... FAILED\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\n\n", expected: ProtocolError::TestStatus },
            Case { name: "failed_summary_status", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: FAILED. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\n\n", expected: ProtocolError::SummaryStatus },
            Case { name: "zero_passed", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\n\n", expected: ProtocolError::SummaryCounts },
            Case { name: "failed_count", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 1 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\n\n", expected: ProtocolError::SummaryCounts },
            Case { name: "ignored_count", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 1 ignored; 0 measured; 7 filtered out; finished in 1.23s\n\n", expected: ProtocolError::SummaryCounts },
            Case { name: "measured_count", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 1 measured; 7 filtered out; finished in 1.23s\n\n", expected: ProtocolError::SummaryCounts },
            Case { name: "duplicate_running", stdout: b"\nrunning 1 test\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\n\n", expected: ProtocolError::Shape(ShapeReason::DuplicateLine) },
            Case { name: "duplicate_summary", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\n\n", expected: ProtocolError::Shape(ShapeReason::DuplicateLine) },
            Case { name: "test_before_running", stdout: b"\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\nrunning 1 test\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\n\n", expected: ProtocolError::Shape(ShapeReason::OutOfOrder) },
            Case { name: "summary_before_test", stdout: b"\nrunning 1 test\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\n", expected: ProtocolError::Shape(ShapeReason::OutOfOrder) },
            Case { name: "missing_running", stdout: b"\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\n\n", expected: ProtocolError::Shape(ShapeReason::MissingLine) },
            Case { name: "missing_summary", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\n", expected: ProtocolError::Shape(ShapeReason::MissingLine) },
            Case { name: "truncated_after_filtered_digits", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7", expected: ProtocolError::Shape(ShapeReason::Truncated) },
            Case { name: "truncated_filtered_suffix", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered", expected: ProtocolError::Shape(ShapeReason::Truncated) },
            Case { name: "truncated_seconds_suffix", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23", expected: ProtocolError::Shape(ShapeReason::Truncated) },
            Case { name: "one_final_lf", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\n", expected: ProtocolError::Shape(ShapeReason::BadFinalTerminator) },
            Case { name: "no_final_lf", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s", expected: ProtocolError::Shape(ShapeReason::BadFinalTerminator) },
            Case { name: "three_final_lfs", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\n\n\n", expected: ProtocolError::Shape(ShapeReason::UnexpectedBlankLine) },
            Case { name: "line_before", stdout: b"diagnostic\n\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\n\n", expected: ProtocolError::Shape(ShapeReason::ExtraLine) },
            Case { name: "line_between_running_and_test", stdout: b"\nrunning 1 test\ndiagnostic\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\n\n", expected: ProtocolError::Shape(ShapeReason::ExtraLine) },
            Case { name: "captured_output", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\ncaptured test output\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\n\n", expected: ProtocolError::Shape(ShapeReason::ExtraLine) },
            Case { name: "line_after", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\n\nwarning: fixture\n", expected: ProtocolError::Shape(ShapeReason::ExtraLine) },
            Case { name: "unexpected_blank_before_running", stdout: b"\n\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\n\n", expected: ProtocolError::Shape(ShapeReason::UnexpectedBlankLine) },
            Case { name: "unexpected_blank_between", stdout: b"\nrunning 1 test\n\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\n\n", expected: ProtocolError::Shape(ShapeReason::UnexpectedBlankLine) },
            Case { name: "ansi", stdout: b"\n\x1b[32mrunning 1 test\x1b[0m\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\n\n", expected: ProtocolError::Encoding(EncodingReason::AnsiEscape) },
            Case { name: "bom", stdout: b"\xef\xbb\xbf\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\n\n", expected: ProtocolError::Encoding(EncodingReason::Bom) },
            Case { name: "extra_space", stdout: b"\nrunning  1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\n\n", expected: ProtocolError::Whitespace },
            Case { name: "tab", stdout: b"\nrunning\t1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\n\n", expected: ProtocolError::Whitespace },
            Case { name: "trailing_test_space", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok \n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\n\n", expected: ProtocolError::Whitespace },
        ];

        for case in cases {
            assert_eq!(
                parse_libtest_exact_v1_stdout(case.stdout),
                Err(case.expected),
                "{}",
                case.name
            );
        }
    }

    #[test]
    fn rejects_summary_whitespace_by_category() {
        struct Case {
            name: &'static str,
            stdout: &'static [u8],
        }
        let cases = [
            Case { name: "leading_summary_space", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\n test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\n\n" },
            Case { name: "double_space_after_status", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok.  1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\n\n" },
            Case { name: "double_space_inside_fixed_count", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1  passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\n\n" },
            Case { name: "trailing_summary_space", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s \n\n" },
            Case { name: "summary_tab", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed;\t0 ignored; 0 measured; 7 filtered out; finished in 1.23s\n\n" },
        ];

        for case in cases {
            assert_eq!(
                parse_libtest_exact_v1_stdout(case.stdout),
                Err(ProtocolError::Whitespace),
                "{}",
                case.name
            );
        }
    }

    #[test]
    fn rejects_malformed_filtered_and_elapsed_decimals_by_category() {
        struct Case {
            name: &'static str,
            stdout: &'static [u8],
            expected: ProtocolError,
        }
        let cases = [
            Case { name: "filtered_empty", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured;  filtered out; finished in 1.23s\n\n", expected: ProtocolError::FilteredDecimal(FilteredDecimalReason::Empty) },
            Case { name: "filtered_negative", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; -1 filtered out; finished in 1.23s\n\n", expected: ProtocolError::FilteredDecimal(FilteredDecimalReason::Sign) },
            Case { name: "filtered_plus", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; +1 filtered out; finished in 1.23s\n\n", expected: ProtocolError::FilteredDecimal(FilteredDecimalReason::Sign) },
            Case { name: "filtered_exponent", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1e2 filtered out; finished in 1.23s\n\n", expected: ProtocolError::FilteredDecimal(FilteredDecimalReason::NonDecimal) },
            Case { name: "filtered_hex", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0x10 filtered out; finished in 1.23s\n\n", expected: ProtocolError::FilteredDecimal(FilteredDecimalReason::NonDecimal) },
            Case { name: "filtered_too_many_digits", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 10000000000 filtered out; finished in 1.23s\n\n", expected: ProtocolError::FilteredDecimal(FilteredDecimalReason::TooManyDigits) },
            Case { name: "filtered_overflow", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 4294967296 filtered out; finished in 1.23s\n\n", expected: ProtocolError::FilteredDecimal(FilteredDecimalReason::ValueOverflow) },
            Case { name: "filtered_zero_zero", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 00 filtered out; finished in 1.23s\n\n", expected: ProtocolError::FilteredDecimal(FilteredDecimalReason::LeadingZero) },
            Case { name: "filtered_leading_zero", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0123 filtered out; finished in 1.23s\n\n", expected: ProtocolError::FilteredDecimal(FilteredDecimalReason::LeadingZero) },
            Case { name: "elapsed_empty", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in s\n\n", expected: ProtocolError::ElapsedDecimal(ElapsedDecimalReason::Empty) },
            Case { name: "elapsed_missing_whole", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in .1s\n\n", expected: ProtocolError::ElapsedDecimal(ElapsedDecimalReason::MissingWhole) },
            Case { name: "elapsed_empty_fraction", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.s\n\n", expected: ProtocolError::ElapsedDecimal(ElapsedDecimalReason::EmptyFraction) },
            Case { name: "elapsed_two_dots", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.2.3s\n\n", expected: ProtocolError::ElapsedDecimal(ElapsedDecimalReason::MultipleDots) },
            Case { name: "elapsed_negative", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in -1.0s\n\n", expected: ProtocolError::ElapsedDecimal(ElapsedDecimalReason::Sign) },
            Case { name: "elapsed_plus", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in +1.0s\n\n", expected: ProtocolError::ElapsedDecimal(ElapsedDecimalReason::Sign) },
            Case { name: "elapsed_lower_exponent", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1e2s\n\n", expected: ProtocolError::ElapsedDecimal(ElapsedDecimalReason::Exponent) },
            Case { name: "elapsed_upper_exponent", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1E2s\n\n", expected: ProtocolError::ElapsedDecimal(ElapsedDecimalReason::Exponent) },
            Case { name: "elapsed_nan", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in NaNs\n\n", expected: ProtocolError::ElapsedDecimal(ElapsedDecimalReason::Special) },
            Case { name: "elapsed_inf", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in Infs\n\n", expected: ProtocolError::ElapsedDecimal(ElapsedDecimalReason::Special) },
            Case { name: "elapsed_infinity", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in infinitys\n\n", expected: ProtocolError::ElapsedDecimal(ElapsedDecimalReason::Special) },
            Case { name: "elapsed_nondecimal", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1_000s\n\n", expected: ProtocolError::ElapsedDecimal(ElapsedDecimalReason::NonDecimal) },
            Case { name: "elapsed_whole_too_many_digits", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 10000000000s\n\n", expected: ProtocolError::ElapsedDecimal(ElapsedDecimalReason::WholeTooManyDigits) },
            Case { name: "elapsed_fraction_too_many_digits", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.1234567890s\n\n", expected: ProtocolError::ElapsedDecimal(ElapsedDecimalReason::FractionTooManyDigits) },
            Case { name: "elapsed_total_too_long", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 4294967295.1234567890s\n\n", expected: ProtocolError::ElapsedDecimal(ElapsedDecimalReason::TokenTooLong) },
            Case { name: "elapsed_whole_overflow", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 4294967296.0s\n\n", expected: ProtocolError::ElapsedDecimal(ElapsedDecimalReason::WholeValueOverflow) },
            Case { name: "elapsed_zero_zero", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 00s\n\n", expected: ProtocolError::ElapsedDecimal(ElapsedDecimalReason::LeadingZero) },
            Case { name: "elapsed_leading_zero", stdout: b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 01.2s\n\n", expected: ProtocolError::ElapsedDecimal(ElapsedDecimalReason::LeadingZero) },
        ];

        for case in cases {
            assert_eq!(
                parse_libtest_exact_v1_stdout(case.stdout),
                Err(case.expected),
                "{}",
                case.name
            );
        }
    }

    #[test]
    fn rejects_encoding_line_ending_and_stream_bounds() {
        assert_eq!(
            parse_libtest_exact_v1_stdout(
                b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_\xffstdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\n\n"
            ),
            Err(ProtocolError::Encoding(
                EncodingReason::NonAsciiOrInvalidUtf8
            ))
        );

        for lone_carriage_return in [
            b"\rrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\n\n".as_slice(),
            b"\nrunning 1 test\rtest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\n\n".as_slice(),
            b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\n\r".as_slice(),
        ] {
            assert_eq!(
                parse_libtest_exact_v1_stdout(lone_carriage_return),
                Err(ProtocolError::LineEnding(LineEndingReason::LoneCr))
            );
        }

        assert_eq!(
            parse_libtest_exact_v1_stdout(
                b"\r\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\n\n"
            ),
            Err(ProtocolError::LineEnding(
                LineEndingReason::MixedLfCrLf
            ))
        );

        assert_eq!(
            parse_libtest_exact_v1_stdout(&vec![b'x'; MAX_LIBTEST_PARSE_BYTES + 1]),
            Err(ProtocolError::Shape(ShapeReason::FrameTooLong))
        );
    }

    #[test]
    fn changing_each_fixed_byte_rejects_the_stream() {
        const BASELINE: &[u8] = b"\nrunning 1 test\ntest reader::dtd_fuzz_tests::replay_stdin ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out; finished in 1.23s\n\n";
        const FIXED_BYTE_MASK: &[u8] = b"FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFNFFFFFFFFFFFFFFFFFFFFFFFFFFFNNNNFFF";
        assert_eq!(BASELINE.len(), FIXED_BYTE_MASK.len());

        for (index, marker) in FIXED_BYTE_MASK.iter().copied().enumerate() {
            if marker == b'N' {
                continue;
            }
            let mut changed = BASELINE.to_vec();
            changed[index] = if changed[index] == b'X' { b'Y' } else { b'X' };
            assert!(
                parse_libtest_exact_v1_stdout(&changed).is_err(),
                "fixed byte {index} was not enforced"
            );
        }
    }

    #[test]
    fn freezes_protocol_identity_and_exact_arguments() {
        assert_eq!(LIBTEST_EXACT_V1, "libtest-exact-v1");
        assert_eq!(
            LIBTEST_EXACT_V1_TEST_NAME,
            "reader::dtd_fuzz_tests::replay_stdin"
        );
        assert_eq!(
            LIBTEST_EXACT_V1_ARGS,
            [
                "--exact",
                "reader::dtd_fuzz_tests::replay_stdin",
                "--ignored",
                "--test-threads=1",
            ]
        );
    }
}
