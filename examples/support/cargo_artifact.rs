//! Bounded Cargo JSON-message parsing and pure library-test artifact selection.
#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::PathBuf;

pub(crate) const MAX_CARGO_JSONL_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_CARGO_JSON_LINE_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_CARGO_JSON_RECORDS: usize = 4_096;
pub(crate) const MAX_JSON_DEPTH: usize = 32;
pub(crate) const MAX_JSON_NODES_PER_RECORD: usize = 65_536;
pub(crate) const MAX_JSON_OBJECT_MEMBERS: usize = 4_096;
pub(crate) const MAX_JSON_ARRAY_ELEMENTS: usize = 4_096;
pub(crate) const MAX_JSON_KEY_BYTES: usize = 256;
pub(crate) const MAX_JSON_STRING_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_PACKAGE_ID_BYTES: usize = 16 * 1024;
pub(crate) const MAX_TARGET_FIELD_BYTES: usize = 1024;
pub(crate) const MAX_PATH_FIELD_BYTES: usize = 32 * 1024;
pub(crate) const MAX_TARGET_KINDS: usize = 16;
pub(crate) const MAX_CRATE_TYPES: usize = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CargoBuildStatus {
    Success,
    NonZero,
    TimedOut,
    OutputLimit,
    SpawnFailure,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ExpectedArtifact {
    pub package_id: String,
    pub target_name: String,
    pub target_kind: Vec<String>,
    pub crate_types: Vec<String>,
    pub edition: String,
    pub doc: bool,
    pub doctest: bool,
    pub test: bool,
    pub canonical_src_path: PathBuf,
    pub canonical_target_dir: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CargoTargetMetadata {
    pub kind: Vec<String>,
    pub crate_types: Vec<String>,
    pub name: String,
    pub src_path: String,
    pub edition: String,
    pub doc: bool,
    pub doctest: bool,
    pub test: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ArtifactRecord {
    line: usize,
    package_id: String,
    target: CargoTargetMetadata,
    profile_test: bool,
    executable: String,
    record_identity: (u64, usize),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StreamLimitKind {
    TotalBytes,
    LineBytes,
    Records,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StreamLayoutKind {
    EmptyRecord,
    MissingFinalLf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum JsonLimitKind {
    Depth,
    Nodes,
    ObjectMembers,
    ArrayElements,
    KeyBytes,
    StringBytes,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum JsonErrorKind {
    InvalidUtf8,
    UnexpectedEnd,
    UnexpectedByte,
    RootNotObject,
    TrailingBytes,
    StringControl,
    InvalidEscape,
    InvalidUnicodeEscape,
    InvalidSurrogate,
    InvalidNumber,
    DuplicateKey,
    Limit(JsonLimitKind),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BuildFinishedError {
    Missing,
    Duplicate,
    False,
    NotLast,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ArtifactError {
    BuildStatus(CargoBuildStatus),
    StreamLimit(StreamLimitKind),
    StreamLayout(StreamLayoutKind),
    Json {
        line: usize,
        offset: usize,
        kind: JsonErrorKind,
    },
    CargoSchema {
        line: usize,
        field: &'static str,
    },
    BuildFinished(BuildFinishedError),
    TargetMetadata {
        line: usize,
        field: &'static str,
    },
    NoMatchingArtifact,
    MultipleMatchingArtifacts {
        first_line: usize,
        second_line: usize,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum JsonValue {
    Null,
    Bool(bool),
    Number,
    String(String),
    Array(Vec<JsonValue>),
    Object(BTreeMap<String, JsonValue>),
}

struct JsonParser<'a> {
    input: &'a [u8],
    line: usize,
    offset: usize,
    nodes: usize,
    #[cfg(test)]
    raw_utf8_validation_bytes: usize,
}

impl<'a> JsonParser<'a> {
    fn new(input: &'a [u8], line: usize) -> Result<Self, ArtifactError> {
        if std::str::from_utf8(input).is_err() {
            return Err(ArtifactError::Json {
                line,
                offset: 0,
                kind: JsonErrorKind::InvalidUtf8,
            });
        }
        Ok(Self {
            input,
            line,
            offset: 0,
            nodes: 0,
            #[cfg(test)]
            raw_utf8_validation_bytes: 0,
        })
    }

    fn parse_root(&mut self) -> Result<BTreeMap<String, JsonValue>, ArtifactError> {
        self.whitespace();
        let value = self.value(1)?;
        self.whitespace();
        if self.offset != self.input.len() {
            return Err(self.error(JsonErrorKind::TrailingBytes));
        }
        match value {
            JsonValue::Object(object) => Ok(object),
            _ => Err(self.error(JsonErrorKind::RootNotObject)),
        }
    }

    fn value(&mut self, depth: usize) -> Result<JsonValue, ArtifactError> {
        if depth > MAX_JSON_DEPTH {
            return Err(self.error(JsonErrorKind::Limit(JsonLimitKind::Depth)));
        }
        self.nodes = self
            .nodes
            .checked_add(1)
            .ok_or_else(|| self.error(JsonErrorKind::Limit(JsonLimitKind::Nodes)))?;
        if self.nodes > MAX_JSON_NODES_PER_RECORD {
            return Err(self.error(JsonErrorKind::Limit(JsonLimitKind::Nodes)));
        }
        self.whitespace();
        match self.peek() {
            Some(b'n') => {
                self.literal(b"null")?;
                Ok(JsonValue::Null)
            }
            Some(b't') => {
                self.literal(b"true")?;
                Ok(JsonValue::Bool(true))
            }
            Some(b'f') => {
                self.literal(b"false")?;
                Ok(JsonValue::Bool(false))
            }
            Some(b'"') => self
                .string(MAX_JSON_STRING_BYTES, JsonLimitKind::StringBytes)
                .map(JsonValue::String),
            Some(b'[') => self.array(depth),
            Some(b'{') => self.object(depth),
            Some(b'-' | b'0'..=b'9') => {
                self.number()?;
                Ok(JsonValue::Number)
            }
            Some(_) => Err(self.error(JsonErrorKind::UnexpectedByte)),
            None => Err(self.error(JsonErrorKind::UnexpectedEnd)),
        }
    }

    fn array(&mut self, depth: usize) -> Result<JsonValue, ArtifactError> {
        self.offset += 1;
        let mut values = Vec::new();
        self.whitespace();
        if self.take(b']') {
            return Ok(JsonValue::Array(values));
        }
        loop {
            if values.len() >= MAX_JSON_ARRAY_ELEMENTS {
                return Err(self.error(JsonErrorKind::Limit(JsonLimitKind::ArrayElements)));
            }
            values.push(self.value(depth + 1)?);
            self.whitespace();
            if self.take(b']') {
                return Ok(JsonValue::Array(values));
            }
            if !self.take(b',') {
                return Err(self.error(if self.peek().is_none() {
                    JsonErrorKind::UnexpectedEnd
                } else {
                    JsonErrorKind::UnexpectedByte
                }));
            }
            self.whitespace();
        }
    }

    fn object(&mut self, depth: usize) -> Result<JsonValue, ArtifactError> {
        self.offset += 1;
        let mut values = BTreeMap::new();
        self.whitespace();
        if self.take(b'}') {
            return Ok(JsonValue::Object(values));
        }
        loop {
            if values.len() >= MAX_JSON_OBJECT_MEMBERS {
                return Err(self.error(JsonErrorKind::Limit(JsonLimitKind::ObjectMembers)));
            }
            if self.peek() != Some(b'"') {
                return Err(self.error(if self.peek().is_none() {
                    JsonErrorKind::UnexpectedEnd
                } else {
                    JsonErrorKind::UnexpectedByte
                }));
            }
            let key = self.string(MAX_JSON_KEY_BYTES, JsonLimitKind::KeyBytes)?;
            self.whitespace();
            if !self.take(b':') {
                return Err(self.error(if self.peek().is_none() {
                    JsonErrorKind::UnexpectedEnd
                } else {
                    JsonErrorKind::UnexpectedByte
                }));
            }
            let value = self.value(depth + 1)?;
            if values.insert(key, value).is_some() {
                return Err(self.error(JsonErrorKind::DuplicateKey));
            }
            self.whitespace();
            if self.take(b'}') {
                return Ok(JsonValue::Object(values));
            }
            if !self.take(b',') {
                return Err(self.error(if self.peek().is_none() {
                    JsonErrorKind::UnexpectedEnd
                } else {
                    JsonErrorKind::UnexpectedByte
                }));
            }
            self.whitespace();
        }
    }

    fn string(&mut self, limit: usize, limit_kind: JsonLimitKind) -> Result<String, ArtifactError> {
        debug_assert_eq!(self.peek(), Some(b'"'));
        self.offset += 1;
        let mut result = String::new();
        loop {
            let byte = self
                .peek()
                .ok_or_else(|| self.error(JsonErrorKind::UnexpectedEnd))?;
            match byte {
                b'"' => {
                    self.offset += 1;
                    return Ok(result);
                }
                b'\\' => {
                    self.offset += 1;
                    let escaped = self
                        .peek()
                        .ok_or_else(|| self.error(JsonErrorKind::UnexpectedEnd))?;
                    self.offset += 1;
                    match escaped {
                        b'"' => Self::push_char(&mut result, '"', limit, limit_kind, self)?,
                        b'\\' => Self::push_char(&mut result, '\\', limit, limit_kind, self)?,
                        b'/' => Self::push_char(&mut result, '/', limit, limit_kind, self)?,
                        b'b' => Self::push_char(&mut result, '\u{0008}', limit, limit_kind, self)?,
                        b'f' => Self::push_char(&mut result, '\u{000c}', limit, limit_kind, self)?,
                        b'n' => Self::push_char(&mut result, '\n', limit, limit_kind, self)?,
                        b'r' => Self::push_char(&mut result, '\r', limit, limit_kind, self)?,
                        b't' => Self::push_char(&mut result, '\t', limit, limit_kind, self)?,
                        b'u' => {
                            let first = self.hex_quad()?;
                            let scalar = if (0xd800..=0xdbff).contains(&first) {
                                if self.peek() != Some(b'\\')
                                    || self.input.get(self.offset + 1) != Some(&b'u')
                                {
                                    return Err(self.error(JsonErrorKind::InvalidSurrogate));
                                }
                                self.offset += 2;
                                let second = self.hex_quad()?;
                                if !(0xdc00..=0xdfff).contains(&second) {
                                    return Err(self.error(JsonErrorKind::InvalidSurrogate));
                                }
                                0x10000
                                    + ((u32::from(first) - 0xd800) << 10)
                                    + (u32::from(second) - 0xdc00)
                            } else if (0xdc00..=0xdfff).contains(&first) {
                                return Err(self.error(JsonErrorKind::InvalidSurrogate));
                            } else {
                                u32::from(first)
                            };
                            let character = char::from_u32(scalar)
                                .ok_or_else(|| self.error(JsonErrorKind::InvalidUnicodeEscape))?;
                            Self::push_char(&mut result, character, limit, limit_kind, self)?;
                        }
                        _ => return Err(self.error(JsonErrorKind::InvalidEscape)),
                    }
                }
                0x00..=0x1f => return Err(self.error(JsonErrorKind::StringControl)),
                0x20..=0x7f => {
                    self.offset += 1;
                    Self::push_char(&mut result, char::from(byte), limit, limit_kind, self)?;
                }
                _ => {
                    let (character, width) = self.raw_utf8_scalar()?;
                    self.offset += width;
                    Self::push_char(&mut result, character, limit, limit_kind, self)?;
                }
            }
        }
    }

    fn raw_utf8_scalar(&mut self) -> Result<(char, usize), ArtifactError> {
        let width = match self.peek() {
            Some(0xc2..=0xdf) => 2,
            Some(0xe0..=0xef) => 3,
            Some(0xf0..=0xf4) => 4,
            Some(_) => return Err(self.error(JsonErrorKind::InvalidUtf8)),
            None => return Err(self.error(JsonErrorKind::UnexpectedEnd)),
        };
        let end = self
            .offset
            .checked_add(width)
            .ok_or_else(|| self.error(JsonErrorKind::InvalidUtf8))?;
        let scalar_bytes = self
            .input
            .get(self.offset..end)
            .ok_or_else(|| self.error(JsonErrorKind::InvalidUtf8))?;
        #[cfg(test)]
        {
            self.raw_utf8_validation_bytes = self
                .raw_utf8_validation_bytes
                .checked_add(scalar_bytes.len())
                .ok_or_else(|| self.error(JsonErrorKind::InvalidUtf8))?;
        }
        let scalar = std::str::from_utf8(scalar_bytes)
            .map_err(|_| self.error(JsonErrorKind::InvalidUtf8))?;
        let mut characters = scalar.chars();
        let character = characters
            .next()
            .ok_or_else(|| self.error(JsonErrorKind::InvalidUtf8))?;
        if characters.next().is_some() || character.len_utf8() != width {
            return Err(self.error(JsonErrorKind::InvalidUtf8));
        }
        Ok((character, width))
    }

    fn push_char(
        result: &mut String,
        character: char,
        limit: usize,
        limit_kind: JsonLimitKind,
        parser: &Self,
    ) -> Result<(), ArtifactError> {
        let length = result
            .len()
            .checked_add(character.len_utf8())
            .ok_or_else(|| parser.error(JsonErrorKind::Limit(limit_kind)))?;
        if length > limit {
            return Err(parser.error(JsonErrorKind::Limit(limit_kind)));
        }
        result.push(character);
        Ok(())
    }

    fn hex_quad(&mut self) -> Result<u16, ArtifactError> {
        let mut value = 0u16;
        for _ in 0..4 {
            let byte = self
                .peek()
                .ok_or_else(|| self.error(JsonErrorKind::UnexpectedEnd))?;
            self.offset += 1;
            let digit = match byte {
                b'0'..=b'9' => u16::from(byte - b'0'),
                b'a'..=b'f' => u16::from(byte - b'a' + 10),
                b'A'..=b'F' => u16::from(byte - b'A' + 10),
                _ => return Err(self.error(JsonErrorKind::InvalidUnicodeEscape)),
            };
            value = value * 16 + digit;
        }
        Ok(value)
    }

    fn number(&mut self) -> Result<(), ArtifactError> {
        if self.take(b'-') && self.peek().is_none() {
            return Err(self.error(JsonErrorKind::InvalidNumber));
        }
        match self.peek() {
            Some(b'0') => {
                self.offset += 1;
                if matches!(self.peek(), Some(b'0'..=b'9')) {
                    return Err(self.error(JsonErrorKind::InvalidNumber));
                }
            }
            Some(b'1'..=b'9') => self.digits(),
            _ => return Err(self.error(JsonErrorKind::InvalidNumber)),
        }
        if self.take(b'.') {
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.error(JsonErrorKind::InvalidNumber));
            }
            self.digits();
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.offset += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.offset += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.error(JsonErrorKind::InvalidNumber));
            }
            self.digits();
        }
        Ok(())
    }

    fn digits(&mut self) {
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.offset += 1;
        }
    }

    fn literal(&mut self, literal: &[u8]) -> Result<(), ArtifactError> {
        if self
            .input
            .get(self.offset..self.offset.saturating_add(literal.len()))
            != Some(literal)
        {
            return Err(
                self.error(if self.offset + literal.len() > self.input.len() {
                    JsonErrorKind::UnexpectedEnd
                } else {
                    JsonErrorKind::UnexpectedByte
                }),
            );
        }
        self.offset += literal.len();
        Ok(())
    }

    fn whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.offset += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.offset).copied()
    }

    fn take(&mut self, byte: u8) -> bool {
        if self.peek() == Some(byte) {
            self.offset += 1;
            true
        } else {
            false
        }
    }

    fn error(&self, kind: JsonErrorKind) -> ArtifactError {
        ArtifactError::Json {
            line: self.line,
            offset: self.offset,
            kind,
        }
    }
}

pub(crate) fn select_artifact_record(
    jsonl: &[u8],
    build_status: CargoBuildStatus,
    expected: &ExpectedArtifact,
) -> Result<ArtifactRecord, ArtifactError> {
    if build_status != CargoBuildStatus::Success {
        return Err(ArtifactError::BuildStatus(build_status));
    }
    if jsonl.len() > MAX_CARGO_JSONL_BYTES {
        return Err(ArtifactError::StreamLimit(StreamLimitKind::TotalBytes));
    }
    if jsonl.last() != Some(&b'\n') {
        return Err(ArtifactError::StreamLayout(
            StreamLayoutKind::MissingFinalLf,
        ));
    }

    let mut selected: Option<ArtifactRecord> = None;
    let mut second_match = None;
    let mut build_finished = None;
    let mut records = 0usize;
    let mut start = 0usize;
    let mut line_number = 0usize;

    while start < jsonl.len() {
        line_number = line_number
            .checked_add(1)
            .ok_or(ArtifactError::StreamLimit(StreamLimitKind::Records))?;
        let relative_end = jsonl[start..]
            .iter()
            .position(|byte| *byte == b'\n')
            .expect("the stream's final byte was checked as LF");
        let end = start + relative_end;
        let line = &jsonl[start..end];
        start = end + 1;

        if line.is_empty() {
            return Err(ArtifactError::StreamLayout(StreamLayoutKind::EmptyRecord));
        }
        if line.len() > MAX_CARGO_JSON_LINE_BYTES {
            return Err(ArtifactError::StreamLimit(StreamLimitKind::LineBytes));
        }
        records = records
            .checked_add(1)
            .ok_or(ArtifactError::StreamLimit(StreamLimitKind::Records))?;
        if records > MAX_CARGO_JSON_RECORDS {
            return Err(ArtifactError::StreamLimit(StreamLimitKind::Records));
        }

        let object = JsonParser::new(line, line_number)?.parse_root()?;
        let reason = required_string(&object, "reason", line_number)?;

        if let Some((_, _)) = build_finished {
            return Err(ArtifactError::BuildFinished(
                if reason == "build-finished" {
                    BuildFinishedError::Duplicate
                } else {
                    BuildFinishedError::NotLast
                },
            ));
        }

        if reason == "build-finished" {
            let success = required_bool(&object, "success", line_number)?;
            build_finished = Some((line_number, success));
            continue;
        }
        if reason != "compiler-artifact" {
            continue;
        }

        if let Some(record) = artifact_from_object(&object, line, line_number, expected)? {
            if let Some(first) = &selected {
                if second_match.is_none() {
                    second_match = Some((first.line, record.line));
                }
            } else {
                selected = Some(record);
            }
        }
    }

    match build_finished {
        None => return Err(ArtifactError::BuildFinished(BuildFinishedError::Missing)),
        Some((_, false)) => return Err(ArtifactError::BuildFinished(BuildFinishedError::False)),
        Some((_, true)) => {}
    }
    if let Some((first_line, second_line)) = second_match {
        return Err(ArtifactError::MultipleMatchingArtifacts {
            first_line,
            second_line,
        });
    }
    selected.ok_or(ArtifactError::NoMatchingArtifact)
}

fn artifact_from_object(
    object: &BTreeMap<String, JsonValue>,
    line_bytes: &[u8],
    line: usize,
    expected: &ExpectedArtifact,
) -> Result<Option<ArtifactRecord>, ArtifactError> {
    let package_id = bounded_identity(
        required_string(object, "package_id", line)?,
        MAX_PACKAGE_ID_BYTES,
        line,
        "package_id",
    )?;
    if package_id != expected.package_id {
        return Ok(None);
    }

    let target = required_object(object, "target", line)?;
    let profile = required_object(object, "profile", line)?;
    let executable_value = required(object, "executable", line)?;
    let kind = bounded_string_array(
        required_array(target, "target.kind", line)?,
        MAX_TARGET_KINDS,
        MAX_TARGET_FIELD_BYTES,
        line,
        "target.kind",
    )?;
    let profile_test = required_bool(profile, "profile.test", line)?;
    if kind != expected.target_kind {
        return Ok(None);
    }

    let metadata = CargoTargetMetadata {
        kind,
        crate_types: bounded_string_array(
            required_array(target, "target.crate_types", line)?,
            MAX_CRATE_TYPES,
            MAX_TARGET_FIELD_BYTES,
            line,
            "target.crate_types",
        )?,
        name: bounded_identity(
            required_string(target, "target.name", line)?,
            MAX_TARGET_FIELD_BYTES,
            line,
            "target.name",
        )?,
        src_path: bounded_identity(
            required_string(target, "target.src_path", line)?,
            MAX_PATH_FIELD_BYTES,
            line,
            "target.src_path",
        )?,
        edition: bounded_identity(
            required_string(target, "target.edition", line)?,
            MAX_TARGET_FIELD_BYTES,
            line,
            "target.edition",
        )?,
        doc: required_bool(target, "target.doc", line)?,
        doctest: required_bool(target, "target.doctest", line)?,
        test: required_bool(target, "target.test", line)?,
    };
    let expected_src_path = expected.canonical_src_path.to_str();
    for (matches, field) in [
        (
            metadata.crate_types == expected.crate_types,
            "target.crate_types",
        ),
        (metadata.name == expected.target_name, "target.name"),
        (
            Some(metadata.src_path.as_str()) == expected_src_path,
            "target.src_path",
        ),
        (metadata.edition == expected.edition, "target.edition"),
        (metadata.doc == expected.doc, "target.doc"),
        (metadata.doctest == expected.doctest, "target.doctest"),
        (metadata.test == expected.test, "target.test"),
    ] {
        if !matches {
            return Err(ArtifactError::TargetMetadata { line, field });
        }
    }
    if !profile_test {
        return Ok(None);
    }
    let executable = match executable_value {
        JsonValue::String(value) => {
            bounded_identity(value, MAX_PATH_FIELD_BYTES, line, "executable")?
        }
        _ => {
            return Err(ArtifactError::CargoSchema {
                line,
                field: "executable",
            });
        }
    };
    Ok(Some(ArtifactRecord {
        line,
        package_id,
        target: metadata,
        profile_test,
        executable,
        record_identity: identity(line_bytes),
    }))
}

fn required<'a>(
    object: &'a BTreeMap<String, JsonValue>,
    field: &'static str,
    line: usize,
) -> Result<&'a JsonValue, ArtifactError> {
    let key = field.rsplit('.').next().expect("static field is nonempty");
    object
        .get(key)
        .ok_or(ArtifactError::CargoSchema { line, field })
}

fn required_string<'a>(
    object: &'a BTreeMap<String, JsonValue>,
    field: &'static str,
    line: usize,
) -> Result<&'a str, ArtifactError> {
    match required(object, field, line)? {
        JsonValue::String(value) => Ok(value),
        _ => Err(ArtifactError::CargoSchema { line, field }),
    }
}

fn required_bool(
    object: &BTreeMap<String, JsonValue>,
    field: &'static str,
    line: usize,
) -> Result<bool, ArtifactError> {
    match required(object, field, line)? {
        JsonValue::Bool(value) => Ok(*value),
        _ => Err(ArtifactError::CargoSchema { line, field }),
    }
}

fn required_object<'a>(
    object: &'a BTreeMap<String, JsonValue>,
    field: &'static str,
    line: usize,
) -> Result<&'a BTreeMap<String, JsonValue>, ArtifactError> {
    match required(object, field, line)? {
        JsonValue::Object(value) => Ok(value),
        _ => Err(ArtifactError::CargoSchema { line, field }),
    }
}

fn required_array<'a>(
    object: &'a BTreeMap<String, JsonValue>,
    field: &'static str,
    line: usize,
) -> Result<&'a [JsonValue], ArtifactError> {
    match required(object, field, line)? {
        JsonValue::Array(value) => Ok(value),
        _ => Err(ArtifactError::CargoSchema { line, field }),
    }
}

fn bounded_string_array(
    values: &[JsonValue],
    count_limit: usize,
    string_limit: usize,
    line: usize,
    field: &'static str,
) -> Result<Vec<String>, ArtifactError> {
    if values.len() > count_limit {
        return Err(ArtifactError::CargoSchema { line, field });
    }
    values
        .iter()
        .map(|value| match value {
            JsonValue::String(value) => bounded_identity(value, string_limit, line, field),
            _ => Err(ArtifactError::CargoSchema { line, field }),
        })
        .collect()
}

fn bounded_identity(
    value: &str,
    limit: usize,
    line: usize,
    field: &'static str,
) -> Result<String, ArtifactError> {
    if value.len() > limit || value.bytes().any(|byte| byte < 0x20 || byte == 0x7f) {
        return Err(ArtifactError::CargoSchema { line, field });
    }
    Ok(value.to_owned())
}

fn identity(bytes: &[u8]) -> (u64, usize) {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    (hash, bytes.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PACKAGE: &str = "path+file:///fixture/source#yaxml@0.1.0";
    const SOURCE: &str = "/fixture/source/src/lib.rs";
    const EXECUTABLE: &str = "/fixture/target/debug/deps/arbitrary test executable";
    const ARTIFACT: &str = concat!(
        "{\"reason\":\"compiler-artifact\",",
        "\"package_id\":\"path+file:///fixture/source#yaxml@0.1.0\",",
        "\"target\":{\"kind\":[\"lib\"],\"crate_types\":[\"lib\"],",
        "\"name\":\"yaxml\",\"src_path\":\"/fixture/source/src/lib.rs\",",
        "\"edition\":\"2024\",\"doc\":true,\"doctest\":true,\"test\":true},",
        "\"profile\":{\"test\":true},",
        "\"executable\":\"/fixture/target/debug/deps/arbitrary test executable\"}"
    );
    const FINISHED: &str = "{\"reason\":\"build-finished\",\"success\":true}\n";

    fn expected() -> ExpectedArtifact {
        ExpectedArtifact {
            package_id: PACKAGE.to_owned(),
            target_name: "yaxml".to_owned(),
            target_kind: vec!["lib".to_owned()],
            crate_types: vec!["lib".to_owned()],
            edition: "2024".to_owned(),
            doc: true,
            doctest: true,
            test: true,
            canonical_src_path: PathBuf::from(SOURCE),
            canonical_target_dir: PathBuf::from("/fixture/target"),
        }
    }

    fn stream(records: &[&str]) -> Vec<u8> {
        let mut result = Vec::new();
        for record in records {
            result.extend_from_slice(record.as_bytes());
            result.push(b'\n');
        }
        result
    }

    fn select(jsonl: &[u8]) -> Result<ArtifactRecord, ArtifactError> {
        select_artifact_record(jsonl, CargoBuildStatus::Success, &expected())
    }

    #[test]
    fn selects_one_exact_artifact_and_preserves_spaces() {
        let input = stream(&[ARTIFACT, &FINISHED[..FINISHED.len() - 1]]);
        let record = select(&input).unwrap();
        assert_eq!(record.line, 1);
        assert_eq!(record.package_id, PACKAGE);
        assert_eq!(record.target.kind, ["lib"]);
        assert_eq!(record.target.src_path, SOURCE);
        assert!(record.profile_test);
        assert_eq!(record.executable, EXECUTABLE);
        assert_eq!(record.record_identity, identity(ARTIFACT.as_bytes()));
    }

    #[test]
    fn accepts_reordered_unknown_and_escaped_fields() {
        const DEPENDENCY: &str = concat!(
            "{\"target\":17,\"profile\":null,\"executable\":false,",
            "\"package_id\":\"registry+https://example.invalid/index#dep@1.0.0\",",
            "\"reason\":\"compiler-artifact\",\"unknown\":{\"nested\":[null,true,-0.25e+2]}}"
        );
        const REORDERED: &str = concat!(
            "{\"fresh\":true,\"irrelevant\":[null,false,{\"n\":1e-2}],",
            "\"executable\":\"\\/fixture\\/target\\/debug\\/deps\\/artifact-\\uD83D\\uDE00\",",
            "\"profile\":{\"ignored\":0,\"test\":true},",
            "\"target\":{\"test\":true,\"doctest\":true,\"doc\":true,",
            "\"edition\":\"2024\",\"src_path\":\"\\/fixture\\/\\uD83D\\uDE00 source\\/src\\/lib.rs\",",
            "\"name\":\"yaxml\",\"crate_types\":[\"lib\"],\"kind\":[\"lib\"]},",
            "\"package_id\":\"path+file:\\/\\/\\/fixture\\/source#yaxml@0.1.0\",",
            "\"reason\":\"compiler-artifact\"}"
        );
        let mut expected = expected();
        expected.canonical_src_path = PathBuf::from("/fixture/😀 source/src/lib.rs");
        let input = stream(&[DEPENDENCY, REORDERED, &FINISHED[..FINISHED.len() - 1]]);
        let record = select_artifact_record(&input, CargoBuildStatus::Success, &expected).unwrap();
        assert_eq!(record.line, 2);
        assert_eq!(record.target.src_path, "/fixture/😀 source/src/lib.rs");
        assert_eq!(record.executable, "/fixture/target/debug/deps/artifact-😀");
    }

    #[test]
    fn ignores_well_formed_nonmatching_artifacts() {
        const WRONG_PACKAGE: &str = concat!(
            "{\"reason\":\"compiler-artifact\",\"package_id\":\"path+file:///fixture/source#yaxml-helper@0.1.0\",",
            "\"target\":17,\"profile\":null,\"executable\":false}"
        );
        const WRONG_KIND: &str = concat!(
            "{\"reason\":\"compiler-artifact\",\"package_id\":\"path+file:///fixture/source#yaxml@0.1.0\",",
            "\"target\":{\"kind\":[\"test\"]},\"profile\":{\"test\":true},\"executable\":null}"
        );
        const ORDINARY_LIB: &str = concat!(
            "{\"reason\":\"compiler-artifact\",\"package_id\":\"path+file:///fixture/source#yaxml@0.1.0\",",
            "\"target\":{\"kind\":[\"lib\"],\"crate_types\":[\"lib\"],\"name\":\"yaxml\",",
            "\"src_path\":\"/fixture/source/src/lib.rs\",\"edition\":\"2024\",",
            "\"doc\":true,\"doctest\":true,\"test\":true},",
            "\"profile\":{\"test\":false},\"executable\":null}"
        );
        const UNKNOWN: &str = concat!(
            "{\"reason\":\"future-cargo-message\",\"payload\":",
            "{\"a\":[null,true,false,0,-0,1.5,2E-3,\"x\\/y\"]}}"
        );
        let input = stream(&[
            WRONG_PACKAGE,
            WRONG_KIND,
            ORDINARY_LIB,
            UNKNOWN,
            ARTIFACT,
            &FINISHED[..FINISHED.len() - 1],
        ]);
        assert_eq!(select(&input).unwrap().line, 5);

        let none = stream(&[
            WRONG_PACKAGE,
            WRONG_KIND,
            ORDINARY_LIB,
            &FINISHED[..FINISHED.len() - 1],
        ]);
        assert_eq!(select(&none), Err(ArtifactError::NoMatchingArtifact));

        for kind in ["test", "example", "bin", "bench", "custom-build"] {
            let wrong_kind =
                WRONG_KIND.replace("\"kind\":[\"test\"]", &format!("\"kind\":[\"{kind}\"]"));
            let input = stream(&[&wrong_kind, ARTIFACT, &FINISHED[..FINISHED.len() - 1]]);
            assert_eq!(select(&input).unwrap().line, 2, "{kind}");
        }
    }

    #[test]
    fn requires_exactly_one_match_without_package_substrings() {
        const PACKAGE_SUBSTRING: &str = concat!(
            "{\"reason\":\"compiler-artifact\",",
            "\"package_id\":\"wrapper-path+file:///fixture/source#yaxml@0.1.0-suffix\",",
            "\"target\":{},\"profile\":{},\"executable\":null}"
        );
        let only_substring = stream(&[PACKAGE_SUBSTRING, &FINISHED[..FINISHED.len() - 1]]);
        assert_eq!(
            select(&only_substring),
            Err(ArtifactError::NoMatchingArtifact)
        );

        let two = stream(&[ARTIFACT, ARTIFACT, &FINISHED[..FINISHED.len() - 1]]);
        assert_eq!(
            select(&two),
            Err(ArtifactError::MultipleMatchingArtifacts {
                first_line: 1,
                second_line: 2,
            })
        );

        let other_executable = ARTIFACT.replace(EXECUTABLE, "/fixture/target/other");
        let two_different = stream(&[ARTIFACT, &other_executable, &FINISHED[..FINISHED.len() - 1]]);
        assert_eq!(
            select(&two_different),
            Err(ArtifactError::MultipleMatchingArtifacts {
                first_line: 1,
                second_line: 2,
            })
        );
    }

    #[test]
    fn rejects_malformed_candidate_schema() {
        struct Case {
            name: &'static str,
            record: &'static str,
            field: &'static str,
        }
        let cases = [
            Case {
                name: "missing_package",
                record: "{\"reason\":\"compiler-artifact\"}",
                field: "package_id",
            },
            Case {
                name: "package_wrong_type",
                record: "{\"reason\":\"compiler-artifact\",\"package_id\":null}",
                field: "package_id",
            },
            Case {
                name: "missing_target",
                record: "{\"reason\":\"compiler-artifact\",\"package_id\":\"path+file:///fixture/source#yaxml@0.1.0\",\"profile\":{\"test\":true},\"executable\":\"/x\"}",
                field: "target",
            },
            Case {
                name: "target_wrong_type",
                record: "{\"reason\":\"compiler-artifact\",\"package_id\":\"path+file:///fixture/source#yaxml@0.1.0\",\"target\":[],\"profile\":{\"test\":true},\"executable\":\"/x\"}",
                field: "target",
            },
            Case {
                name: "profile_wrong_type",
                record: "{\"reason\":\"compiler-artifact\",\"package_id\":\"path+file:///fixture/source#yaxml@0.1.0\",\"target\":{},\"profile\":true,\"executable\":\"/x\"}",
                field: "profile",
            },
            Case {
                name: "missing_executable_even_wrong_kind",
                record: "{\"reason\":\"compiler-artifact\",\"package_id\":\"path+file:///fixture/source#yaxml@0.1.0\",\"target\":{\"kind\":[\"bin\"]},\"profile\":{\"test\":true}}",
                field: "executable",
            },
            Case {
                name: "kind_wrong_type",
                record: "{\"reason\":\"compiler-artifact\",\"package_id\":\"path+file:///fixture/source#yaxml@0.1.0\",\"target\":{\"kind\":\"lib\"},\"profile\":{\"test\":true},\"executable\":\"/x\"}",
                field: "target.kind",
            },
            Case {
                name: "profile_test_wrong_type",
                record: "{\"reason\":\"compiler-artifact\",\"package_id\":\"path+file:///fixture/source#yaxml@0.1.0\",\"target\":{\"kind\":[\"lib\"]},\"profile\":{\"test\":1},\"executable\":\"/x\"}",
                field: "profile.test",
            },
            Case {
                name: "metadata_missing",
                record: "{\"reason\":\"compiler-artifact\",\"package_id\":\"path+file:///fixture/source#yaxml@0.1.0\",\"target\":{\"kind\":[\"lib\"]},\"profile\":{\"test\":true},\"executable\":\"/x\"}",
                field: "target.crate_types",
            },
        ];
        for case in cases {
            let input = stream(&[case.record, &FINISHED[..FINISHED.len() - 1]]);
            assert_eq!(
                select(&input),
                Err(ArtifactError::CargoSchema {
                    line: 1,
                    field: case.field,
                }),
                "{}",
                case.name
            );
        }

        for (name, executable) in [("null", "null"), ("bool", "false"), ("array", "[]")] {
            let record = ARTIFACT.replace(&format!("\"{EXECUTABLE}\""), executable);
            let input = stream(&[&record, &FINISHED[..FINISHED.len() - 1]]);
            assert_eq!(
                select(&input),
                Err(ArtifactError::CargoSchema {
                    line: 1,
                    field: "executable",
                }),
                "{name}"
            );
        }
    }

    #[test]
    fn rejects_missing_or_wrong_typed_reason() {
        for (name, record) in [
            ("missing", "{\"message\":\"compiler-artifact\"}"),
            ("null", "{\"reason\":null}"),
            ("boolean", "{\"reason\":true}"),
            ("array", "{\"reason\":[]}"),
        ] {
            let input = stream(&[record]);
            assert_eq!(
                select(&input),
                Err(ArtifactError::CargoSchema {
                    line: 1,
                    field: "reason",
                }),
                "{name}"
            );
        }
    }

    #[test]
    fn rejects_wrong_types_for_every_matching_target_field() {
        let cases = [
            (
                "kind_element",
                "\"kind\":[\"lib\"]",
                "\"kind\":[1]",
                "target.kind",
            ),
            (
                "crate_types",
                "\"crate_types\":[\"lib\"]",
                "\"crate_types\":\"lib\"",
                "target.crate_types",
            ),
            ("name", "\"name\":\"yaxml\"", "\"name\":null", "target.name"),
            (
                "src_path",
                "\"src_path\":\"/fixture/source/src/lib.rs\"",
                "\"src_path\":1",
                "target.src_path",
            ),
            (
                "edition",
                "\"edition\":\"2024\"",
                "\"edition\":false",
                "target.edition",
            ),
            ("doc", "\"doc\":true", "\"doc\":\"true\"", "target.doc"),
            (
                "doctest",
                "\"doctest\":true",
                "\"doctest\":null",
                "target.doctest",
            ),
            ("test", "\"test\":true", "\"test\":0", "target.test"),
        ];
        for (name, from, to, field) in cases {
            let changed = ARTIFACT.replacen(from, to, 1);
            let input = stream(&[&changed, &FINISHED[..FINISHED.len() - 1]]);
            assert_eq!(
                select(&input),
                Err(ArtifactError::CargoSchema { line: 1, field }),
                "{name}"
            );
        }

        let missing_executable = ARTIFACT.replace(
            ",\"executable\":\"/fixture/target/debug/deps/arbitrary test executable\"",
            "",
        );
        let input = stream(&[&missing_executable, &FINISHED[..FINISHED.len() - 1]]);
        assert_eq!(
            select(&input),
            Err(ArtifactError::CargoSchema {
                line: 1,
                field: "executable",
            })
        );
    }

    #[test]
    fn distinguishes_metadata_mismatch_from_missing_candidate() {
        let cases = [
            (
                "crate_types",
                "\"crate_types\":[\"lib\"]",
                "\"crate_types\":[\"rlib\"]",
                "target.crate_types",
            ),
            (
                "name",
                "\"name\":\"yaxml\"",
                "\"name\":\"other\"",
                "target.name",
            ),
            (
                "source",
                &format!("\"{SOURCE}\""),
                "\"/other/lib.rs\"",
                "target.src_path",
            ),
            ("edition", "\"2024\"", "\"2021\"", "target.edition"),
            ("doc", "\"doc\":true", "\"doc\":false", "target.doc"),
            (
                "doctest",
                "\"doctest\":true",
                "\"doctest\":false",
                "target.doctest",
            ),
            ("test", "\"test\":true", "\"test\":false", "target.test"),
        ];
        for (name, from, to, field) in cases {
            let changed = ARTIFACT.replacen(from, to, 1);
            let input = stream(&[&changed, &FINISHED[..FINISHED.len() - 1]]);
            assert_eq!(
                select(&input),
                Err(ArtifactError::TargetMetadata { line: 1, field }),
                "{name}"
            );
        }
    }

    #[test]
    fn external_and_in_band_build_status_are_separate_gates() {
        let positive = stream(&[ARTIFACT, &FINISHED[..FINISHED.len() - 1]]);
        for status in [
            CargoBuildStatus::NonZero,
            CargoBuildStatus::TimedOut,
            CargoBuildStatus::OutputLimit,
            CargoBuildStatus::SpawnFailure,
        ] {
            assert_eq!(
                select_artifact_record(&positive, status, &expected()),
                Err(ArtifactError::BuildStatus(status))
            );
        }

        let cases = [
            (
                "missing",
                stream(&[ARTIFACT]),
                ArtifactError::BuildFinished(BuildFinishedError::Missing),
            ),
            (
                "false",
                stream(&[
                    ARTIFACT,
                    "{\"reason\":\"build-finished\",\"success\":false}",
                ]),
                ArtifactError::BuildFinished(BuildFinishedError::False),
            ),
            (
                "duplicate",
                stream(&[
                    ARTIFACT,
                    &FINISHED[..FINISHED.len() - 1],
                    &FINISHED[..FINISHED.len() - 1],
                ]),
                ArtifactError::BuildFinished(BuildFinishedError::Duplicate),
            ),
            (
                "not_last",
                stream(&[&FINISHED[..FINISHED.len() - 1], "{\"reason\":\"future\"}"]),
                ArtifactError::BuildFinished(BuildFinishedError::NotLast),
            ),
        ];
        for (name, input, error) in cases {
            assert_eq!(select(&input), Err(error), "{name}");
        }
        for malformed in [
            "{\"reason\":\"build-finished\"}",
            "{\"reason\":\"build-finished\",\"success\":null}",
            "{\"reason\":\"build-finished\",\"success\":\"true\"}",
            "{\"reason\":\"build-finished\",\"success\":1}",
        ] {
            let input = stream(&[ARTIFACT, malformed]);
            assert_eq!(
                select(&input),
                Err(ArtifactError::CargoSchema {
                    line: 2,
                    field: "success",
                })
            );
        }
    }

    #[test]
    fn parses_complete_json_grammar_for_unknown_fields() {
        let records = [
            " { \"reason\" : \"future\", \"null\":null, \"bools\":[true,false], \"object\":{\"x\":\"y\"} } ",
            "{\"reason\":\"future\",\"strings\":[\"raw café漢\",\"\\u263a\",\"\\uD83D\\uDE00\",\"\\\"\\\\\\/\\b\\f\\n\\r\\t\"]}",
            "{\"reason\":\"future\",\"numbers\":[0,-0,1,-42,1.25,1e2,1E+2,-1.5e-2]}",
        ];
        for (index, record) in records.into_iter().enumerate() {
            let input = stream(&[record, &FINISHED[..FINISHED.len() - 1]]);
            assert_eq!(
                select(&input),
                Err(ArtifactError::NoMatchingArtifact),
                "record {index}"
            );
        }

        let crlf =
            b"{\"reason\":\"future\"}\r\n{\"reason\":\"build-finished\",\"success\":true}\r\n";
        assert_eq!(select(crlf), Err(ArtifactError::NoMatchingArtifact));
    }

    #[test]
    fn large_raw_unicode_field_has_linear_scalar_validation_work() {
        const ITERATIONS: usize = 8;
        let raw = "é𝄞".repeat(4_096);
        let record = format!("{{\"reason\":\"future\",\"payload\":\"{raw}\"}}");

        for _ in 0..ITERATIONS {
            let mut parser = JsonParser::new(record.as_bytes(), 1).unwrap();
            let object = parser.parse_root().unwrap();
            assert_eq!(object.get("payload"), Some(&JsonValue::String(raw.clone())));
            assert_eq!(parser.raw_utf8_validation_bytes, raw.len());
        }
    }

    #[test]
    fn rejects_json_layout_utf8_and_root_errors() {
        let cases: &[(&str, &[u8], ArtifactError)] = &[
            (
                "empty_record",
                b"\n",
                ArtifactError::StreamLayout(StreamLayoutKind::EmptyRecord),
            ),
            (
                "missing_final_lf",
                b"{\"reason\":\"future\"}",
                ArtifactError::StreamLayout(StreamLayoutKind::MissingFinalLf),
            ),
            (
                "bom",
                b"\xef\xbb\xbf{\"reason\":\"future\"}\n",
                ArtifactError::Json {
                    line: 1,
                    offset: 0,
                    kind: JsonErrorKind::UnexpectedByte,
                },
            ),
            (
                "array_root",
                b"[]\n",
                ArtifactError::Json {
                    line: 1,
                    offset: 2,
                    kind: JsonErrorKind::RootNotObject,
                },
            ),
            (
                "scalar_root",
                b"true\n",
                ArtifactError::Json {
                    line: 1,
                    offset: 4,
                    kind: JsonErrorKind::RootNotObject,
                },
            ),
            (
                "trailing",
                b"{\"reason\":\"future\"}x\n",
                ArtifactError::Json {
                    line: 1,
                    offset: 19,
                    kind: JsonErrorKind::TrailingBytes,
                },
            ),
            (
                "invalid_utf8",
                b"{\"reason\":\"future\",\"x\":\"\xff\"}\n",
                ArtifactError::Json {
                    line: 1,
                    offset: 0,
                    kind: JsonErrorKind::InvalidUtf8,
                },
            ),
        ];
        for (name, input, error) in cases {
            assert_eq!(select(input), Err(error.clone()), "{name}");
        }
    }

    #[test]
    fn rejects_bad_escapes_surrogates_and_string_controls() {
        let cases = [
            ("unknown_escape", "{\"reason\":\"future\",\"x\":\"\\q\"}"),
            ("short_unicode", "{\"reason\":\"future\",\"x\":\"\\u12\"}"),
            (
                "nonhex_unicode",
                "{\"reason\":\"future\",\"x\":\"\\u12xz\"}",
            ),
            ("lone_high", "{\"reason\":\"future\",\"x\":\"\\uD800\"}"),
            (
                "high_then_bmp",
                "{\"reason\":\"future\",\"x\":\"\\uD800\\u0041\"}",
            ),
            ("lone_low", "{\"reason\":\"future\",\"x\":\"\\uDC00\"}"),
            ("raw_tab", "{\"reason\":\"future\",\"x\":\"a\tb\"}"),
            (
                "raw_control",
                "{\"reason\":\"future\",\"x\":\"a\u{0001}b\"}",
            ),
        ];
        for (name, record) in cases {
            let input = stream(&[record]);
            assert!(
                matches!(select(&input), Err(ArtifactError::Json { .. })),
                "{name}"
            );
        }
    }

    #[test]
    fn rejects_number_grammar_faults() {
        let cases = ["+1", "01", "-", "1.", "1e", "1e+", "NaN", "Inf"];
        for number in cases {
            let record = format!("{{\"reason\":\"future\",\"n\":{number}}}");
            let input = stream(&[&record]);
            assert!(
                matches!(select(&input), Err(ArtifactError::Json { .. })),
                "{number}"
            );
        }
    }

    #[test]
    fn rejects_structural_errors_and_all_duplicate_keys() {
        let malformed = [
            "{\"reason\" \"future\"}",
            "{\"reason\":}",
            "{\"reason\":\"future\",}",
            "{\"reason\":\"future\"",
            "{\"reason\":\"future\" \"x\":1}",
            "{\"reason\":\"future\",\"x\":[1,]}",
            "{\"reason\":\"future\",\"x\":[1 2]}",
        ];
        for record in malformed {
            let input = stream(&[record]);
            assert!(matches!(select(&input), Err(ArtifactError::Json { .. })));
        }

        let duplicates = [
            "{\"reason\":\"future\",\"reason\":\"again\"}",
            "{\"reason\":\"future\",\"x\":{\"a\":1,\"a\":2}}",
            "{\"reason\":\"future\",\"x\":1,\"\\u0078\":2}",
        ];
        for record in duplicates {
            let input = stream(&[record]);
            assert!(matches!(
                select(&input),
                Err(ArtifactError::Json {
                    kind: JsonErrorKind::DuplicateKey,
                    ..
                })
            ));
        }
    }

    #[test]
    fn every_truncated_build_finished_prefix_is_rejected() {
        const COMPLETE: &[u8] = b"{\"reason\":\"build-finished\",\"success\":true}\n";
        assert_eq!(select(COMPLETE), Err(ArtifactError::NoMatchingArtifact));
        for end in 0..COMPLETE.len() {
            assert!(select(&COMPLETE[..end]).is_err(), "accepted prefix {end}");
        }
    }

    fn padded_unknown_line(length: usize) -> Vec<u8> {
        const PREFIX: &[u8] = b"{\"reason\":\"future\"}";
        assert!(length >= PREFIX.len());
        let mut line = PREFIX.to_vec();
        line.resize(length, b' ');
        line
    }

    #[test]
    fn enforces_total_and_per_line_byte_bounds() {
        let exact_line = padded_unknown_line(MAX_CARGO_JSON_LINE_BYTES);
        let mut input = exact_line.clone();
        input.push(b'\n');
        input.extend_from_slice(FINISHED.as_bytes());
        assert_eq!(select(&input), Err(ArtifactError::NoMatchingArtifact));

        let over_line = padded_unknown_line(MAX_CARGO_JSON_LINE_BYTES + 1);
        let mut input = over_line;
        input.push(b'\n');
        assert_eq!(
            select(&input),
            Err(ArtifactError::StreamLimit(StreamLimitKind::LineBytes))
        );

        let mut exact_stream = Vec::with_capacity(MAX_CARGO_JSONL_BYTES);
        for _ in 0..7 {
            exact_stream.extend_from_slice(&exact_line);
            exact_stream.push(b'\n');
        }
        let final_length = MAX_CARGO_JSONL_BYTES - exact_stream.len() - 1;
        let final_line = padded_unknown_line(final_length);
        exact_stream.extend_from_slice(&final_line);
        exact_stream.push(b'\n');
        assert_eq!(exact_stream.len(), MAX_CARGO_JSONL_BYTES);
        assert_eq!(
            select(&exact_stream),
            Err(ArtifactError::BuildFinished(BuildFinishedError::Missing))
        );
        exact_stream.insert(exact_stream.len() - 1, b' ');
        assert_eq!(
            select(&exact_stream),
            Err(ArtifactError::StreamLimit(StreamLimitKind::TotalBytes))
        );
    }

    #[test]
    fn enforces_record_count_bound() {
        let mut exact = Vec::new();
        for _ in 0..(MAX_CARGO_JSON_RECORDS - 1) {
            exact.extend_from_slice(b"{\"reason\":\"future\"}\n");
        }
        exact.extend_from_slice(FINISHED.as_bytes());
        assert_eq!(select(&exact), Err(ArtifactError::NoMatchingArtifact));

        let mut over = Vec::new();
        for _ in 0..MAX_CARGO_JSON_RECORDS {
            over.extend_from_slice(b"{\"reason\":\"future\"}\n");
        }
        over.extend_from_slice(FINISHED.as_bytes());
        assert_eq!(
            select(&over),
            Err(ArtifactError::StreamLimit(StreamLimitKind::Records))
        );
    }

    fn nested_record(array_count: usize) -> String {
        let mut record = String::from("{\"reason\":\"future\",\"value\":");
        record.extend(std::iter::repeat_n('[', array_count));
        record.push_str("null");
        record.extend(std::iter::repeat_n(']', array_count));
        record.push('}');
        record
    }

    #[test]
    fn enforces_depth_bound() {
        // Root object + 30 arrays + scalar = depth 32.
        let exact = nested_record(MAX_JSON_DEPTH - 2);
        let input = stream(&[&exact, &FINISHED[..FINISHED.len() - 1]]);
        assert_eq!(select(&input), Err(ArtifactError::NoMatchingArtifact));

        let over = nested_record(MAX_JSON_DEPTH - 1);
        let input = stream(&[&over]);
        assert!(matches!(
            select(&input),
            Err(ArtifactError::Json {
                kind: JsonErrorKind::Limit(JsonLimitKind::Depth),
                ..
            })
        ));
    }

    fn node_record(last_array_elements: usize) -> String {
        let mut record = String::from("{\"reason\":\"future\",\"value\":[");
        for array in 0..16 {
            if array != 0 {
                record.push(',');
            }
            record.push('[');
            let elements = if array == 15 {
                last_array_elements
            } else {
                MAX_JSON_ARRAY_ELEMENTS
            };
            for element in 0..elements {
                if element != 0 {
                    record.push(',');
                }
                record.push_str("null");
            }
            record.push(']');
        }
        record.push_str("]}");
        record
    }

    #[test]
    fn enforces_node_bound() {
        // root + reason string + outer array + 16 inner arrays + 65_517 scalars
        let exact = node_record(4_077);
        assert_eq!(
            JsonParser::new(exact.as_bytes(), 1)
                .unwrap()
                .parse_root()
                .unwrap()
                .get("reason"),
            Some(&JsonValue::String("future".to_owned()))
        );
        let over = node_record(4_078);
        assert!(matches!(
            JsonParser::new(over.as_bytes(), 1).unwrap().parse_root(),
            Err(ArtifactError::Json {
                kind: JsonErrorKind::Limit(JsonLimitKind::Nodes),
                ..
            })
        ));
    }

    fn object_member_record(extra_members: usize) -> String {
        let mut record = String::from("{\"reason\":\"future\"");
        for index in 0..extra_members {
            record.push_str(&format!(",\"k{index}\":null"));
        }
        record.push('}');
        record
    }

    #[test]
    fn enforces_object_member_bound() {
        let exact = object_member_record(MAX_JSON_OBJECT_MEMBERS - 1);
        assert!(
            JsonParser::new(exact.as_bytes(), 1)
                .unwrap()
                .parse_root()
                .is_ok()
        );
        let over = object_member_record(MAX_JSON_OBJECT_MEMBERS);
        assert!(matches!(
            JsonParser::new(over.as_bytes(), 1).unwrap().parse_root(),
            Err(ArtifactError::Json {
                kind: JsonErrorKind::Limit(JsonLimitKind::ObjectMembers),
                ..
            })
        ));
    }

    fn array_record(elements: usize) -> String {
        let values = std::iter::repeat_n("null", elements)
            .collect::<Vec<_>>()
            .join(",");
        format!("{{\"reason\":\"future\",\"values\":[{values}]}}")
    }

    #[test]
    fn enforces_array_element_bound() {
        let exact = array_record(MAX_JSON_ARRAY_ELEMENTS);
        assert!(
            JsonParser::new(exact.as_bytes(), 1)
                .unwrap()
                .parse_root()
                .is_ok()
        );
        let over = array_record(MAX_JSON_ARRAY_ELEMENTS + 1);
        assert!(matches!(
            JsonParser::new(over.as_bytes(), 1).unwrap().parse_root(),
            Err(ArtifactError::Json {
                kind: JsonErrorKind::Limit(JsonLimitKind::ArrayElements),
                ..
            })
        ));
    }

    #[test]
    fn enforces_decoded_key_and_string_bounds() {
        let exact_key = "k".repeat(MAX_JSON_KEY_BYTES);
        let exact = format!("{{\"reason\":\"future\",\"{exact_key}\":null}}");
        assert!(
            JsonParser::new(exact.as_bytes(), 1)
                .unwrap()
                .parse_root()
                .is_ok()
        );
        let over_key = "k".repeat(MAX_JSON_KEY_BYTES + 1);
        let over = format!("{{\"reason\":\"future\",\"{over_key}\":null}}");
        assert!(matches!(
            JsonParser::new(over.as_bytes(), 1).unwrap().parse_root(),
            Err(ArtifactError::Json {
                kind: JsonErrorKind::Limit(JsonLimitKind::KeyBytes),
                ..
            })
        ));

        let exact_string = format!("\"{}\"", "x".repeat(MAX_JSON_STRING_BYTES));
        assert!(matches!(
            JsonParser::new(exact_string.as_bytes(), 1)
                .unwrap()
                .parse_root(),
            Err(ArtifactError::Json {
                kind: JsonErrorKind::RootNotObject,
                ..
            })
        ));
        let over_string = format!("\"{}\"", "x".repeat(MAX_JSON_STRING_BYTES + 1));
        assert!(matches!(
            JsonParser::new(over_string.as_bytes(), 1)
                .unwrap()
                .parse_root(),
            Err(ArtifactError::Json {
                kind: JsonErrorKind::Limit(JsonLimitKind::StringBytes),
                ..
            })
        ));
    }

    #[test]
    fn enforces_semantic_string_and_list_bounds() {
        let mut expected_target = expected();
        let package = "p".repeat(MAX_PACKAGE_ID_BYTES);
        expected_target.package_id = package.clone();
        let artifact = ARTIFACT.replace(PACKAGE, &package);
        let input = stream(&[&artifact, &FINISHED[..FINISHED.len() - 1]]);
        assert!(
            select_artifact_record(&input, CargoBuildStatus::Success, &expected_target).is_ok()
        );
        let package_over = "p".repeat(MAX_PACKAGE_ID_BYTES + 1);
        let artifact = ARTIFACT.replace(PACKAGE, &package_over);
        let input = stream(&[&artifact, &FINISHED[..FINISHED.len() - 1]]);
        assert_eq!(
            select(&input),
            Err(ArtifactError::CargoSchema {
                line: 1,
                field: "package_id",
            })
        );

        let name = "n".repeat(MAX_TARGET_FIELD_BYTES);
        let mut expected_target = expected();
        expected_target.target_name = name.clone();
        let artifact = ARTIFACT.replace("\"name\":\"yaxml\"", &format!("\"name\":\"{name}\""));
        let input = stream(&[&artifact, &FINISHED[..FINISHED.len() - 1]]);
        assert!(
            select_artifact_record(&input, CargoBuildStatus::Success, &expected_target).is_ok()
        );
        let name_over = "n".repeat(MAX_TARGET_FIELD_BYTES + 1);
        let artifact = ARTIFACT.replace("\"name\":\"yaxml\"", &format!("\"name\":\"{name_over}\""));
        let input = stream(&[&artifact, &FINISHED[..FINISHED.len() - 1]]);
        assert_eq!(
            select(&input),
            Err(ArtifactError::CargoSchema {
                line: 1,
                field: "target.name",
            })
        );

        let path = format!("/{}", "s".repeat(MAX_PATH_FIELD_BYTES - 1));
        let mut expected_target = expected();
        expected_target.canonical_src_path = PathBuf::from(&path);
        let artifact = ARTIFACT.replace(SOURCE, &path);
        let input = stream(&[&artifact, &FINISHED[..FINISHED.len() - 1]]);
        assert!(
            select_artifact_record(&input, CargoBuildStatus::Success, &expected_target).is_ok()
        );
        let path_over = format!("/{}", "s".repeat(MAX_PATH_FIELD_BYTES));
        let artifact = ARTIFACT.replace(SOURCE, &path_over);
        let input = stream(&[&artifact, &FINISHED[..FINISHED.len() - 1]]);
        assert_eq!(
            select(&input),
            Err(ArtifactError::CargoSchema {
                line: 1,
                field: "target.src_path",
            })
        );

        let executable = format!("/{}", "e".repeat(MAX_PATH_FIELD_BYTES - 1));
        let artifact = ARTIFACT.replace(EXECUTABLE, &executable);
        let input = stream(&[&artifact, &FINISHED[..FINISHED.len() - 1]]);
        assert_eq!(select(&input).unwrap().executable, executable);
        let executable_over = format!("/{}", "e".repeat(MAX_PATH_FIELD_BYTES));
        let artifact = ARTIFACT.replace(EXECUTABLE, &executable_over);
        let input = stream(&[&artifact, &FINISHED[..FINISHED.len() - 1]]);
        assert_eq!(
            select(&input),
            Err(ArtifactError::CargoSchema {
                line: 1,
                field: "executable",
            })
        );

        let kinds = std::iter::repeat_n("\"lib\"", MAX_TARGET_KINDS)
            .collect::<Vec<_>>()
            .join(",");
        let mut expected_target = expected();
        expected_target.target_kind = vec!["lib".to_owned(); MAX_TARGET_KINDS];
        let artifact = ARTIFACT.replacen("\"kind\":[\"lib\"]", &format!("\"kind\":[{kinds}]"), 1);
        let input = stream(&[&artifact, &FINISHED[..FINISHED.len() - 1]]);
        assert!(
            select_artifact_record(&input, CargoBuildStatus::Success, &expected_target).is_ok()
        );
        let kinds_over = std::iter::repeat_n("\"lib\"", MAX_TARGET_KINDS + 1)
            .collect::<Vec<_>>()
            .join(",");
        let artifact =
            ARTIFACT.replacen("\"kind\":[\"lib\"]", &format!("\"kind\":[{kinds_over}]"), 1);
        let input = stream(&[&artifact, &FINISHED[..FINISHED.len() - 1]]);
        assert_eq!(
            select(&input),
            Err(ArtifactError::CargoSchema {
                line: 1,
                field: "target.kind",
            })
        );

        let crate_types = std::iter::repeat_n("\"lib\"", MAX_CRATE_TYPES)
            .collect::<Vec<_>>()
            .join(",");
        let mut expected_target = expected();
        expected_target.crate_types = vec!["lib".to_owned(); MAX_CRATE_TYPES];
        let artifact = ARTIFACT.replace(
            "\"crate_types\":[\"lib\"]",
            &format!("\"crate_types\":[{crate_types}]"),
        );
        let input = stream(&[&artifact, &FINISHED[..FINISHED.len() - 1]]);
        assert!(
            select_artifact_record(&input, CargoBuildStatus::Success, &expected_target).is_ok()
        );
        let crate_types_over = std::iter::repeat_n("\"lib\"", MAX_CRATE_TYPES + 1)
            .collect::<Vec<_>>()
            .join(",");
        let artifact = ARTIFACT.replace(
            "\"crate_types\":[\"lib\"]",
            &format!("\"crate_types\":[{crate_types_over}]"),
        );
        let input = stream(&[&artifact, &FINISHED[..FINISHED.len() - 1]]);
        assert_eq!(
            select(&input),
            Err(ArtifactError::CargoSchema {
                line: 1,
                field: "target.crate_types",
            })
        );
    }

    #[test]
    fn rejects_control_characters_in_selection_identity_fields() {
        for (from, to, field) in [
            (PACKAGE, "bad\\u0000package", "package_id"),
            (
                "\"name\":\"yaxml\"",
                "\"name\":\"bad\\u007fname\"",
                "target.name",
            ),
            (SOURCE, "/bad\\nsource", "target.src_path"),
            (EXECUTABLE, "/bad\\texecutable", "executable"),
        ] {
            let artifact = ARTIFACT.replace(from, to);
            let input = stream(&[&artifact, &FINISHED[..FINISHED.len() - 1]]);
            assert_eq!(
                select(&input),
                Err(ArtifactError::CargoSchema { line: 1, field })
            );
        }
    }
}
