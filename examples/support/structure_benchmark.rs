use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;

pub const PROTOCOL: &str = "YAXML-STRUCTURE-BENCH-1";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    pub helper: PathBuf,
    pub yaxml_helper: PathBuf,
    pub samples: usize,
    pub case: String,
    pub encoding: String,
    pub width: usize,
    pub depth: usize,
    pub payload_bytes: usize,
    pub iterations: usize,
    pub operations: usize,
    pub traversal_passes: usize,
    pub serialization_passes: usize,
}

#[derive(Debug, Eq, PartialEq)]
pub enum Error {
    Argument(&'static str),
    Row(&'static str),
    Mismatch(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (kind, message) = match self {
            Self::Argument(message) => ("argument", message),
            Self::Row(message) => ("row", message),
            Self::Mismatch(message) => ("comparison", message),
        };
        write!(output, "matched benchmark {kind} error: {message}")
    }
}

impl std::error::Error for Error {}

fn number(value: OsString, message: &'static str) -> Result<usize, Error> {
    value
        .into_string()
        .map_err(|_| Error::Argument(message))?
        .parse()
        .map_err(|_| Error::Argument(message))
}

pub fn parse_args(arguments: impl IntoIterator<Item = OsString>) -> Result<Config, Error> {
    let mut values = arguments.into_iter();
    let mut helper = None;
    let mut yaxml_helper = None;
    let mut samples = 7;
    let mut case = None;
    let mut encoding = None;
    let mut width = None;
    let mut depth = None;
    let mut payload_bytes = None;
    let mut iterations = 5;
    let mut operations = 1_000;
    let mut traversal_passes = 16;
    let mut serialization_passes = 4;
    while let Some(argument) = values.next() {
        let argument = argument
            .into_string()
            .map_err(|_| Error::Argument("arguments must be UTF-8"))?;
        let value = values
            .next()
            .ok_or(Error::Argument("option requires a value"))?;
        match argument.as_str() {
            "--libxml2-helper" => helper = Some(PathBuf::from(value)),
            "--yaxml-helper" => yaxml_helper = Some(PathBuf::from(value)),
            "--samples" => samples = number(value, "samples must be an integer")?,
            "--case" => {
                case = Some(
                    value
                        .into_string()
                        .map_err(|_| Error::Argument("case must be UTF-8"))?,
                );
            }
            "--encoding" => {
                encoding = Some(
                    value
                        .into_string()
                        .map_err(|_| Error::Argument("encoding must be UTF-8"))?,
                );
            }
            "--width" => width = Some(number(value, "width must be an integer")?),
            "--depth" => depth = Some(number(value, "depth must be an integer")?),
            "--payload-bytes" => {
                payload_bytes = Some(number(value, "payload bytes must be an integer")?);
            }
            "--iterations" => iterations = number(value, "iterations must be an integer")?,
            "--operations" => operations = number(value, "operations must be an integer")?,
            "--traversal-passes" => {
                traversal_passes = number(value, "traversal passes must be an integer")?;
            }
            "--serialization-passes" => {
                serialization_passes = number(value, "serialization passes must be an integer")?;
            }
            _ => return Err(Error::Argument("unknown option")),
        }
    }
    let config = Config {
        helper: helper.ok_or(Error::Argument("--libxml2-helper is required"))?,
        yaxml_helper: yaxml_helper.ok_or(Error::Argument("--yaxml-helper is required"))?,
        samples,
        case: case.ok_or(Error::Argument("--case is required"))?,
        encoding: encoding.ok_or(Error::Argument("--encoding is required"))?,
        width: width.ok_or(Error::Argument("--width is required"))?,
        depth: depth.ok_or(Error::Argument("--depth is required"))?,
        payload_bytes: payload_bytes.ok_or(Error::Argument("--payload-bytes is required"))?,
        iterations,
        operations,
        traversal_passes,
        serialization_passes,
    };
    if config.samples == 0
        || config.width == 0
        || config.depth == 0
        || config.iterations == 0
        || config.operations == 0
        || config.traversal_passes == 0
        || config.serialization_passes == 0
    {
        return Err(Error::Argument(
            "counts except payload bytes must be positive",
        ));
    }
    if ![
        "append_text",
        "append_cdata",
        "append_comment",
        "append_pi",
        "remove_first",
        "remove_middle",
        "remove_last",
        "remove_deep",
        "churn",
    ]
    .contains(&config.case.as_str())
    {
        return Err(Error::Argument("unknown case"));
    }
    if !["utf8", "utf16le", "utf16be"].contains(&config.encoding.as_str()) {
        return Err(Error::Argument("unknown encoding"));
    }
    if config.case == "append_pi" && config.payload_bytes == 0 {
        return Err(Error::Argument(
            "empty PI payload has an intentional libxml2 lexical difference",
        ));
    }
    Ok(config)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Row {
    pub text: String,
    fields: BTreeMap<String, String>,
}

impl Row {
    pub fn parse(output: &[u8]) -> Result<Self, Error> {
        let output = std::str::from_utf8(output).map_err(|_| Error::Row("output is not UTF-8"))?;
        let mut lines = output.lines().filter(|line| !line.is_empty());
        let text = lines.next().ok_or(Error::Row("missing result row"))?;
        if lines.next().is_some() {
            return Err(Error::Row("multiple result rows"));
        }
        let mut fields = BTreeMap::new();
        for field in text.split_whitespace() {
            let (name, value) = field
                .split_once('=')
                .ok_or(Error::Row("field is not key=value"))?;
            if name.is_empty()
                || value.is_empty()
                || fields.insert(name.into(), value.into()).is_some()
            {
                return Err(Error::Row("empty or duplicate field"));
            }
        }
        let row = Self {
            text: text.into(),
            fields,
        };
        for name in [
            "benchmark",
            "schema",
            "implementation",
            "case",
            "encoding",
            "status",
            "output_hash",
            "edit_ns",
        ] {
            row.get(name)?;
        }
        if row.get("benchmark")? != "owned_structure" || row.get("schema")? != "2" {
            return Err(Error::Row("unexpected benchmark schema"));
        }
        if row.get("status")? != "complete" {
            return Err(Error::Row("benchmark row is incomplete"));
        }
        Ok(row)
    }

    pub fn get(&self, name: &str) -> Result<&str, Error> {
        self.fields
            .get(name)
            .map(String::as_str)
            .ok_or(Error::Row("required field is missing"))
    }

    fn usize(&self, name: &str) -> Result<usize, Error> {
        self.get(name)?
            .parse()
            .map_err(|_| Error::Row("numeric field is invalid"))
    }
}

pub struct PairSummary {
    pub text_live_node_delta_per_iteration: usize,
}

fn multiplied(left: usize, right: usize) -> Result<usize, Error> {
    left.checked_mul(right)
        .ok_or(Error::Mismatch("expected accounting overflow"))
}

fn added(left: usize, right: usize) -> Result<usize, Error> {
    left.checked_add(right)
        .ok_or(Error::Mismatch("expected accounting overflow"))
}

pub fn validate_config(config: &Config, row: &Row) -> Result<(), Error> {
    for (name, expected) in [
        ("case", config.case.as_str()),
        ("encoding", config.encoding.as_str()),
    ] {
        if row.get(name)? != expected {
            return Err(Error::Mismatch(
                "row does not match requested configuration",
            ));
        }
    }
    for (name, expected) in [
        ("width", config.width),
        ("depth", config.depth),
        ("payload_bytes", config.payload_bytes),
        ("iterations", config.iterations),
        ("configured_units_per_iteration", config.operations),
        ("traversal_passes", config.traversal_passes),
        ("serialization_passes", config.serialization_passes),
    ] {
        if row.usize(name)? != expected {
            return Err(Error::Mismatch(
                "row does not match requested configuration",
            ));
        }
    }
    for name in [
        "setup_ns",
        "edit_ns",
        "traversal_ns",
        "serialization_ns",
        "validation_ns",
        "drop_ns",
        "counted_output_bytes_total",
        "validation_output_bytes_total",
        "sink_writes_total",
    ] {
        row.usize(name)?;
    }
    let edit_rate = row
        .get("edit_calls_per_s")?
        .parse::<f64>()
        .map_err(|_| Error::Row("edit rate is invalid"))?;
    if !edit_rate.is_finite() || edit_rate < 0.0 {
        return Err(Error::Row("edit rate is invalid"));
    }

    let removal = config.case.starts_with("remove_");
    let churn = config.case == "churn";
    let planned = if removal {
        config.width.min(config.operations)
    } else {
        config.operations
    };
    let completed_units = multiplied(planned, config.iterations)?;
    let calls_per_unit = if churn { 2 } else { 1 };
    let completed_calls = multiplied(completed_units, calls_per_unit)?;
    let created = if removal { 0 } else { completed_units };
    let subtree_nodes = if config.case == "remove_deep" {
        added(config.depth, usize::from(config.payload_bytes != 0))?
    } else {
        added(1, usize::from(config.payload_bytes != 0))?
    };
    let removed = if removal {
        multiplied(completed_units, subtree_nodes)?
    } else if churn {
        completed_units
    } else {
        0
    };
    let initial = if config.case == "remove_deep" {
        added(2, multiplied(config.width, subtree_nodes)?)?
    } else {
        added(
            added(1, config.depth)?,
            multiplied(config.width, subtree_nodes)?,
        )?
    };
    for (name, expected) in [
        ("planned_units_per_iteration", planned),
        ("completed_units_total", completed_units),
        ("requested_edit_calls_total", completed_calls),
        ("completed_edit_calls_total", completed_calls),
        ("created_nodes_total", created),
        ("removed_nodes_total", removed),
        ("initial_live_nodes_per_iteration", initial),
    ] {
        if row.usize(name)? != expected {
            return Err(Error::Mismatch("row operation accounting is wrong"));
        }
    }
    if row.get("implementation")? == "yaxml" {
        for (name, expected) in [
            ("max_value_bytes", 1_048_576),
            ("max_owned_bytes", 67_108_864),
            ("max_created_nodes", 1_000_000),
        ] {
            if row.usize(name)? != expected {
                return Err(Error::Mismatch("yaxml quota is not the matched default"));
            }
        }
    } else if row.get("implementation")? == "libxml2" {
        if row.get("libxml_version")? != "21504" {
            return Err(Error::Mismatch("libxml2 row version is not pinned"));
        }
    } else {
        return Err(Error::Mismatch("unknown implementation label"));
    }
    let implementation = row.get("implementation")?;
    let merged_per_iteration = if implementation == "libxml2" && config.case == "append_text" {
        planned.saturating_sub(1)
    } else {
        0
    };
    let expected_merged = multiplied(merged_per_iteration, config.iterations)?;
    let final_live = if removal {
        initial
            .checked_sub(multiplied(planned, subtree_nodes)?)
            .ok_or(Error::Mismatch("expected accounting underflow"))?
    } else if churn {
        initial
    } else {
        added(initial, planned - merged_per_iteration)?
    };
    let traversal_visits = multiplied(
        multiplied(final_live, config.iterations)?,
        config.traversal_passes,
    )?;
    for (name, expected) in [
        ("merged_nodes_total", expected_merged),
        ("final_live_nodes_per_iteration", final_live),
        ("traversal_visits_total", traversal_visits),
    ] {
        if row.usize(name)? != expected {
            return Err(Error::Mismatch("row live-node accounting is wrong"));
        }
    }
    Ok(())
}

pub fn validate_pair(rust: &Row, libxml2: &Row) -> Result<PairSummary, Error> {
    if rust.get("implementation")? != "yaxml" || libxml2.get("implementation")? != "libxml2" {
        return Err(Error::Mismatch("implementation labels are wrong"));
    }
    for name in [
        "schema",
        "case",
        "encoding",
        "width",
        "depth",
        "payload_bytes",
        "input_bytes",
        "iterations",
        "configured_units_per_iteration",
        "planned_units_per_iteration",
        "completed_units_total",
        "requested_edit_calls_total",
        "completed_edit_calls_total",
        "created_nodes_total",
        "removed_nodes_total",
        "initial_live_nodes_per_iteration",
        "traversal_passes",
        "serialization_passes",
        "counted_output_bytes_total",
        "validation_output_bytes_total",
        "output_hash",
        "status",
    ] {
        if rust.get(name)? != libxml2.get(name)? {
            return Err(Error::Mismatch("matched field differs"));
        }
    }
    let case = rust.get("case")?;
    let rust_final = rust.usize("final_live_nodes_per_iteration")?;
    let c_final = libxml2.usize("final_live_nodes_per_iteration")?;
    let c_merged = libxml2.usize("merged_nodes_total")?;
    if rust.usize("merged_nodes_total")? != 0 {
        return Err(Error::Mismatch("yaxml unexpectedly reports merged nodes"));
    }
    let delta = rust_final
        .checked_sub(c_final)
        .ok_or(Error::Mismatch("libxml2 has more live nodes than yaxml"))?;
    if case == "append_text" {
        let planned = rust.usize("planned_units_per_iteration")?;
        let iterations = rust.usize("iterations")?;
        let expected_delta = planned.saturating_sub(1);
        let expected_merged = expected_delta
            .checked_mul(iterations)
            .ok_or(Error::Mismatch("merged-node accounting overflow"))?;
        if delta != expected_delta || c_merged != expected_merged {
            return Err(Error::Mismatch("Text coalescing accounting differs"));
        }
    } else if delta != 0 || c_merged != 0 {
        return Err(Error::Mismatch("non-Text live-node accounting differs"));
    }
    let rust_visits = rust.usize("traversal_visits_total")?;
    let c_visits = libxml2.usize("traversal_visits_total")?;
    let expected_visit_delta = delta
        .checked_mul(rust.usize("iterations")?)
        .and_then(|value| value.checked_mul(rust.usize("traversal_passes").ok()?))
        .ok_or(Error::Mismatch("traversal accounting overflow"))?;
    if rust_visits.checked_sub(c_visits) != Some(expected_visit_delta) {
        return Err(Error::Mismatch("live traversal accounting differs"));
    }
    Ok(PairSummary {
        text_live_node_delta_per_iteration: delta,
    })
}

pub fn verify_version(output: &[u8]) -> Result<(), Error> {
    let output = std::str::from_utf8(output).map_err(|_| Error::Row("version is not UTF-8"))?;
    let expected = format!("{PROTOCOL} compiled=21504 runtime=21504 tree=1 output=1\n");
    if output != expected {
        return Err(Error::Mismatch("helper is not pinned libxml2 2.15.4"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(case: &str, payload: &str) -> Vec<OsString> {
        [
            "--libxml2-helper",
            "/helper",
            "--yaxml-helper",
            "/yaxml",
            "--case",
            case,
            "--encoding",
            "utf8",
            "--width",
            "16",
            "--depth",
            "64",
            "--payload-bytes",
            payload,
            "--operations",
            "10",
            "--traversal-passes",
            "2",
            "--serialization-passes",
            "1",
        ]
        .into_iter()
        .map(OsString::from)
        .collect()
    }

    fn row(
        implementation: &str,
        case: &str,
        final_live: usize,
        merged: usize,
        visits: usize,
    ) -> Row {
        Row::parse(
            format!(
                "benchmark=owned_structure schema=2 implementation={implementation} libxml_version=21504 case={case} encoding=utf8 width=16 depth=64 payload_bytes=32 input_bytes=100 iterations=5 configured_units_per_iteration=10 planned_units_per_iteration=10 completed_units_total=50 requested_edit_calls_total=50 completed_edit_calls_total=50 created_nodes_total=50 removed_nodes_total=0 merged_nodes_total={merged} initial_live_nodes_per_iteration=97 final_live_nodes_per_iteration={final_live} traversal_passes=2 traversal_visits_total={visits} serialization_passes=1 counted_output_bytes_total=100 validation_output_bytes_total=100 sink_writes_total=1 max_value_bytes=1048576 max_owned_bytes=67108864 max_created_nodes=1000000 output_hash=abc status=complete setup_ns=1 edit_ns=1 traversal_ns=1 serialization_ns=1 validation_ns=1 drop_ns=1 edit_calls_per_s=1.0\n"
            )
            .as_bytes(),
        )
        .unwrap()
    }

    #[test]
    fn arguments_accept_valid_counts_and_reject_unsafe_or_lexically_different_rows() {
        let config = parse_args(args("churn", "0")).unwrap();
        assert_eq!(config.samples, 7);
        assert_eq!(config.iterations, 5);
        assert_eq!(config.operations, 10);
        assert_eq!(
            parse_args(args("append_pi", "0")),
            Err(Error::Argument(
                "empty PI payload has an intentional libxml2 lexical difference"
            ))
        );
        let mut unknown = args("churn", "1");
        unknown.extend([OsString::from("--wat"), OsString::from("1")]);
        assert_eq!(parse_args(unknown), Err(Error::Argument("unknown option")));
    }

    #[test]
    fn rows_fail_closed_on_missing_duplicate_incomplete_or_extra_lines() {
        assert_eq!(Row::parse(b""), Err(Error::Row("missing result row")));
        assert_eq!(
            Row::parse(b"benchmark=owned_structure benchmark=x\n"),
            Err(Error::Row("empty or duplicate field"))
        );
        assert_eq!(
            Row::parse(b"benchmark=owned_structure\nsecond=x\n"),
            Err(Error::Row("multiple result rows"))
        );
    }

    #[test]
    fn pair_gate_accepts_only_the_documented_text_coalescing_delta() {
        let rust = row("yaxml", "append_text", 107, 0, 1070);
        let c = row("libxml2", "append_text", 98, 45, 980);
        assert_eq!(
            validate_pair(&rust, &c)
                .unwrap()
                .text_live_node_delta_per_iteration,
            9
        );
        let wrong_hash = Row::parse(
            c.text
                .replace("output_hash=abc", "output_hash=def")
                .as_bytes(),
        )
        .unwrap();
        assert!(matches!(
            validate_pair(&rust, &wrong_hash),
            Err(Error::Mismatch("matched field differs"))
        ));
        let non_text_rust = row("yaxml", "append_comment", 107, 0, 1070);
        let non_text_c = row("libxml2", "append_comment", 106, 0, 1060);
        assert!(matches!(
            validate_pair(&non_text_rust, &non_text_c),
            Err(Error::Mismatch("non-Text live-node accounting differs"))
        ));
    }

    #[test]
    fn configuration_gate_rejects_zero_or_wrong_work_and_missing_phase_fields() {
        let config = parse_args(args("append_text", "32")).unwrap();
        let valid = row("yaxml", "append_text", 107, 0, 1070);
        assert_eq!(validate_config(&config, &valid), Ok(()));
        let wrong = Row::parse(
            valid
                .text
                .replace("completed_units_total=50", "completed_units_total=0")
                .as_bytes(),
        )
        .unwrap();
        assert_eq!(
            validate_config(&config, &wrong),
            Err(Error::Mismatch("row operation accounting is wrong"))
        );
        let missing = Row::parse(valid.text.replace(" serialization_ns=1", "").as_bytes());
        assert!(missing.is_ok(), "phase requirements belong to config gate");
        assert_eq!(
            validate_config(&config, &missing.unwrap()),
            Err(Error::Row("required field is missing"))
        );
    }

    #[test]
    fn version_gate_requires_both_pinned_versions_and_features() {
        assert_eq!(
            verify_version(
                b"YAXML-STRUCTURE-BENCH-1 compiled=21504 runtime=21504 tree=1 output=1\n"
            ),
            Ok(())
        );
        assert!(
            verify_version(
                b"YAXML-STRUCTURE-BENCH-1 compiled=21504 runtime=21503 tree=1 output=1\n"
            )
            .is_err()
        );
    }
}
