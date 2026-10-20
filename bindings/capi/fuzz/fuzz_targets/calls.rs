//! Random call sequences through the C ABI, as a C caller would make them:
//! NULL out-parameters and errors, stale, freed, foreign and invented
//! handles, forged nodes, double frees of every allocation (including the
//! messages error paths return), invalid UTF-8, and callbacks that stop
//! parsing or fail writes. Under ASan any bad pointer the library hands out
//! is caught when the callbacks and this driver read it. After each input
//! every handle and allocation is freed and the library's tables must be
//! empty.
#![no_main]

use std::ffi::c_void;
use std::ptr::{null, null_mut};

use yaxml_capi::types::*;
use yaxml_capi::*;

const DOCUMENTS: [&str; 8] = [
    "<r a='1' xmlns:p='urn:p'><p:a p:k='v'>text<b/><!--c--><?pi d?></p:a><![CDATA[x]]></r>",
    "<!DOCTYPE r [<!ELEMENT r (a)*><!ELEMENT a EMPTY><!ATTLIST a id ID #IMPLIED><!ENTITY e 'ent'>]><r><a id='i'/>&e;</r>",
    "<x:root xmlns:x='urn:x' xmlns='urn:d'><child x:attr='1'/><child/></x:root>",
    "<r>5</r>",
    "<?xml version='1.0' encoding='UTF-8' standalone='yes'?>\n<r>\n  <i ok='1'/>\n  <i/>\n</r>",
    "<a><b><c><d>deep</d></c></b></a>",
    "<r xml:id='top' xml:lang='en' xml:space='preserve'> s </r>",
    "<broken><r>",
];

const SCHEMAS: [(i32, &str); 3] = [
    (1, "<xs:schema xmlns:xs='http://www.w3.org/2001/XMLSchema'><xs:element name='r' type='xs:int'/></xs:schema>"),
    (2, "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><zeroOrMore><element><anyName/><empty/></element></zeroOrMore></element>"),
    (3, "<schema xmlns='http://purl.oclc.org/dsdl/schematron'><pattern id='p'><rule context='i'><assert test='@ok'>no ok</assert></rule></pattern></schema>"),
];

const EXPRESSIONS: [&str; 8] = ["//*", "//@*", "//namespace::*", "count(//*)", "string(.)", "//p:*", "boolean(/r)", "'unterminated"];

struct Input<'a>(&'a [u8]);

impl<'a> Input<'a> {
    fn byte(&mut self) -> u8 {
        let (&first, rest) = self.0.split_first().unwrap_or((&0, &[]));
        self.0 = rest;
        first
    }

    fn below(&mut self, n: usize) -> usize {
        usize::from(self.byte()) % n.max(1)
    }

    fn bytes(&mut self) -> &'a [u8] {
        let n = self.below(64).min(self.0.len());
        let (taken, rest) = self.0.split_at(n);
        self.0 = rest;
        taken
    }

    /// A string argument: NULL, empty, fuzz bytes (maybe not UTF-8) or a name.
    fn string(&mut self) -> yaxml_str {
        match self.byte() % 6 {
            0 => yaxml_str::NULL,
            1 => yaxml_str::of(""),
            2 => {
                let b = self.bytes();
                yaxml_str { ptr: b.as_ptr(), len: b.len() }
            }
            3 => yaxml_str::of("p"),
            4 => yaxml_str::of("urn:p"),
            _ => yaxml_str::of("a"),
        }
    }
}

#[derive(Default)]
struct State {
    docs: Vec<*mut yaxml_doc>,
    nodes: Vec<yaxml_node>,
    items: Vec<yaxml_xpath_item>,
    schemas: Vec<*mut yaxml_schema>,
    readers: Vec<*mut yaxml_reader>,
    pushers: Vec<*mut yaxml_push>,
    allocations: Vec<*const c_void>,
}

impl State {
    fn pick<T: Copy>(list: &[T], input: &mut Input<'_>, fallback: T) -> T {
        match list.len() {
            0 => fallback,
            n => list[input.below(n + 1).min(n - 1)],
        }
    }

    fn doc(&self, input: &mut Input<'_>) -> *mut yaxml_doc {
        match input.byte() % 16 {
            0 => null_mut(),
            1 => std::ptr::without_provenance_mut(0x10_0000 + usize::from(input.byte())),
            _ => Self::pick(&self.docs, input, null_mut()),
        }
    }

    fn node(&self, input: &mut Input<'_>) -> yaxml_node {
        let node = Self::pick(&self.nodes, input, yaxml_node::default());
        match input.byte() % 16 {
            0 => yaxml_node::default(),
            1 => yaxml_node { slot: node.slot.wrapping_add(u64::from(input.byte())), ..node },
            2 => yaxml_node { doc: node.doc.wrapping_add(1), ..node },
            _ => node,
        }
    }

    fn keep_error(&mut self, err: &yaxml_error, status: i32) {
        assert!((-12..=1).contains(&status), "status {status}");
        if status < 0 {
            assert_eq!(err.status, status);
            touch(err.diagnostic.message);
            self.allocations.push(err.diagnostic.message.ptr.cast());
        }
    }
}

/// Reads every byte of a string the library handed out (ASan checks it).
fn touch(s: yaxml_str) -> usize {
    if s.ptr.is_null() {
        assert_eq!(s.len, 0);
        return 0;
    }
    // SAFETY: the library promises `len` readable bytes.
    let bytes = unsafe { std::slice::from_raw_parts(s.ptr, s.len) };
    std::hint::black_box(bytes.iter().map(|&b| usize::from(b)).sum::<usize>() + usize::from(std::str::from_utf8(bytes).is_ok()))
}

fn touch_name(n: &yaxml_name) -> usize {
    touch(n.qname) + touch(n.prefix) + touch(n.local) + touch(n.uri)
}

fn touch_event(e: &yaxml_event) -> usize {
    let mut sum = touch_name(&e.name) + touch(e.value) + touch(e.target) + touch(e.system_id) + touch(e.internal_subset);
    if e.attribute_count > 0 {
        // SAFETY: the event lends `attribute_count` attributes.
        let attributes = unsafe { std::slice::from_raw_parts(e.attributes, e.attribute_count) };
        sum += attributes.iter().map(|a| touch_name(&a.name) + touch(a.value)).sum::<usize>();
    }
    sum
}

extern "C" fn on_event(user: *mut c_void, event: *const yaxml_event) -> i32 {
    // SAFETY: the driver passes a u8 countdown; the library a live event.
    let (countdown, event) = unsafe { (&mut *user.cast::<u8>(), &*event) };
    std::hint::black_box(touch_event(event));
    *countdown = countdown.saturating_sub(1);
    i32::from(*countdown == 0)
}

extern "C" fn on_write(user: *mut c_void, bytes: *const u8, len: usize) -> i32 {
    // SAFETY: as above; the library passes `len` bytes.
    let (countdown, bytes) = unsafe { (&mut *user.cast::<u8>(), std::slice::from_raw_parts(bytes, len)) };
    std::hint::black_box(bytes.iter().map(|&b| usize::from(b)).sum::<usize>());
    *countdown = countdown.saturating_sub(1);
    i32::from(*countdown == 0)
}

fn options(input: &mut Input<'_>) -> yaxml_parse_options {
    let flags = [0, 1, 2, 1 << 4, 1 << 8, 1 << 14, 1 << 13, 1 << 17, 1 | 2 | (1 << 3)];
    yaxml_parse_options {
        struct_size: if input.byte() % 32 == 0 { 8 } else { size_of::<yaxml_parse_options>() as u32 },
        flags: flags[input.below(flags.len())],
        max_depth: u64::from(input.byte() % 4),
        max_events: if input.byte() % 8 == 0 { 3 } else { 0 },
        ..Default::default()
    }
}

/// One call; returns false when the input is used up.
fn step(state: &mut State, input: &mut Input<'_>) -> bool {
    if input.0.is_empty() {
        return false;
    }
    let mut err = yaxml_error::default();
    let errp = if input.byte() % 8 == 0 { null_mut() } else { &raw mut err };
    let null_out = input.byte() % 16 == 0;
    macro_rules! out {
        ($v:ident) => {
            if null_out { null_mut() } else { &raw mut $v }
        };
    }
    let op = input.byte() % 40;
    // SAFETY (all calls): pointers are NULL or point to live locals and
    // buffers; handles and nodes may be anything, which the ABI checks.
    let status = unsafe {
        match op {
            0..=3 => {
                let text = match input.byte() % 4 {
                    0 => input.bytes(),
                    _ => DOCUMENTS[input.below(DOCUMENTS.len())].as_bytes(),
                };
                let o = options(input);
                let optp = if input.byte() % 2 == 0 { null() } else { &raw const o };
                let mut doc = null_mut();
                let status = yaxml_parse(text.as_ptr(), text.len(), optp, out!(doc), errp);
                if status == OK {
                    state.docs.push(doc);
                    let mut root = yaxml_node::default();
                    assert_eq!(yaxml_doc_root(doc, &mut root, null_mut()), OK);
                    state.nodes.push(root);
                }
                status
            }
            4 => yaxml_doc_free(state.doc(input)),
            5 => {
                let mut node = yaxml_node::default();
                let status = match input.byte() % 3 {
                    0 => yaxml_doc_root(state.doc(input), out!(node), errp),
                    1 => yaxml_doc_element(state.doc(input), out!(node), errp),
                    _ => yaxml_doc_element_by_id(state.doc(input), input.string(), out!(node), errp),
                };
                if status == OK {
                    state.nodes.push(node);
                }
                status
            }
            6..=9 => {
                let mut node = yaxml_node::default();
                let relation = i32::from(input.byte() % 7);
                let status = yaxml_node_relative(state.doc(input), state.node(input), relation, out!(node), errp);
                if status == OK {
                    state.nodes.push(node);
                }
                status
            }
            10 => {
                let (mut kind, mut line, mut count) = (0, 0, 0);
                let (doc, node) = (state.doc(input), state.node(input));
                match input.byte() % 3 {
                    0 => yaxml_node_kind(doc, node, out!(kind), errp),
                    1 => yaxml_node_line(doc, node, out!(line), errp),
                    _ => yaxml_node_attribute_count(doc, node, out!(count), errp),
                }
            }
            11 => {
                let mut name = yaxml_name::default();
                let status = yaxml_node_name(state.doc(input), state.node(input), out!(name), errp);
                touch_name(&name);
                status
            }
            12 => {
                let mut value = yaxml_str::NULL;
                let (doc, node) = (state.doc(input), state.node(input));
                let status = match input.byte() % 5 {
                    0 => yaxml_node_value(doc, node, out!(value), errp),
                    1 => yaxml_node_attribute(doc, node, input.string(), out!(value), errp),
                    2 => yaxml_node_attribute_ns(doc, node, input.string(), input.string(), out!(value), errp),
                    3 => yaxml_node_namespace_uri(doc, node, input.string(), out!(value), errp),
                    _ => yaxml_node_namespace_prefix(doc, node, input.string(), out!(value), errp),
                };
                touch(value);
                status
            }
            13 => {
                let mut attribute = yaxml_attribute::default();
                let index = input.below(6);
                let status = yaxml_node_attribute_at(state.doc(input), state.node(input), index, out!(attribute), errp);
                std::hint::black_box(touch_name(&attribute.name) + touch(attribute.value));
                status
            }
            14 => {
                let mut buffer = yaxml_buffer::default();
                let (doc, node) = (state.doc(input), state.node(input));
                let status = match input.byte() % 2 {
                    0 => yaxml_node_content(doc, node, out!(buffer), errp),
                    _ => yaxml_node_path(doc, node, out!(buffer), errp),
                };
                touch(yaxml_str { ptr: buffer.ptr, len: buffer.len });
                state.allocations.push(buffer.ptr.cast());
                status
            }
            15 | 16 => {
                let mut value = yaxml_xpath_value::default();
                let expression = match input.byte() % 4 {
                    0 => input.string(),
                    _ => yaxml_str::of(EXPRESSIONS[input.below(EXPRESSIONS.len())]),
                };
                let ns = [yaxml_ns { prefix: input.string(), uri: input.string() }];
                let count = input.below(2);
                let status = yaxml_xpath_eval(state.doc(input), state.node(input), expression, ns.as_ptr(), count, out!(value), errp);
                if status == OK {
                    touch(yaxml_str { ptr: value.string.ptr, len: value.string.len });
                    if value.len > 0 {
                        let items = std::slice::from_raw_parts(value.items, value.len);
                        state.items.extend_from_slice(items);
                        state.nodes.extend(items.iter().map(|i| i.node));
                    }
                    state.allocations.push(value.string.ptr.cast());
                    state.allocations.push(value.items.cast());
                }
                status
            }
            17 => {
                let mut buffer = yaxml_buffer::default();
                let flags = [0, 1, 2, 4, 1 << 6, 1 << 20][input.below(6)];
                let encoding = match input.byte() % 4 {
                    0 => yaxml_str::of("ISO-8859-1"),
                    1 => yaxml_str::of("UTF-16"),
                    _ => yaxml_str::NULL,
                };
                let status = yaxml_save(state.doc(input), state.node(input), flags, encoding, out!(buffer), errp);
                touch(yaxml_str { ptr: buffer.ptr, len: buffer.len });
                state.allocations.push(buffer.ptr.cast());
                status
            }
            18 => {
                let mut countdown = input.byte();
                let callback = if input.byte() % 8 == 0 { None } else { Some(on_write as _) };
                let (doc, node) = (state.doc(input), state.node(input));
                yaxml_save_to(doc, node, 0, yaxml_str::NULL, callback, (&raw mut countdown).cast(), errp)
            }
            19 => {
                let mut buffer = yaxml_buffer::default();
                let take = input.below(state.items.len() + 1).min(state.items.len());
                let start = state.items.len() - take;
                let mut items = state.items[start..].to_vec();
                if let Some(item) = items.first_mut() {
                    item.r#type = [0, 2, 18, 5][input.below(4)];
                }
                let nodes = if input.byte() % 2 == 0 { null() } else { items.as_ptr() };
                let prefixes = [input.string()];
                let status = yaxml_c14n(state.doc(input), i32::from(input.byte() % 4), i32::from(input.byte() % 2), nodes, items.len(), prefixes.as_ptr(), input.below(2), out!(buffer), errp);
                touch(yaxml_str { ptr: buffer.ptr, len: buffer.len });
                state.allocations.push(buffer.ptr.cast());
                status
            }
            20 => {
                let mut list = yaxml_diagnostics::default();
                let status = match input.byte() % 3 {
                    0 => yaxml_validate_dtd(state.doc(input), out!(list), errp),
                    1 => yaxml_doc_diagnostics(state.doc(input), out!(list), errp),
                    _ => {
                        let schema = State::pick(&state.schemas, input, null_mut());
                        yaxml_validate(state.doc(input), schema, out!(list), errp)
                    }
                };
                if list.len > 0 {
                    let records = std::slice::from_raw_parts(list.items, list.len);
                    records.iter().for_each(|d| { touch(d.message); });
                    assert!(list.errors <= list.len);
                }
                state.allocations.push(list.items.cast());
                status
            }
            21 => {
                let (kind, text) = SCHEMAS[input.below(SCHEMAS.len())];
                let (kind, text) = match input.byte() % 4 {
                    0 => (i32::from(input.byte() % 5), input.bytes()),
                    _ => (kind, text.as_bytes()),
                };
                let mut schema = null_mut();
                let status = yaxml_schema_new(kind, text.as_ptr(), text.len(), input.string(), out!(schema), errp);
                if status == OK {
                    state.schemas.push(schema);
                }
                status
            }
            22 => yaxml_schema_free(State::pick(&state.schemas, input, null_mut())),
            23 | 24 => {
                let mut node = yaxml_node::default();
                let spec = yaxml_name_spec { local: input.string(), uri: input.string(), prefix: input.string(), automatic: i32::from(input.byte() % 2), reserved: 0 };
                let (doc, parent) = (state.doc(input), state.node(input));
                let status = match input.byte() % 3 {
                    0 => yaxml_append_element(doc, parent, &spec, out!(node), errp),
                    1 => yaxml_append_leaf(doc, parent, [3, 4, 7, 8, 1][input.below(5)], input.string(), input.string(), out!(node), errp),
                    _ => {
                        let mut created = null_mut();
                        let status = yaxml_doc_create(&spec, &mut created, errp);
                        if status == OK {
                            state.docs.push(created);
                            yaxml_doc_element(created, &mut node, null_mut())
                        } else {
                            status
                        }
                    }
                };
                if status == OK {
                    state.nodes.push(node);
                }
                status
            }
            25 => yaxml_set_value(state.doc(input), state.node(input), input.string(), errp),
            26 => {
                let spec = yaxml_name_spec { local: input.string(), uri: input.string(), prefix: input.string(), automatic: i32::from(input.byte() % 2), reserved: 0 };
                let (doc, node) = (state.doc(input), state.node(input));
                match input.byte() % 5 {
                    0 => yaxml_set_attribute(doc, node, &spec, input.string(), errp),
                    1 => yaxml_remove_attribute(doc, node, input.string(), input.string(), errp),
                    2 => yaxml_declare_namespace(doc, node, input.string(), input.string(), errp),
                    3 => yaxml_set_name(doc, node, &spec, errp),
                    _ => yaxml_set_url(doc, input.string(), errp),
                }
            }
            27 | 28 => {
                let mut node = yaxml_node::default();
                let (doc, target, moved) = (state.doc(input), state.node(input), state.node(input));
                let status = yaxml_move(doc, i32::from(input.byte() % 6), target, moved, out!(node), errp);
                if status == OK && !null_out {
                    state.nodes.push(node);
                }
                status
            }
            29 => {
                let (doc, node) = (state.doc(input), state.node(input));
                match input.byte() % 3 {
                    0 => yaxml_unlink(doc, node, errp),
                    1 => yaxml_remove(doc, node, errp),
                    _ => yaxml_compact(doc, errp),
                }
            }
            30 => {
                let mut node = yaxml_node::default();
                let (doc, source, from) = (state.doc(input), state.doc(input), state.node(input));
                let deep = i32::from(input.byte() % 2);
                let status = match input.byte() % 2 {
                    0 => yaxml_copy(doc, from, deep, out!(node), errp),
                    _ => yaxml_import(doc, source, from, deep, out!(node), errp),
                };
                if status == OK {
                    state.nodes.push(node);
                }
                status
            }
            31 => {
                let mut copy = null_mut();
                let status = yaxml_doc_clone(state.doc(input), out!(copy), errp);
                if status == OK {
                    state.docs.push(copy);
                }
                status
            }
            32 => {
                let limits = yaxml_edit_limits {
                    struct_size: if input.byte() % 16 == 0 { 4 } else { size_of::<yaxml_edit_limits>() as u32 },
                    max_value_bytes: u64::from(input.byte() % 8),
                    max_depth: u64::from(input.byte() % 4),
                    max_created_nodes: u64::from(input.byte() % 16),
                    ..Default::default()
                };
                let limitsp = if input.byte() % 4 == 0 { null() } else { &raw const limits };
                yaxml_doc_set_edit_limits(state.doc(input), limitsp, errp)
            }
            33 => {
                let text = match input.byte() % 4 {
                    0 => input.bytes(),
                    _ => DOCUMENTS[input.below(DOCUMENTS.len())].as_bytes(),
                };
                let o = options(input);
                let mut reader = null_mut();
                let status = yaxml_reader_new(text.as_ptr(), text.len(), &o, out!(reader), errp);
                if status == OK {
                    state.readers.push(reader);
                }
                status
            }
            34 => {
                let reader = State::pick(&state.readers, input, null_mut());
                match input.below(3) {
                    0 => {
                        let mut event = yaxml_event::default();
                        let status = yaxml_reader_next(reader, out!(event), errp);
                        touch_event(&event);
                        status
                    }
                    _ => {
                        // Batches of 0 (an argument error) to 7 events.
                        let (mut events, mut count) = (null(), 0);
                        let max = input.below(8);
                        let status = yaxml_reader_next_batch(reader, max, out!(events), &raw mut count, errp);
                        if status == OK {
                            std::slice::from_raw_parts(events, count).iter().for_each(|e| {
                                touch_event(e);
                            });
                        }
                        status
                    }
                }
            }
            35 => yaxml_reader_free(State::pick(&state.readers, input, null_mut())),
            36 => {
                let o = options(input);
                let mut push = null_mut();
                let status = yaxml_push_new(&o, out!(push), errp);
                if status == OK {
                    state.pushers.push(push);
                }
                status
            }
            37 => {
                let push = State::pick(&state.pushers, input, null_mut());
                let mut countdown = input.byte();
                let callback = if input.byte() % 4 == 0 { None } else { Some(on_event as _) };
                let user = (&raw mut countdown).cast();
                match input.byte() % 3 {
                    0 => yaxml_push_finish(push, callback, user, errp),
                    _ => {
                        let text = match input.byte() % 2 {
                            0 => input.bytes(),
                            _ => {
                                let doc = DOCUMENTS[input.below(DOCUMENTS.len())].as_bytes();
                                &doc[..input.below(doc.len() + 1)]
                            }
                        };
                        yaxml_push_feed(push, text.as_ptr(), text.len(), callback, user, errp)
                    }
                }
            }
            38 => yaxml_push_free(State::pick(&state.pushers, input, null_mut())),
            _ => {
                // Free an allocation again, or an invented pointer.
                let ptr = State::pick(&state.allocations, input, null());
                yaxml_free(ptr);
                yaxml_free(std::ptr::without_provenance(0x1000 + usize::from(input.byte())));
                OK
            }
        }
    };
    // The free functions take no `err`.
    if errp.is_null() || matches!(op, 4 | 22 | 35 | 38 | 39) {
        assert!((-12..=1).contains(&status), "status {status}");
    } else {
        state.keep_error(&err, status);
    }
    true
}

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let mut input = Input(data);
    let mut state = State::default();
    let mut steps = 0;
    while steps < 256 && step(&mut state, &mut input) {
        steps += 1;
    }
    // SAFETY: handles are checked; frees of freed handles are errors.
    unsafe {
        state.docs.iter().for_each(|&d| { yaxml_doc_free(d); });
        state.schemas.iter().for_each(|&s| { yaxml_schema_free(s); });
        state.readers.iter().for_each(|&r| { yaxml_reader_free(r); });
        state.pushers.iter().for_each(|&p| { yaxml_push_free(p); });
    }
    state.allocations.iter().for_each(|&a| yaxml_free(a));
    // Double frees after everything is gone stay harmless.
    state.allocations.iter().for_each(|&a| yaxml_free(a));
    assert_eq!(yaxml_capi::live(), (0, 0), "a handle or allocation leaked");
});
