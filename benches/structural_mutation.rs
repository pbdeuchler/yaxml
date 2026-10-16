//! Structural owned-DOM phases are timed independently. Fixture generation and
//! transport encoding precede every clock. Setup parses and selects handles;
//! edit measures public structural calls; traversal measures complete live
//! public views; serialization uses a counting sink; validation checks handles,
//! serializes to bytes and reparses; drop destroys only retained benchmark state.
//!
//! Debug defaults are a bounded `cargo test --all-targets` smoke matrix. Release
//! defaults are yaxml-only directional evidence. `reference_model` names the
//! libxml2 lifecycle a future C helper must match; no external timing or speed
//! claim is emitted by this benchmark.

#[path = "support/structural_mutation.rs"]
mod support;

use std::collections::HashSet;
use std::hint::black_box;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use support::{CaseKind, MatchedRequest, OperationCounts, removal_order};
use yaxml::{
    EditError, EditLimits, Limits, OwnedDocument, OwnedNodeId, OwnedNodeKind, Reader, Writer,
    WriterLimits,
};

const ENCODINGS: &[&str] = &["utf8", "utf16le", "utf16be"];
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

fn parameter(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

fn number(name: &str, fallback: usize) -> usize {
    parameter(name).map_or(fallback, |value| {
        value.parse().expect("integer benchmark parameter")
    })
}

fn numbers(name: &str, defaults: &[usize]) -> Vec<usize> {
    parameter(name).map_or_else(
        || defaults.to_vec(),
        |value| vec![value.parse().expect("integer benchmark parameter")],
    )
}

fn selected(name: &str, value: &str) -> bool {
    parameter(name).is_none_or(|choice| choice == value)
}

fn encode(xml: &str, encoding: &str) -> Vec<u8> {
    if encoding == "utf8" {
        return xml.as_bytes().to_vec();
    }
    let little = encoding == "utf16le";
    let mut bytes = if little {
        vec![0xff, 0xfe]
    } else {
        vec![0xfe, 0xff]
    };
    for unit in xml.encode_utf16() {
        bytes.extend_from_slice(&if little {
            unit.to_le_bytes()
        } else {
            unit.to_be_bytes()
        });
    }
    bytes
}

fn shallow_xml(width: usize, depth: usize, payload: &str) -> String {
    let item = format!("<n>{payload}</n>");
    format!(
        "{}{}{}",
        "<r>".repeat(depth),
        item.repeat(width),
        "</r>".repeat(depth)
    )
}

fn deep_xml(width: usize, depth: usize, payload: &str) -> String {
    let branch = format!("{}{}{}", "<n>".repeat(depth), payload, "</n>".repeat(depth));
    format!("<r>{}</r>", branch.repeat(width))
}

#[derive(Clone)]
struct RemovalTarget {
    root: OwnedNodeId,
    stale: Vec<OwnedNodeId>,
}

#[derive(Clone, Copy)]
struct LiveLeaf {
    id: OwnedNodeId,
    kind: OwnedNodeKind,
}

#[derive(Default)]
struct CountingSink {
    bytes: usize,
    writes: usize,
    flushes: usize,
}

impl Write for CountingSink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes = self.bytes.checked_add(bytes.len()).unwrap();
        self.writes = self.writes.checked_add(1).unwrap();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flushes = self.flushes.checked_add(1).unwrap();
        Ok(())
    }
}

struct Config {
    iterations: usize,
    operations: usize,
    traversal_passes: usize,
    serialization_passes: usize,
    edit: EditLimits,
}

fn deepest_parent(document: &OwnedDocument, depth: usize) -> OwnedNodeId {
    let mut node = document.root_element();
    for _ in 1..depth {
        node = node.first_child().expect("outer chain must continue");
    }
    node.id()
}

fn targets(
    document: &OwnedDocument,
    parent: OwnedNodeId,
    case: CaseKind,
    operations: usize,
) -> Vec<RemovalTarget> {
    let children: Vec<_> = document
        .node(parent)
        .unwrap()
        .children()
        .filter(|node| node.kind() == OwnedNodeKind::Element)
        .map(|node| RemovalTarget {
            root: node.id(),
            stale: node
                .descendants()
                .map(|descendant| descendant.id())
                .collect(),
        })
        .collect();
    removal_order(children.len(), case)
        .into_iter()
        .take(operations)
        .map(|index| children[index].clone())
        .collect()
}

fn kind_code(kind: OwnedNodeKind) -> usize {
    match kind {
        OwnedNodeKind::Document => 1,
        OwnedNodeKind::Element => 2,
        OwnedNodeKind::Text => 3,
        OwnedNodeKind::Cdata => 4,
        OwnedNodeKind::Comment => 5,
        OwnedNodeKind::ProcessingInstruction => 6,
        _ => 7,
    }
}

fn mix(mut state: u64, value: usize) -> u64 {
    state ^= value as u64;
    state.rotate_left(13).wrapping_mul(0x9e37_79b1_85eb_ca87)
}

fn traverse(document: &OwnedDocument) -> (usize, u64) {
    let mut count = 0;
    let mut checksum = 0xcbf2_9ce4_8422_2325;
    for node in document.nodes() {
        count += 1;
        checksum = mix(checksum, kind_code(node.kind()));
        if let Some(range) = node.range() {
            checksum = mix(checksum, range.start);
            checksum = mix(checksum, range.end);
        }
        if let Some(name) = node.name() {
            checksum = mix(checksum, name.qualified.len());
            checksum = mix(checksum, name.namespace.as_deref().map_or(0, str::len));
        }
        if let Some(text) = node.text() {
            checksum = mix(checksum, text.len());
            checksum = mix(
                checksum,
                text.as_bytes().first().copied().unwrap_or(0) as usize,
            );
        }
        for attribute in node.attributes() {
            checksum = mix(checksum, attribute.name.qualified.len());
            checksum = mix(checksum, attribute.value.len());
        }
    }
    (count, checksum)
}

fn append(
    editor: &mut yaxml::Editor<'_>,
    case: CaseKind,
    parent: OwnedNodeId,
    payload: &str,
) -> Result<LiveLeaf, EditError> {
    let (id, kind) = match case {
        CaseKind::AppendText | CaseKind::Churn => {
            (editor.append_text(parent, payload)?, OwnedNodeKind::Text)
        }
        CaseKind::AppendCdata => (editor.append_cdata(parent, payload)?, OwnedNodeKind::Cdata),
        CaseKind::AppendComment => (
            editor.append_comment(parent, payload)?,
            OwnedNodeKind::Comment,
        ),
        CaseKind::AppendPi => (
            editor.append_processing_instruction(parent, "bench", payload)?,
            OwnedNodeKind::ProcessingInstruction,
        ),
        _ => unreachable!("append helper requires an append case"),
    };
    Ok(LiveLeaf { id, kind })
}

fn fnv(bytes: &[u8], mut checksum: u64) -> u64 {
    for byte in bytes {
        checksum = (checksum ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
    }
    checksum
}

fn measure(
    case: CaseKind,
    encoding: &str,
    width: usize,
    depth: usize,
    payload: &str,
    config: &Config,
    export_path: Option<&Path>,
) {
    let xml = if case == CaseKind::RemoveDeep {
        deep_xml(width, depth, payload)
    } else {
        shallow_xml(width, depth, payload)
    };
    let input = encode(&xml, encoding);
    let max_depth = depth.checked_add(2).expect("depth overflow");
    let parse = Limits {
        max_depth,
        ..Limits::default()
    };
    let output = WriterLimits {
        max_depth,
        ..WriterLimits::default()
    };

    let mut setup = Duration::ZERO;
    let mut edit = Duration::ZERO;
    let mut traversal = Duration::ZERO;
    let mut serialization = Duration::ZERO;
    let mut validation = Duration::ZERO;
    let mut destruction = Duration::ZERO;
    let mut aggregate = OperationCounts::default();
    let mut expected_initial = None;
    let mut expected_final = None;
    let mut expected_slots = None;
    let mut expected_units = None;
    let mut expected_failure: Option<Option<EditError>> = None;
    let mut completed_units_total = 0_u128;
    let mut traversal_visits_total = 0_u128;
    let mut counted_output_bytes_total = 0_u128;
    let mut validation_output_bytes_total = 0_u128;
    let mut sink_writes_total = 0_u128;
    let mut checksum = 0xcbf2_9ce4_8422_2325;
    let mut output_hash = None;
    let mut exported_output = None;

    for _ in 0..config.iterations {
        let started = Instant::now();
        let mut document = match OwnedDocument::with_limits(black_box(&input), parse) {
            Ok(document) => document,
            Err(error) => {
                println!(
                    "benchmark=owned_structure schema=2 implementation=yaxml case={} encoding={encoding} width={width} depth={depth} payload_bytes={} input_bytes={} status=parse_rejected error_kind={:?} error_offset={}",
                    case.label(),
                    payload.len(),
                    input.len(),
                    error.kind,
                    error.offset
                );
                return;
            }
        };
        let initial_live = document.nodes().len();
        let parent = if case == CaseKind::RemoveDeep {
            document.root_element().id()
        } else {
            deepest_parent(&document, depth)
        };
        let removal_targets = if case.is_removal() {
            targets(&document, parent, case, config.operations)
        } else {
            Vec::new()
        };
        let planned_units = if case.is_removal() {
            removal_targets.len()
        } else {
            config.operations
        };
        setup += started.elapsed();

        let started = Instant::now();
        let mut counts = OperationCounts::default();
        let mut completed_units = 0;
        let mut failure = None;
        let mut live = Vec::with_capacity(if case == CaseKind::Churn {
            0
        } else {
            planned_units
        });
        let mut stale = Vec::new();
        {
            let mut editor = document.edit(config.edit).unwrap();
            if case.is_removal() {
                for target in &removal_targets {
                    counts.request().expect("benchmark accounting overflow");
                    if let Err(error) = black_box(editor.remove_subtree(black_box(target.root))) {
                        failure = Some(error);
                        break;
                    }
                    counts
                        .complete_removal(target.stale.len())
                        .expect("benchmark accounting overflow");
                    stale.extend_from_slice(&target.stale);
                    completed_units += 1;
                }
            } else if case == CaseKind::Churn {
                for _ in 0..planned_units {
                    counts.request().expect("benchmark accounting overflow");
                    let leaf = match black_box(append(
                        &mut editor,
                        case,
                        black_box(parent),
                        black_box(payload),
                    )) {
                        Ok(leaf) => leaf,
                        Err(error) => {
                            failure = Some(error);
                            break;
                        }
                    };
                    counts
                        .complete_append()
                        .expect("benchmark accounting overflow");
                    counts.request().expect("benchmark accounting overflow");
                    if let Err(error) = black_box(editor.remove_subtree(black_box(leaf.id))) {
                        live.push(leaf);
                        failure = Some(error);
                        break;
                    }
                    counts
                        .complete_removal(1)
                        .expect("benchmark accounting overflow");
                    stale.push(leaf.id);
                    completed_units += 1;
                }
            } else {
                for _ in 0..planned_units {
                    counts.request().expect("benchmark accounting overflow");
                    match black_box(append(
                        &mut editor,
                        case,
                        black_box(parent),
                        black_box(payload),
                    )) {
                        Ok(leaf) => {
                            counts
                                .complete_append()
                                .expect("benchmark accounting overflow");
                            live.push(leaf);
                            completed_units += 1;
                        }
                        Err(error) => {
                            failure = Some(error);
                            break;
                        }
                    }
                }
            }
        }
        edit += started.elapsed();

        let final_live = counts
            .final_live(initial_live)
            .expect("benchmark accounting underflow");
        let slot_count = initial_live
            .checked_add(counts.created_nodes)
            .expect("benchmark accounting overflow");
        assert_eq!(*expected_initial.get_or_insert(initial_live), initial_live);
        assert_eq!(*expected_final.get_or_insert(final_live), final_live);
        assert_eq!(*expected_slots.get_or_insert(slot_count), slot_count);
        assert_eq!(*expected_units.get_or_insert(planned_units), planned_units);
        assert_eq!(*expected_failure.get_or_insert(failure), failure);

        let started = Instant::now();
        let mut visit_checksum = 0_u64;
        for _ in 0..config.traversal_passes {
            let (visits, pass_checksum) = black_box(traverse(black_box(&document)));
            assert_eq!(visits, final_live);
            traversal_visits_total = traversal_visits_total
                .checked_add(visits as u128)
                .expect("benchmark traversal accounting overflow");
            visit_checksum = visit_checksum.wrapping_add(pass_checksum);
        }
        black_box(visit_checksum);
        traversal += started.elapsed();

        let started = Instant::now();
        let mut counted_bytes = None;
        for _ in 0..config.serialization_passes {
            let mut writer = Writer::with_limits(CountingSink::default(), output);
            writer
                .write_owned_document(black_box(&document))
                .expect("structurally edited tree must serialize");
            let position = writer.position();
            let sink = writer.into_inner();
            assert_eq!(position, sink.bytes);
            assert_eq!(sink.flushes, 1);
            assert_eq!(*counted_bytes.get_or_insert(sink.bytes), sink.bytes);
            counted_output_bytes_total = counted_output_bytes_total
                .checked_add(sink.bytes as u128)
                .expect("benchmark output accounting overflow");
            sink_writes_total = sink_writes_total
                .checked_add(sink.writes as u128)
                .expect("benchmark sink accounting overflow");
            black_box(sink);
        }
        serialization += started.elapsed();

        let started = Instant::now();
        assert_eq!(document.nodes().len(), final_live);
        let mut unique = HashSet::new();
        for id in &stale {
            assert!(unique.insert(*id), "stale handle repeated");
            assert!(document.node(*id).is_none(), "removed handle remained live");
        }
        for leaf in &live {
            assert!(unique.insert(leaf.id), "created handle repeated");
            let node = document.node(leaf.id).expect("created handle became stale");
            assert_eq!(node.kind(), leaf.kind);
            assert_eq!(node.text(), Some(payload));
            if leaf.kind == OwnedNodeKind::ProcessingInstruction {
                assert_eq!(node.processing_instruction_target(), Some("bench"));
            }
            assert_eq!(node.range(), None);
        }
        let mut writer = Writer::with_limits(Vec::new(), output);
        writer
            .write_owned_document(&document)
            .expect("validation serialization must complete");
        let bytes = writer.into_inner();
        assert_eq!(Some(bytes.len()), counted_bytes);
        let mut reader = Reader::with_limits(&bytes, parse).expect("output must start parsing");
        while let Some(event) = reader.read_event().expect("output must finish parsing") {
            black_box(event);
        }
        assert_eq!(reader.position(), bytes.len());
        let row_output_hash = fnv(&bytes, FNV_OFFSET);
        assert_eq!(*output_hash.get_or_insert(row_output_hash), row_output_hash);
        checksum = fnv(&bytes, checksum ^ visit_checksum);
        validation_output_bytes_total = validation_output_bytes_total
            .checked_add(bytes.len() as u128)
            .expect("benchmark validation accounting overflow");
        validation += started.elapsed();

        aggregate.requested = aggregate
            .requested
            .checked_add(counts.requested)
            .expect("benchmark request accounting overflow");
        aggregate.completed = aggregate
            .completed
            .checked_add(counts.completed)
            .expect("benchmark completion accounting overflow");
        aggregate.created_nodes = aggregate
            .created_nodes
            .checked_add(counts.created_nodes)
            .expect("benchmark creation accounting overflow");
        aggregate.removed_nodes = aggregate
            .removed_nodes
            .checked_add(counts.removed_nodes)
            .expect("benchmark removal accounting overflow");
        completed_units_total = completed_units_total
            .checked_add(completed_units as u128)
            .expect("benchmark unit accounting overflow");

        drop(reader);
        if export_path.is_some() && exported_output.is_none() {
            exported_output = Some(bytes);
        } else {
            drop(bytes);
        }
        drop(removal_targets);
        drop(live);
        drop(stale);
        let started = Instant::now();
        drop(document);
        destruction += started.elapsed();
    }

    let initial_live = expected_initial.unwrap();
    let final_live = expected_final.unwrap();
    let slot_count = expected_slots.unwrap();
    let planned_units = expected_units.unwrap();
    let failure = expected_failure.unwrap();
    let status = if failure.is_some() {
        "edit_rejected"
    } else {
        "complete"
    };
    let error_kind = failure.map(|error| error.kind);
    let error_value_offset = failure.and_then(|error| error.value_offset);
    if let Some(path) = export_path {
        MatchedRequest {
            case,
            encoding,
            width,
            depth,
            payload: payload.as_bytes(),
            iterations: config.iterations,
            operations: config.operations,
            traversal_passes: config.traversal_passes,
            serialization_passes: config.serialization_passes,
            input: &input,
            expected: exported_output.as_deref().unwrap(),
        }
        .write_file(path)
        .expect("matched benchmark request export must complete");
    }
    let comparison_status = if export_path.is_some() {
        "matched_input"
    } else {
        "unmeasured"
    };
    println!(
        "benchmark=owned_structure schema=2 implementation=yaxml case={} encoding={encoding} reference_model={} comparison_status={comparison_status} width={width} depth={depth} payload_bytes={} source_utf8_bytes={} input_bytes={} iterations={} configured_units_per_iteration={} planned_units_per_iteration={planned_units} completed_units_total={completed_units_total} requested_edit_calls_total={} completed_edit_calls_total={} created_nodes_total={} merged_nodes_total=0 removed_nodes_total={} initial_live_nodes_per_iteration={initial_live} final_live_nodes_per_iteration={final_live} expected_slots_per_iteration={slot_count} traversal_passes={} traversal_visits_total={traversal_visits_total} serialization_passes={} counted_output_bytes_total={counted_output_bytes_total} validation_output_bytes_total={validation_output_bytes_total} sink_writes_total={sink_writes_total} max_value_bytes={} max_owned_bytes={} max_created_nodes={} status={status} error_kind={error_kind:?} error_value_offset={error_value_offset:?} setup_ns={} edit_ns={} traversal_ns={} serialization_ns={} validation_ns={} drop_ns={} output_hash={:016x} checksum={checksum:016x} edit_calls_per_s={:.2}",
        case.label(),
        case.reference_model(),
        payload.len(),
        xml.len(),
        input.len(),
        config.iterations,
        config.operations,
        aggregate.requested,
        aggregate.completed,
        aggregate.created_nodes,
        aggregate.removed_nodes,
        config.traversal_passes,
        config.serialization_passes,
        config.edit.max_value_bytes,
        config.edit.max_owned_bytes,
        config.edit.max_created_nodes,
        setup.as_nanos(),
        edit.as_nanos(),
        traversal.as_nanos(),
        serialization.as_nanos(),
        validation.as_nanos(),
        destruction.as_nanos(),
        output_hash.unwrap(),
        aggregate.completed as f64 / edit.as_secs_f64()
    );
}

fn main() {
    let export_path = parameter("YAXML_STRUCTURE_EXPORT").map(PathBuf::from);
    let selected_case = parameter("YAXML_STRUCTURE_CASE").map(|value| {
        CaseKind::parse(&value).unwrap_or_else(|| panic!("unknown structural benchmark case"))
    });
    if let Some(encoding) = parameter("YAXML_STRUCTURE_ENCODING") {
        assert!(
            ENCODINGS.contains(&encoding.as_str()),
            "unknown structural benchmark encoding"
        );
    }
    let debug = cfg!(debug_assertions);
    let widths = numbers(
        "YAXML_STRUCTURE_WIDTH",
        if debug { &[4] } else { &[16, 256] },
    );
    let depths = numbers("YAXML_STRUCTURE_DEPTH", if debug { &[4] } else { &[1, 64] });
    let payloads = numbers(
        "YAXML_STRUCTURE_PAYLOAD_BYTES",
        if debug { &[0, 8] } else { &[32, 1024] },
    );
    let config = Config {
        iterations: number("YAXML_STRUCTURE_ITERATIONS", if debug { 1 } else { 5 }),
        operations: number("YAXML_STRUCTURE_OPERATIONS", if debug { 4 } else { 1_000 }),
        traversal_passes: number(
            "YAXML_STRUCTURE_TRAVERSAL_PASSES",
            if debug { 1 } else { 16 },
        ),
        serialization_passes: number(
            "YAXML_STRUCTURE_SERIALIZATION_PASSES",
            if debug { 1 } else { 4 },
        ),
        edit: EditLimits {
            max_value_bytes: number(
                "YAXML_STRUCTURE_MAX_VALUE_BYTES",
                EditLimits::default().max_value_bytes,
            ),
            max_owned_bytes: number(
                "YAXML_STRUCTURE_MAX_OWNED_BYTES",
                EditLimits::default().max_owned_bytes,
            ),
            max_created_nodes: number(
                "YAXML_STRUCTURE_MAX_CREATED_NODES",
                EditLimits::default().max_created_nodes,
            ),
        },
    };
    if export_path.is_some() {
        for required in [
            "YAXML_STRUCTURE_CASE",
            "YAXML_STRUCTURE_ENCODING",
            "YAXML_STRUCTURE_WIDTH",
            "YAXML_STRUCTURE_DEPTH",
            "YAXML_STRUCTURE_PAYLOAD_BYTES",
        ] {
            assert!(
                parameter(required).is_some(),
                "{required} is required for export"
            );
        }
    }
    assert!(
        config.iterations > 0
            && config.operations > 0
            && config.traversal_passes > 0
            && config.serialization_passes > 0
    );
    for width in &widths {
        assert!(*width > 0);
    }
    for depth in &depths {
        assert!(*depth > 0);
    }
    for case in CaseKind::ALL {
        if selected_case.is_some_and(|selected| selected != case) {
            continue;
        }
        for width in &widths {
            for depth in &depths {
                for payload_bytes in &payloads {
                    let payload = "x".repeat(*payload_bytes);
                    for encoding in ENCODINGS {
                        if selected("YAXML_STRUCTURE_ENCODING", encoding) {
                            measure(
                                case,
                                encoding,
                                *width,
                                *depth,
                                &payload,
                                &config,
                                export_path.as_deref(),
                            );
                        }
                    }
                }
            }
        }
    }
}
