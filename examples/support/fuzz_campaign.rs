//! Pure campaign policy and coverage-summary logic.
#![forbid(unsafe_code)]

use crate::file_identity::content_identity;
use std::collections::BTreeSet;
use std::fmt;
use std::path::Path;

pub const MAX_INPUT_BYTES: usize = 1024 * 1024;
pub const MAX_SEED_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    pub cases: usize,
    pub wall_seconds: u64,
    pub target_seconds: u64,
    pub input_bytes: usize,
    pub output_bytes: usize,
    pub profile_bytes: usize,
    pub seed_files: usize,
    pub seed_bytes: usize,
    pub corpus_files: usize,
    pub corpus_bytes: usize,
    pub pending_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            cases: 500,
            wall_seconds: 300,
            target_seconds: 5,
            input_bytes: MAX_INPUT_BYTES,
            output_bytes: 64 * 1024,
            profile_bytes: 32 * 1024 * 1024,
            seed_files: 128,
            seed_bytes: MAX_SEED_BYTES,
            corpus_files: 256,
            corpus_bytes: 16 * 1024 * 1024,
            pending_bytes: 2 * 1024 * 1024,
        }
    }
}

impl Limits {
    pub fn validate(self) -> Result<Self, CampaignError> {
        let hard = Self {
            cases: 10_000,
            wall_seconds: 3_600,
            target_seconds: 30,
            input_bytes: MAX_INPUT_BYTES,
            output_bytes: 1024 * 1024,
            profile_bytes: 128 * 1024 * 1024,
            seed_files: 128,
            seed_bytes: MAX_SEED_BYTES,
            corpus_files: 1_024,
            corpus_bytes: 64 * 1024 * 1024,
            pending_bytes: 2 * 1024 * 1024,
        };
        if self.cases == 0
            || self.wall_seconds == 0
            || self.target_seconds == 0
            || self.input_bytes == 0
            || self.output_bytes == 0
            || self.profile_bytes == 0
            || self.seed_files == 0
            || self.seed_bytes == 0
            || self.corpus_files == 0
            || self.corpus_bytes == 0
            || self.pending_bytes == 0
        {
            return Err(CampaignError::Invalid(
                "all campaign limits must be positive",
            ));
        }
        if self.cases > hard.cases
            || self.wall_seconds > hard.wall_seconds
            || self.target_seconds > hard.target_seconds
            || self.input_bytes > hard.input_bytes
            || self.output_bytes > hard.output_bytes
            || self.profile_bytes > hard.profile_bytes
            || self.seed_files > hard.seed_files
            || self.seed_bytes > hard.seed_bytes
            || self.corpus_files > hard.corpus_files
            || self.corpus_bytes > hard.corpus_bytes
            || self.pending_bytes > hard.pending_bytes
        {
            return Err(CampaignError::Invalid(
                "campaign limit exceeds its hard maximum",
            ));
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Counts {
    pub regions_total: u64,
    pub regions_covered: u64,
    pub functions_total: u64,
    pub functions_covered: u64,
    pub lines_total: u64,
    pub lines_covered: u64,
    pub branches_total: u64,
    pub branches_covered: u64,
}

impl Counts {
    pub fn has_novelty_over(self, old: Self) -> bool {
        self.regions_covered > old.regions_covered
            || self.functions_covered > old.functions_covered
            || self.branches_covered > old.branches_covered
    }

    pub fn denominators_match(self, old: Self) -> bool {
        self.regions_total == old.regions_total
            && self.functions_total == old.functions_total
            && self.lines_total == old.lines_total
            && self.branches_total == old.branches_total
    }

    pub fn covered_never_decreases(self, old: Self) -> bool {
        self.regions_covered >= old.regions_covered
            && self.functions_covered >= old.functions_covered
            && self.lines_covered >= old.lines_covered
            && self.branches_covered >= old.branches_covered
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Summary {
    pub counts: Counts,
    pub files: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default)]
struct Totals {
    regions: (u64, u64),
    functions: (u64, u64),
    lines: (u64, u64),
    branches: (u64, u64),
}

const HEADER: &[&str] = &[
    "Filename",
    "Regions",
    "Missed",
    "Regions",
    "Cover",
    "Functions",
    "Missed",
    "Functions",
    "Executed",
    "Lines",
    "Missed",
    "Lines",
    "Cover",
    "Branches",
    "Missed",
    "Branches",
    "Cover",
];

/// Parse the stable LLVM 22 `llvm-cov report` text summary.
pub fn parse_summary(text: &str, expected_files: &[String]) -> Result<Summary, CampaignError> {
    let mut lines = text.lines();
    let header = lines
        .next()
        .ok_or(CampaignError::Report("missing header"))?;
    if header.split_whitespace().collect::<Vec<_>>() != HEADER {
        return Err(CampaignError::Report("unexpected LLVM report header"));
    }
    let mut files = Vec::new();
    let mut total = None;
    let mut sums = Totals::default();
    for line in lines {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.is_empty() || fields[0].bytes().all(|byte| byte == b'-') {
            continue;
        }
        if fields.len() != 13 {
            return Err(CampaignError::Report("unexpected summary row width"));
        }
        let counts = Counts {
            regions_total: number(fields[1])?,
            regions_covered: subtract(fields[1], fields[2])?,
            functions_total: number(fields[4])?,
            functions_covered: subtract(fields[4], fields[5])?,
            lines_total: number(fields[7])?,
            lines_covered: subtract(fields[7], fields[8])?,
            branches_total: number(fields[10])?,
            branches_covered: subtract(fields[10], fields[11])?,
        };
        check_percentage(fields[3], counts.regions_total)?;
        check_percentage(fields[6], counts.functions_total)?;
        check_percentage(fields[9], counts.lines_total)?;
        check_percentage(fields[12], counts.branches_total)?;
        if fields[0] == "TOTAL" {
            if total.replace(counts).is_some() {
                return Err(CampaignError::Report("duplicate TOTAL row"));
            }
        } else {
            files.push(fields[0].to_owned());
            add(&mut sums.regions, counts.regions_total, number(fields[2])?)?;
            add(
                &mut sums.functions,
                counts.functions_total,
                number(fields[5])?,
            )?;
            add(&mut sums.lines, counts.lines_total, number(fields[8])?)?;
            add(
                &mut sums.branches,
                counts.branches_total,
                number(fields[11])?,
            )?;
        }
    }
    let expected: BTreeSet<_> = expected_files.iter().map(String::as_str).collect();
    let actual: BTreeSet<_> = files.iter().map(String::as_str).collect();
    if expected.len() != expected_files.len()
        || actual != expected
        || files.len() != expected_files.len()
    {
        return Err(CampaignError::Report(
            "production source rows differ from manifest",
        ));
    }
    let counts = total.ok_or(CampaignError::Report("missing TOTAL row"))?;
    if sums.regions
        != (
            counts.regions_total,
            counts.regions_total - counts.regions_covered,
        )
        || sums.functions
            != (
                counts.functions_total,
                counts.functions_total - counts.functions_covered,
            )
        || sums.lines
            != (
                counts.lines_total,
                counts.lines_total - counts.lines_covered,
            )
        || sums.branches
            != (
                counts.branches_total,
                counts.branches_total - counts.branches_covered,
            )
    {
        return Err(CampaignError::Report("source rows do not sum to TOTAL"));
    }
    if counts.regions_covered > counts.regions_total
        || counts.functions_covered > counts.functions_total
        || counts.lines_covered > counts.lines_total
        || counts.branches_covered > counts.branches_total
    {
        return Err(CampaignError::Report("missed count exceeds denominator"));
    }
    Ok(Summary { counts, files })
}

fn number(field: &str) -> Result<u64, CampaignError> {
    field
        .parse()
        .map_err(|_| CampaignError::Report("invalid count"))
}

fn subtract(total: &str, missed: &str) -> Result<u64, CampaignError> {
    number(total)?
        .checked_sub(number(missed)?)
        .ok_or(CampaignError::Report("missed count exceeds denominator"))
}

fn add(total: &mut (u64, u64), value: u64, missed: u64) -> Result<(), CampaignError> {
    total.0 = total
        .0
        .checked_add(value)
        .ok_or(CampaignError::Report("count sum overflow"))?;
    total.1 = total
        .1
        .checked_add(missed)
        .ok_or(CampaignError::Report("count sum overflow"))?;
    Ok(())
}

fn check_percentage(field: &str, denominator: u64) -> Result<(), CampaignError> {
    if field == "-" {
        return if denominator == 0 {
            Ok(())
        } else {
            Err(CampaignError::Report("missing percentage"))
        };
    }
    let number = field
        .strip_suffix('%')
        .ok_or(CampaignError::Report("invalid percentage"))?
        .parse::<f64>()
        .map_err(|_| CampaignError::Report("invalid percentage"))?;
    if !number.is_finite()
        || !(0.0..=100.0).contains(&number)
        || (denominator == 0 && number != 0.0)
    {
        return Err(CampaignError::Report(
            "percentage is outside expected range",
        ));
    }
    Ok(())
}

#[derive(Debug, Eq, PartialEq)]
pub enum CampaignError {
    Invalid(&'static str),
    Report(&'static str),
}

impl fmt::Display for CampaignError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) | Self::Report(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for CampaignError {}

/// Stable accidental-corruption identity; this is not a cryptographic hash.
pub fn identity(bytes: &[u8]) -> (u64, usize) {
    content_identity(bytes)
}

pub fn source_names(
    source_directory: &Path,
    paths: &[impl AsRef<Path>],
) -> Result<Vec<String>, CampaignError> {
    let mut names = Vec::with_capacity(paths.len());
    for path in paths {
        let path = path.as_ref();
        let relative = path
            .strip_prefix(source_directory)
            .map_err(|_| CampaignError::Invalid("production source escaped its src directory"))?;
        let name = relative.to_str().ok_or(CampaignError::Invalid(
            "production source relative path is not UTF-8",
        ))?;
        if name.is_empty()
            || relative.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            })
        {
            return Err(CampaignError::Invalid(
                "production source relative path is invalid",
            ));
        }
        names.push(name.to_owned());
    }
    names.sort();
    if names.is_empty() || names.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(CampaignError::Invalid(
            "production source list is empty or ambiguous",
        ));
    }
    Ok(names)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mutation {
    Replace,
    Insert,
    Delete,
    Truncate,
    Splice,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MutationRecord {
    pub parent_index: usize,
    pub operation: Mutation,
    pub offset: usize,
    pub value: u8,
    pub splice_len: usize,
    pub pre_state: u64,
    pub rng_state: u64,
}

/// Fixed xorshift(13, 7, 17) then multiply PRNG. State zero is remapped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Prng(u64);

impl Prng {
    pub fn new(seed: u64) -> Self {
        Self(if seed == 0 { 0x9e3779b97f4a7c15 } else { seed })
    }

    pub fn next(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.0 = value;
        value.wrapping_mul(0x2545f4914f6cdd1d)
    }

    pub fn state(self) -> u64 {
        self.0
    }
}

pub fn mutate(parent: &[u8], rng: &mut Prng, max_len: usize) -> (Vec<u8>, MutationRecord) {
    let pre_state = rng.state();
    let operation = match rng.next() % 5 {
        0 => Mutation::Replace,
        1 => Mutation::Insert,
        2 => Mutation::Delete,
        3 => Mutation::Truncate,
        _ => Mutation::Splice,
    };
    let mut output = parent.to_vec();
    let offset = if parent.is_empty() {
        0
    } else {
        usize::try_from(rng.next() % parent.len() as u64).unwrap_or(0)
    };
    let value = rng.next() as u8;
    let mut splice_len = 0;
    match operation {
        Mutation::Replace if !output.is_empty() => output[offset] = value,
        Mutation::Insert if output.len() < max_len => {
            output.insert(offset.min(output.len()), value)
        }
        Mutation::Delete if !output.is_empty() => {
            output.remove(offset);
        }
        Mutation::Truncate => output.truncate(offset),
        Mutation::Splice if output.len() < max_len => {
            splice_len = (rng.next() as usize % 16).min(max_len - output.len());
            output.splice(
                offset.min(output.len())..offset.min(output.len()),
                std::iter::repeat_n(value, splice_len),
            );
        }
        _ => {}
    }
    if output.len() > max_len {
        output.truncate(max_len);
    }
    let record = MutationRecord {
        parent_index: 0,
        operation,
        offset,
        value,
        splice_len,
        pre_state,
        rng_state: rng.state(),
    };
    (output, record)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_hard_limits_are_enforced() {
        let defaults = Limits {
            cases: 500,
            wall_seconds: 300,
            target_seconds: 5,
            input_bytes: 1024 * 1024,
            output_bytes: 64 * 1024,
            profile_bytes: 32 * 1024 * 1024,
            seed_files: 128,
            seed_bytes: 8 * 1024 * 1024,
            corpus_files: 256,
            corpus_bytes: 16 * 1024 * 1024,
            pending_bytes: 2 * 1024 * 1024,
        };
        assert_eq!(Limits::default(), defaults);
        assert_eq!(defaults.validate(), Ok(defaults));

        // This literal is the frozen public hard-limit contract. Accept the
        // exact maxima together, and reject zero or maximum+1 independently
        // for every public limit field.
        let hard = Limits {
            cases: 10_000,
            wall_seconds: 3_600,
            target_seconds: 30,
            input_bytes: 1024 * 1024,
            output_bytes: 1024 * 1024,
            profile_bytes: 128 * 1024 * 1024,
            seed_files: 128,
            seed_bytes: 8 * 1024 * 1024,
            corpus_files: 1_024,
            corpus_bytes: 64 * 1024 * 1024,
            pending_bytes: 2 * 1024 * 1024,
        };
        assert_eq!(hard.validate(), Ok(hard));

        macro_rules! check_limit {
            ($field:ident) => {{
                let mut zero = hard;
                zero.$field = 0;
                assert_eq!(
                    zero.validate(),
                    Err(CampaignError::Invalid(
                        "all campaign limits must be positive"
                    )),
                    "zero {}",
                    stringify!($field)
                );
                let mut above = hard;
                above.$field += 1;
                assert_eq!(
                    above.validate(),
                    Err(CampaignError::Invalid(
                        "campaign limit exceeds its hard maximum"
                    )),
                    "maximum+1 {}",
                    stringify!($field)
                );
            }};
        }
        check_limit!(cases);
        check_limit!(wall_seconds);
        check_limit!(target_seconds);
        check_limit!(input_bytes);
        check_limit!(output_bytes);
        check_limit!(profile_bytes);
        check_limit!(seed_files);
        check_limit!(seed_bytes);
        check_limit!(corpus_files);
        check_limit!(corpus_bytes);
        check_limit!(pending_bytes);
    }

    #[test]
    fn llvm_report_requires_schema_rows_and_total() {
        let names = vec!["writer.rs".to_owned()];
        let text = "Filename Regions Missed Regions Cover Functions Missed Functions Executed Lines Missed Lines Cover Branches Missed Branches Cover\n-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------\nwriter.rs 549 17 96.90% 24 1 95.83% 315 3 99.05% 64 1 98.44%\n-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------\nTOTAL 549 17 96.90% 24 1 95.83% 315 3 99.05% 64 1 98.44%\n";
        let parsed = parse_summary(&text, &names).unwrap();
        assert_eq!(parsed.counts.regions_covered, 532);
        assert_eq!(parsed.counts.functions_covered, 23);
        assert_eq!(parsed.counts.lines_covered, 312);
        assert_eq!(parsed.counts.branches_covered, 63);
        assert!(parse_summary(&text.replace("Branches", "Arcs"), &names).is_err());
        assert!(parse_summary(&text.replace("TOTAL", "OTHER"), &names).is_err());
        assert!(parse_summary(&text.replace("writer.rs", "extra.rs"), &names).is_err());
        assert!(parse_summary(&text.replace("96.90%", "nan%"), &names).is_err());
        let zero = text.replace(
            "549 17 96.90% 24 1 95.83% 315 3 99.05% 64 1 98.44%",
            "0 0 - 0 0 - 0 0 - 0 0 -",
        );
        assert!(parse_summary(&zero, &names).is_ok());
        let invalid_zero = zero.replace("0 0 -", "0 0 0.00%");
        assert!(parse_summary(&invalid_zero, &names).is_ok());
    }

    #[test]
    fn denominator_freeze_and_novelty_are_separate() {
        let original = Counts {
            regions_total: 10,
            regions_covered: 5,
            functions_total: 2,
            functions_covered: 1,
            lines_total: 8,
            lines_covered: 4,
            branches_total: 6,
            branches_covered: 2,
        };
        let duplicate = Counts {
            regions_covered: 5,
            ..original
        };
        assert!(duplicate.denominators_match(original));
        assert!(!duplicate.has_novelty_over(original));
        let novel = Counts {
            regions_covered: 6,
            ..original
        };
        assert!(novel.has_novelty_over(original));
        assert!(
            !Counts {
                branches_total: 7,
                ..novel
            }
            .denominators_match(original)
        );
        assert!(
            !Counts {
                regions_covered: 4,
                ..novel
            }
            .covered_never_decreases(original)
        );
    }

    #[test]
    fn mutation_stream_is_replayable_and_bounded() {
        let mut left = Prng::new(7);
        let mut right = Prng::new(7);
        let first = mutate(b"<r/>", &mut left, 8);
        assert_eq!(first, mutate(b"<r/>", &mut right, 8));
        for _ in 0..100 {
            let (candidate, _) = mutate(&first.0, &mut left, 8);
            assert!(candidate.len() <= 8);
        }
        assert_ne!(identity(b"a"), identity(b"b"));
        let mut golden = Prng::new(1);
        assert_eq!(golden.next(), 0xbafacf624f01c45d);
        assert_eq!(golden.next(), 0x02da6891e507685d);
    }

    #[test]
    fn source_rows_preserve_sorted_relative_identity() {
        assert_eq!(
            source_names(
                Path::new("src"),
                &["src/z.rs", "src/owned/construction.rs", "src/a.rs"]
            )
            .unwrap(),
            vec!["a.rs", "owned/construction.rs", "z.rs"]
        );
        assert!(source_names(Path::new("src"), &["src/a.rs", "other/a.rs"]).is_err());
        assert!(source_names(Path::new("src"), &["src/a.rs", "src/a.rs"]).is_err());
    }
}
