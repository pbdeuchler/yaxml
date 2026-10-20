//! Every exported function, happy and sad paths, called as C would.
// Every call here is an FFI call with valid pointers or deliberately invalid
// handles (which the ABI checks); the per-block lints guard the library.
#![allow(
    clippy::undocumented_unsafe_blocks,
    clippy::multiple_unsafe_ops_per_block
)]

/// (domain, code, level, line, message) of a diagnostic.
type Diag = (i32, i32, i32, u64, String);
/// (kind, name or target, value, attribute count) of an event.
type Ev = (i32, Option<String>, Option<String>, usize);

use std::ffi::c_void;
use std::ptr::{null, null_mut};

use yaxml_capi::types::*;
use yaxml_capi::*;

fn s(text: &str) -> yaxml_str {
    yaxml_str::of(text)
}

fn spec(local: &str, uri: Option<&str>, prefix: Option<&str>, automatic: bool) -> yaxml_name_spec {
    yaxml_name_spec {
        local: s(local),
        uri: yaxml_str::opt(uri),
        prefix: yaxml_str::opt(prefix),
        automatic: automatic.into(),
        reserved: 0,
    }
}

/// A borrowed result as a String; NULL is `None`.
fn read(v: yaxml_str) -> Option<String> {
    // SAFETY: the library returned `v` for a live document.
    (!v.ptr.is_null()).then(|| {
        String::from_utf8(unsafe { std::slice::from_raw_parts(v.ptr, v.len) }.to_vec()).unwrap()
    })
}

/// An owned result as a String, freed.
fn take(b: yaxml_buffer) -> String {
    let text = read(yaxml_str {
        ptr: b.ptr,
        len: b.len,
    })
    .unwrap_or_default();
    yaxml_free(b.ptr.cast());
    text
}

/// The error's message, freed (a second free is harmless).
fn message(e: &yaxml_error) -> String {
    let text = read(e.diagnostic.message).unwrap_or_default();
    yaxml_free(e.diagnostic.message.ptr.cast());
    yaxml_free(e.diagnostic.message.ptr.cast());
    text
}

fn parse_with(
    xml: &str,
    options: Option<&yaxml_parse_options>,
) -> Result<*mut yaxml_doc, (i32, yaxml_error)> {
    let (mut doc, mut err) = (null_mut(), yaxml_error::default());
    let options = options.map_or(null(), |o| o as *const _);
    // SAFETY: valid pointers.
    match unsafe { yaxml_parse(xml.as_ptr(), xml.len(), options, &mut doc, &mut err) } {
        OK => Ok(doc),
        status => Err((status, err)),
    }
}

fn parse(xml: &str) -> *mut yaxml_doc {
    parse_with(xml, None).unwrap()
}

fn options(flags: u32) -> yaxml_parse_options {
    yaxml_parse_options {
        struct_size: size_of::<yaxml_parse_options>() as u32,
        flags,
        ..Default::default()
    }
}

fn root(doc: *mut yaxml_doc) -> yaxml_node {
    let mut node = yaxml_node::default();
    // SAFETY: valid pointers.
    assert_eq!(unsafe { yaxml_doc_element(doc, &mut node, null_mut()) }, OK);
    node
}

fn related(doc: *mut yaxml_doc, node: yaxml_node, relation: i32) -> (i32, yaxml_node) {
    let mut out = yaxml_node::default();
    // SAFETY: valid pointers.
    (
        unsafe { yaxml_node_relative(doc, node, relation, &mut out, null_mut()) },
        out,
    )
}

fn child(doc: *mut yaxml_doc, node: yaxml_node) -> yaxml_node {
    related(doc, node, 2).1
}

fn save(doc: *mut yaxml_doc, node: yaxml_node, flags: u32) -> String {
    let mut out = yaxml_buffer::default();
    // SAFETY: valid pointers.
    assert_eq!(
        unsafe { yaxml_save(doc, node, flags, yaxml_str::NULL, &mut out, null_mut()) },
        OK
    );
    take(out)
}

fn document(doc: *mut yaxml_doc) -> String {
    let mut node = yaxml_node::default();
    // SAFETY: valid pointers.
    assert_eq!(unsafe { yaxml_doc_root(doc, &mut node, null_mut()) }, OK);
    save(doc, node, yaxml_capi_flags::NO_DECL)
}

mod yaxml_capi_flags {
    pub const NO_DECL: u32 = 2;
}

fn free(doc: *mut yaxml_doc) {
    // SAFETY: handle tokens are checked.
    assert_eq!(unsafe { yaxml_doc_free(doc) }, OK);
}

fn xpath(
    doc: *mut yaxml_doc,
    context: yaxml_node,
    expression: &str,
    ns: &[yaxml_ns],
) -> Result<yaxml_xpath_value, (i32, yaxml_error)> {
    let (mut out, mut err) = (yaxml_xpath_value::default(), yaxml_error::default());
    // SAFETY: valid pointers.
    match unsafe {
        yaxml_xpath_eval(
            doc,
            context,
            s(expression),
            ns.as_ptr(),
            ns.len(),
            &mut out,
            &mut err,
        )
    } {
        OK => Ok(out),
        status => Err((status, err)),
    }
}

fn items(value: &yaxml_xpath_value) -> Vec<yaxml_xpath_item> {
    // SAFETY: the library returned `len` items.
    let items = unsafe { yaxml_capi_slice(value.items, value.len) };
    items.to_vec()
}

/// # Safety
/// `ptr` holds `len` values or is NULL.
unsafe fn yaxml_capi_slice<'a, T>(ptr: *const T, len: usize) -> &'a [T] {
    if ptr.is_null() {
        return &[];
    }
    // SAFETY: forwarded.
    unsafe { std::slice::from_raw_parts(ptr, len) }
}

fn diagnostics(d: yaxml_diagnostics) -> Vec<Diag> {
    // SAFETY: the library returned `len` items.
    let list = unsafe { yaxml_capi_slice(d.items, d.len) }
        .iter()
        .map(|d| (d.domain, d.code, d.level, d.line, read(d.message).unwrap()))
        .collect();
    yaxml_free(d.items.cast());
    list
}

#[test]
fn library_queries() {
    // SAFETY: a static NUL-terminated string.
    let version = unsafe { std::ffi::CStr::from_ptr(yaxml_version()) };
    assert_eq!(version.to_str().unwrap(), yaxml::VERSION);
    for (feature, expected) in [(4, 1), (999, 0), (-1, 0)] {
        assert_eq!(yaxml_has_feature(feature), expected, "{feature}");
    }
    yaxml_free(null());
    yaxml_free(0x1234 as *const c_void);
    let (mut handles, mut allocations) = (0, 0);
    // SAFETY: valid pointers (the counts race with other tests, so only the status is checked).
    assert_eq!(
        unsafe { yaxml_live_objects(&mut handles, &mut allocations) },
        OK
    );
    // SAFETY: NULL is checked.
    assert_eq!(
        unsafe { yaxml_live_objects(null_mut(), &mut allocations) },
        ERR_NULL
    );
}

#[test]
fn parse_reports_libxml2_diagnostics_and_option_errors() {
    let doc = parse("<r/>");
    free(doc);

    let (status, err) = parse_with("<a>\n  <b></c>\n</a>", None).unwrap_err();
    assert_eq!((status, err.status), (ERR_XML, ERR_XML));
    let d = err.diagnostic;
    assert_eq!(
        (d.domain, d.code, d.level, d.line, d.column),
        (1, 76, 3, 2, 10)
    );
    assert_eq!(
        message(&err),
        "Opening and ending tag mismatch: b line 2 and c"
    );

    let bad_size = yaxml_parse_options {
        struct_size: 8,
        ..options(0)
    };
    let deep = yaxml_parse_options {
        max_depth: 1,
        ..options(0)
    };
    let cases: [(&str, yaxml_parse_options, i32, i32); 4] = [
        ("unsupported flag", options(1 << 17), ERR_XML, 115),
        ("struct_size too small", bad_size, ERR_ARGUMENT, 0),
        ("limit", deep, ERR_XML, 114),
        (
            "base not UTF-8",
            yaxml_parse_options {
                base: yaxml_str {
                    ptr: b"\xff".as_ptr(),
                    len: 1,
                },
                ..options(0)
            },
            ERR_ARGUMENT,
            0,
        ),
    ];
    for (what, o, status, code) in cases {
        let (actual, err) = parse_with("<r><a/></r>", Some(&o)).unwrap_err();
        assert_eq!((actual, err.diagnostic.code), (status, code), "{what}");
        message(&err);
    }

    // A newer header's struct: unknown trailing bytes must be zero.
    #[repr(C)]
    struct Grown {
        options: yaxml_parse_options,
        extra: u64,
    }
    let mut grown = Grown {
        options: options(0),
        extra: 0,
    };
    grown.options.struct_size = size_of::<Grown>() as u32;
    let (mut doc, mut err) = (null_mut(), yaxml_error::default());
    let ptr = (&raw const grown).cast::<yaxml_parse_options>();
    // SAFETY: `grown` holds struct_size bytes.
    assert_eq!(
        unsafe { yaxml_parse(b"<r/>".as_ptr(), 4, ptr, &mut doc, &mut err) },
        OK
    );
    free(doc);
    let grown = Grown { extra: 1, ..grown };
    let ptr = (&raw const grown).cast::<yaxml_parse_options>();
    // SAFETY: as above.
    assert_eq!(
        unsafe { yaxml_parse(b"<r/>".as_ptr(), 4, ptr, &mut doc, &mut err) },
        ERR_ARGUMENT
    );
    message(&err);

    // NULL arguments.
    // SAFETY: NULL is checked.
    assert_eq!(
        unsafe { yaxml_parse(null(), 3, null(), &mut doc, null_mut()) },
        ERR_NULL
    );
    // SAFETY: NULL is checked.
    assert_eq!(
        unsafe { yaxml_parse(b"<r/>".as_ptr(), 4, null(), null_mut(), null_mut()) },
        ERR_NULL
    );
    // Empty input is a parse error, not a pointer error.
    // SAFETY: NULL with length 0 is empty.
    assert_eq!(
        unsafe { yaxml_parse(null(), 0, null(), &mut doc, &mut err) },
        ERR_XML
    );
    assert_eq!(err.diagnostic.code, 4);
    message(&err);
}

#[test]
fn handles_are_checked_and_frees_are_idempotent() {
    let doc = parse("<r><a/></r>");
    let other = parse("<r><a/></r>");
    let a = child(doc, root(doc));
    let foreign = child(other, root(other));
    let invented = 0x7777_0000 as *mut yaxml_doc;
    let mut kind = 0;
    for (what, d, node, status) in [
        ("live", doc, a, OK),
        ("foreign node", doc, foreign, ERR_HANDLE),
        ("null node", doc, yaxml_node::default(), ERR_HANDLE),
        (
            "forged slot",
            doc,
            yaxml_node { slot: 1 << 40, ..a },
            ERR_HANDLE,
        ),
        ("invented doc", invented, a, ERR_HANDLE),
        ("NULL doc", null_mut(), a, ERR_NULL),
    ] {
        // SAFETY: valid out-pointer; handles are checked.
        assert_eq!(
            unsafe { yaxml_node_kind(d, node, &mut kind, null_mut()) },
            status,
            "{what}"
        );
    }
    // SAFETY: handles are checked.
    assert_eq!(unsafe { yaxml_remove(doc, a, null_mut()) }, OK);
    // SAFETY: as above.
    assert_eq!(
        unsafe { yaxml_node_kind(doc, a, &mut kind, null_mut()) },
        ERR_HANDLE
    );
    free(doc);
    // SAFETY: as above.
    assert_eq!(unsafe { yaxml_doc_free(doc) }, ERR_HANDLE);
    // SAFETY: as above.
    assert_eq!(
        unsafe { yaxml_node_kind(doc, a, &mut kind, null_mut()) },
        ERR_HANDLE
    );
    // SAFETY: as above.
    assert_eq!(unsafe { yaxml_doc_free(null_mut()) }, ERR_NULL);
    // A document handle is not a reader, push parser or schema handle.
    // SAFETY: as above.
    assert_eq!(unsafe { yaxml_reader_free(other.cast()) }, ERR_HANDLE);
    // SAFETY: as above.
    assert_eq!(unsafe { yaxml_push_free(other.cast()) }, ERR_HANDLE);
    // SAFETY: as above.
    assert_eq!(unsafe { yaxml_schema_free(other.cast()) }, ERR_HANDLE);
    free(other);
}

#[test]
fn navigation_names_values_and_attributes() {
    let xml = "<?xml version='1.0'?>\n<!DOCTYPE r [<!ENTITY e 'ent'>]>\n<p:r xmlns:p='urn:p' xmlns='urn:d' a='1' p:b='2' xml:id='top'>text<?pi data?><!--c-->&e;<k/></p:r>";
    let doc = parse(xml);
    let r = root(doc);
    let mut name = yaxml_name::default();
    // SAFETY: valid pointers.
    assert_eq!(
        unsafe { yaxml_node_name(doc, r, &mut name, null_mut()) },
        OK
    );
    assert_eq!(
        (
            read(name.qname),
            read(name.prefix),
            read(name.local),
            read(name.uri)
        ),
        (
            Some("p:r".into()),
            Some("p".into()),
            Some("r".into()),
            Some("urn:p".into())
        )
    );

    // Children: text, PI, comment, entity reference, element.
    let mut kinds = Vec::new();
    let mut node = child(doc, r);
    loop {
        let mut kind = 0;
        // SAFETY: valid pointers.
        assert_eq!(
            unsafe { yaxml_node_kind(doc, node, &mut kind, null_mut()) },
            OK
        );
        kinds.push(kind);
        let (status, next) = related(doc, node, 4);
        if status == NONE {
            assert_eq!(next, yaxml_node::default());
            break;
        }
        node = next;
    }
    assert_eq!(kinds, [3, 7, 8, 5, 1]);
    let text = child(doc, r);
    let pi = related(doc, text, 4).1;
    let entity = related(doc, related(doc, pi, 4).1, 4).1;
    let k = related(doc, r, 3).1;
    for (what, relation, from, expected) in [
        ("parent", 1, text, (OK, r)),
        ("last child", 3, r, (OK, k)),
        ("previous", 5, pi, (OK, text)),
        ("no previous", 5, text, (NONE, yaxml_node::default())),
        ("leaf child", 2, text, (NONE, yaxml_node::default())),
    ] {
        assert_eq!(related(doc, from, relation), expected, "{what}");
    }
    assert_eq!(related(doc, r, 9).0, ERR_ARGUMENT);
    let mut document_node = yaxml_node::default();
    // SAFETY: valid pointers.
    unsafe { yaxml_doc_root(doc, &mut document_node, null_mut()) };
    assert_eq!(related(doc, document_node, 1).0, NONE);
    let mut kind = 0;
    // SAFETY: valid pointers.
    unsafe { yaxml_node_kind(doc, document_node, &mut kind, null_mut()) };
    assert_eq!(kind, 9);

    for (what, node, expected) in [
        ("PI", pi, (OK, Some("pi"))),
        ("entity", entity, (OK, Some("e"))),
        ("text", text, (NONE, None)),
    ] {
        let mut name = yaxml_name::default();
        // SAFETY: valid pointers.
        let status = unsafe { yaxml_node_name(doc, node, &mut name, null_mut()) };
        assert_eq!((status, read(name.local).as_deref()), expected, "{what}");
    }

    let mut value = yaxml_str::default();
    // SAFETY: valid pointers.
    assert_eq!(
        unsafe { yaxml_node_value(doc, pi, &mut value, null_mut()) },
        OK
    );
    assert_eq!(read(value).as_deref(), Some("data"));
    // SAFETY: valid pointers.
    assert_eq!(
        unsafe { yaxml_node_value(doc, r, &mut value, null_mut()) },
        NONE
    );
    assert!(value.ptr.is_null());

    let mut buffer = yaxml_buffer::default();
    // SAFETY: valid pointers.
    assert_eq!(
        unsafe { yaxml_node_content(doc, r, &mut buffer, null_mut()) },
        OK
    );
    assert_eq!(take(buffer), "textent");
    // SAFETY: valid pointers.
    assert_eq!(
        unsafe { yaxml_node_path(doc, k, &mut buffer, null_mut()) },
        OK
    );
    assert_eq!(take(buffer), "/p:r/*");
    let mut line = 0;
    // SAFETY: valid pointers.
    assert_eq!(
        unsafe { yaxml_node_line(doc, r, &mut line, null_mut()) },
        OK
    );
    assert_eq!(line, 3);

    let mut count = 0;
    // SAFETY: valid pointers.
    assert_eq!(
        unsafe { yaxml_node_attribute_count(doc, r, &mut count, null_mut()) },
        OK
    );
    assert_eq!(count, 5);
    let mut attribute = yaxml_attribute::default();
    // SAFETY: valid pointers.
    assert_eq!(
        unsafe { yaxml_node_attribute_at(doc, r, 3, &mut attribute, null_mut()) },
        OK
    );
    assert_eq!(
        (
            read(attribute.name.qname).as_deref(),
            read(attribute.value).as_deref()
        ),
        (Some("p:b"), Some("2"))
    );
    // SAFETY: valid pointers.
    assert_eq!(
        unsafe { yaxml_node_attribute_at(doc, r, 5, &mut attribute, null_mut()) },
        NONE
    );

    for (what, name, expected) in [
        ("a", "a", (OK, Some("1"))),
        ("b by local", "b", (OK, Some("2"))),
        ("missing", "z", (NONE, None)),
    ] {
        // SAFETY: valid pointers.
        let status = unsafe { yaxml_node_attribute(doc, r, s(name), &mut value, null_mut()) };
        assert_eq!((status, read(value).as_deref()), expected, "{what}");
    }
    let bad = yaxml_str {
        ptr: b"\xc3".as_ptr(),
        len: 1,
    };
    // SAFETY: valid pointers.
    assert_eq!(
        unsafe { yaxml_node_attribute(doc, r, bad, &mut value, null_mut()) },
        ERR_ARGUMENT
    );
    for (what, uri, expected) in [
        ("namespaced", Some("urn:p"), (OK, Some("2"))),
        ("no namespace", None, (NONE, None)),
    ] {
        // SAFETY: valid pointers.
        let status = unsafe {
            yaxml_node_attribute_ns(doc, r, s("b"), yaxml_str::opt(uri), &mut value, null_mut())
        };
        assert_eq!((status, read(value).as_deref()), expected, "{what}");
    }
    for (what, prefix, expected) in [
        ("p", Some("p"), (OK, Some("urn:p"))),
        ("default", None, (OK, Some("urn:d"))),
        ("unbound", Some("q"), (NONE, None)),
    ] {
        // SAFETY: valid pointers.
        let status = unsafe {
            yaxml_node_namespace_uri(doc, k, yaxml_str::opt(prefix), &mut value, null_mut())
        };
        assert_eq!((status, read(value).as_deref()), expected, "{what}");
    }
    for (what, uri, expected) in [
        ("p", "urn:p", (OK, Some("p"))),
        ("default", "urn:d", (OK, None)),
        ("unbound", "urn:x", (NONE, None)),
    ] {
        // SAFETY: valid pointers.
        let status = unsafe { yaxml_node_namespace_prefix(doc, k, s(uri), &mut value, null_mut()) };
        assert_eq!((status, read(value).as_deref()), expected, "{what}");
    }
    let mut found = yaxml_node::default();
    // SAFETY: valid pointers.
    assert_eq!(
        unsafe { yaxml_doc_element_by_id(doc, s("top"), &mut found, null_mut()) },
        OK
    );
    assert_eq!(found, r);
    // SAFETY: valid pointers.
    assert_eq!(
        unsafe { yaxml_doc_element_by_id(doc, s("none"), &mut found, null_mut()) },
        NONE
    );
    // SAFETY: NULL out is checked.
    assert_eq!(
        unsafe { yaxml_node_value(doc, pi, null_mut(), null_mut()) },
        ERR_NULL
    );
    free(doc);
}

#[test]
fn recovered_documents_have_diagnostics_and_may_lack_an_element() {
    let doc = parse_with("<!--only-->", Some(&options(1))).unwrap();
    let mut node = yaxml_node::default();
    // SAFETY: valid pointers.
    assert_eq!(
        unsafe { yaxml_doc_element(doc, &mut node, null_mut()) },
        NONE
    );
    let mut out = yaxml_diagnostics::default();
    // SAFETY: valid pointers.
    assert_eq!(
        unsafe { yaxml_doc_diagnostics(doc, &mut out, null_mut()) },
        OK
    );
    let list = diagnostics(out);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].1, 4);
    free(doc);

    let doc = parse("<r/>");
    // SAFETY: valid pointers.
    assert_eq!(
        unsafe { yaxml_doc_diagnostics(doc, &mut out, null_mut()) },
        OK
    );
    assert!(out.items.is_null() && out.len == 0);
    free(doc);

    // A namespace error does not stop a parse, with or without RECOVER.
    for flags in [0, 1] {
        let doc = parse_with("<r><p:c/></r>", Some(&options(flags))).unwrap();
        // SAFETY: valid pointers.
        assert_eq!(
            unsafe { yaxml_doc_diagnostics(doc, &mut out, null_mut()) },
            OK
        );
        let message = "Namespace prefix p on c is not defined".to_owned();
        assert_eq!(diagnostics(out), [(3, 201, 2, 1, message)]);
        free(doc);
    }
}

#[test]
fn xpath_values_and_errors() {
    let doc = parse("<r xmlns:p='urn:p'><p:i k='1'/><p:i k='2'/></r>");
    let r = root(doc);
    let ns = [yaxml_ns {
        prefix: s("q"),
        uri: s("urn:p"),
    }];
    let nodes = xpath(doc, r, "//q:i/@k", &ns).unwrap();
    assert_eq!(nodes.r#type, 1);
    let list = items(&nodes);
    assert_eq!(
        list.iter().map(|i| (i.r#type, i.index)).collect::<Vec<_>>(),
        [(2, 0), (2, 0)]
    );
    yaxml_free(nodes.items.cast());
    let tree = xpath(doc, r, "q:i", &ns).unwrap();
    assert_eq!(
        items(&tree)[0],
        yaxml_xpath_item {
            node: child(doc, r),
            r#type: 0,
            index: 0
        }
    );
    yaxml_free(tree.items.cast());
    let empty = xpath(doc, r, "nothing", &[]).unwrap();
    assert!((empty.r#type, empty.items.is_null()) == (1, true));

    let boolean = xpath(doc, r, "count(*) = 2", &[]).unwrap();
    assert_eq!((boolean.r#type, boolean.boolean), (2, 1));
    let number = xpath(doc, r, "sum(//@k)", &[]).unwrap();
    assert_eq!((number.r#type, number.number), (3, 3.0));
    let string = xpath(doc, r, "concat('a', 'b')", &[]).unwrap();
    assert_eq!((string.r#type, take(string.string)), (4, "ab".into()));

    for (what, expression, status, code) in [
        ("syntax", "'open", ERR_XPATH, 1202),
        ("undefined prefix", "//z:i", ERR_XPATH, 1219),
        ("unknown function", "nope()", ERR_XPATH, 1209),
    ] {
        let (actual, err) = xpath(doc, r, expression, &[]).unwrap_err();
        assert_eq!(
            (actual, err.diagnostic.domain, err.diagnostic.code),
            (status, 12, code),
            "{what}"
        );
        message(&err);
    }
    let (status, _) = xpath(doc, yaxml_node::default(), ".", &[]).unwrap_err();
    assert_eq!(status, ERR_HANDLE);
    free(doc);
}

extern "C" fn collect(user: *mut c_void, bytes: *const u8, len: usize) -> i32 {
    // SAFETY: the test passes a Vec<u8>; the library passes `len` bytes.
    let (sink, bytes) = unsafe {
        (
            &mut *user.cast::<Vec<u8>>(),
            std::slice::from_raw_parts(bytes, len),
        )
    };
    sink.extend_from_slice(bytes);
    0
}

extern "C" fn refuse(_: *mut c_void, _: *const u8, _: usize) -> i32 {
    1
}

#[test]
fn save_to_buffers_and_callbacks() {
    let doc = parse("<r><a>caf\u{e9}</a></r>");
    let r = root(doc);
    let mut document_node = yaxml_node::default();
    // SAFETY: valid pointers.
    unsafe { yaxml_doc_root(doc, &mut document_node, null_mut()) };
    assert_eq!(
        save(doc, document_node, 0),
        "<?xml version=\"1.0\"?>\n<r><a>caf&#xE9;</a></r>\n"
    );
    assert_eq!(save(doc, child(doc, r), 0), "<a>caf&#xE9;</a>");
    assert_eq!(save(doc, r, 1), "<r>\n  <a>caf&#xE9;</a>\n</r>");
    let mut out = yaxml_buffer::default();
    // SAFETY: valid pointers.
    assert_eq!(
        unsafe { yaxml_save(doc, child(doc, r), 0, s("ISO-8859-1"), &mut out, null_mut()) },
        OK
    );
    let latin1 = out;
    // SAFETY: the library returned `len` bytes.
    assert_eq!(
        unsafe { std::slice::from_raw_parts(latin1.ptr, latin1.len) },
        b"<a>caf\xe9</a>"
    );
    yaxml_free(latin1.ptr.cast());
    let mut err = yaxml_error::default();
    for (what, flags, encoding, code) in [
        ("encoding", 0, "nope", 32),
        ("flags", 1 << 20, "UTF-8", 115),
    ] {
        // SAFETY: valid pointers.
        assert_eq!(
            unsafe { yaxml_save(doc, r, flags, s(encoding), &mut out, &mut err) },
            ERR_XML,
            "{what}"
        );
        assert_eq!(err.diagnostic.code, code, "{what}");
        message(&err);
    }

    let mut sink = Vec::<u8>::new();
    let user = (&raw mut sink).cast::<c_void>();
    // SAFETY: `collect` accepts a Vec<u8>.
    assert_eq!(
        unsafe { yaxml_save_to(doc, r, 0, yaxml_str::NULL, Some(collect), user, null_mut()) },
        OK
    );
    assert_eq!(sink, b"<r><a>caf&#xE9;</a></r>");
    // SAFETY: `refuse` ignores its arguments.
    assert_eq!(
        unsafe {
            yaxml_save_to(
                doc,
                r,
                0,
                yaxml_str::NULL,
                Some(refuse),
                null_mut(),
                &mut err,
            )
        },
        ERR_CALLBACK
    );
    message(&err);
    // SAFETY: NULL callbacks are checked.
    assert_eq!(
        unsafe { yaxml_save_to(doc, r, 0, yaxml_str::NULL, None, null_mut(), null_mut()) },
        ERR_NULL
    );
    free(doc);
}

fn c14n(
    doc: *mut yaxml_doc,
    mode: i32,
    comments: bool,
    nodes: Option<&[yaxml_xpath_item]>,
    prefixes: &[yaxml_str],
) -> Result<String, (i32, i32)> {
    let (mut out, mut err) = (yaxml_buffer::default(), yaxml_error::default());
    let (ptr, len) = nodes.map_or((null(), 0), |n| (n.as_ptr(), n.len()));
    // SAFETY: valid pointers.
    match unsafe {
        yaxml_c14n(
            doc,
            mode,
            comments.into(),
            ptr,
            len,
            prefixes.as_ptr(),
            prefixes.len(),
            &mut out,
            &mut err,
        )
    } {
        OK => Ok(take(out)),
        status => Err((status, err.diagnostic.code)).inspect(|_| {
            message(&err);
        }),
    }
}

#[test]
fn canonicalization() {
    let doc = parse("<r xmlns:p='urn:p' b='2' a='1'><!--c--><p:x/></r>");
    assert_eq!(
        c14n(doc, 0, false, None, &[]).unwrap(),
        "<r xmlns:p=\"urn:p\" a=\"1\" b=\"2\"><p:x></p:x></r>"
    );
    assert_eq!(
        c14n(doc, 2, true, None, &[]).unwrap(),
        "<r xmlns:p=\"urn:p\" a=\"1\" b=\"2\"><!--c--><p:x></p:x></r>"
    );
    let set = xpath(
        doc,
        root(doc),
        "//p:x | //p:x/namespace::*",
        &[yaxml_ns {
            prefix: s("p"),
            uri: s("urn:p"),
        }],
    )
    .unwrap();
    let list = items(&set);
    yaxml_free(set.items.cast());
    assert_eq!(
        c14n(doc, 1, false, Some(&list), &[s("p")]).unwrap(),
        "<p:x xmlns:p=\"urn:p\"></p:x>"
    );
    assert_eq!(c14n(doc, 7, false, None, &[]), Err((ERR_ARGUMENT, 0)));
    let bad = [yaxml_xpath_item {
        r#type: 99,
        ..list[0]
    }];
    assert_eq!(c14n(doc, 0, false, Some(&bad), &[]), Err((ERR_ARGUMENT, 0)));
    free(doc);
    let doc = parse("<!DOCTYPE r [<!ENTITY e 'x'>]><r>&e;</r>");
    assert_eq!(c14n(doc, 0, false, None, &[]), Err((ERR_C14N, 1953)));
    free(doc);
}

fn schema(kind: i32, text: &str) -> Result<*mut yaxml_schema, (i32, i32, i32)> {
    let (mut out, mut err) = (null_mut(), yaxml_error::default());
    // SAFETY: valid pointers.
    match unsafe {
        yaxml_schema_new(
            kind,
            text.as_ptr(),
            text.len(),
            s("schema.xml"),
            &mut out,
            &mut err,
        )
    } {
        OK => Ok(out),
        status => Err((status, err.diagnostic.domain, err.diagnostic.code)).inspect(|_| {
            message(&err);
        }),
    }
}

fn validate(doc: *mut yaxml_doc, schema: *mut yaxml_schema) -> (i32, Vec<Diag>, usize) {
    let mut out = yaxml_diagnostics::default();
    // SAFETY: valid pointers.
    let status = unsafe { yaxml_validate(doc, schema, &mut out, null_mut()) };
    let errors = out.errors;
    (status, diagnostics(out), errors)
}

#[test]
fn validation_against_dtds_and_schemas() {
    let dtd = "<!DOCTYPE r [<!ELEMENT r (a)><!ELEMENT a EMPTY>]>\n<r><b/></r>";
    let doc = parse(dtd);
    let mut out = yaxml_diagnostics::default();
    // SAFETY: valid pointers.
    assert_eq!(unsafe { yaxml_validate_dtd(doc, &mut out, null_mut()) }, OK);
    let errors = out.errors;
    let list = diagnostics(out);
    assert!(
        errors >= 1 && list.iter().all(|d| d.0 == 23 && d.3 == 2),
        "{list:?}"
    );
    free(doc);
    let doc = parse_with(dtd, Some(&options(1 << 4))).unwrap();
    // SAFETY: valid pointers.
    assert_eq!(
        unsafe { yaxml_doc_diagnostics(doc, &mut out, null_mut()) },
        OK
    );
    assert!(!diagnostics(out).is_empty());
    free(doc);

    let xsd = "<xs:schema xmlns:xs='http://www.w3.org/2001/XMLSchema'><xs:element name='r' type='xs:int'/></xs:schema>";
    let rng = "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><text/></element>";
    let sch = "<schema xmlns='http://purl.oclc.org/dsdl/schematron'><pattern id='p'><rule context='i'><assert test='@ok'>needs ok</assert></rule></pattern></schema>";
    for (what, kind, text, good, bad, domain) in [
        ("xsd", 1, xsd, "<r>5</r>", "<r>x</r>", 17),
        ("relaxng", 2, rng, "<r>t</r>", "<r><x/></r>", 19),
        (
            "schematron",
            3,
            sch,
            "<r><i ok='1'/></r>",
            "<r><i/></r>",
            28,
        ),
    ] {
        let schema = schema(kind, text).unwrap();
        let doc = parse(good);
        assert_eq!(validate(doc, schema), (OK, vec![], 0), "{what}");
        free(doc);
        let doc = parse(bad);
        let (status, list, errors) = validate(doc, schema);
        assert!(!list.is_empty(), "{what}");
        assert_eq!(
            (status, list[0].0, errors > 0),
            (OK, domain, true),
            "{what}: {list:?}"
        );
        free(doc);
        // SAFETY: handles are checked.
        assert_eq!(unsafe { yaxml_schema_free(schema) }, OK);
        // SAFETY: as above.
        assert_eq!(unsafe { yaxml_schema_free(schema) }, ERR_HANDLE);
        let doc = parse(good);
        assert_eq!(validate(doc, schema).0, ERR_HANDLE);
        free(doc);
    }
    for (what, kind, text, expected) in [
        ("xsd", 1, "<x/>", (ERR_SCHEMA, 16)),
        ("relaxng", 2, "<x/>", (ERR_SCHEMA, 18)),
        ("schematron", 3, "<x/>", (ERR_SCHEMA, 16)),
        ("kind", 9, xsd, (ERR_ARGUMENT, 0)),
    ] {
        let (status, domain, _) = schema(kind, text).unwrap_err();
        assert_eq!((status, domain), expected, "{what}");
    }
    // A Schematron schema without rules is xmlSchematronValidateDoc's -1.
    let empty = schema(3, "<schema xmlns='http://purl.oclc.org/dsdl/schematron'><pattern><rule context='i'/></pattern></schema>").unwrap();
    let doc = parse("<r/>");
    let (status, list, errors) = validate(doc, empty);
    assert_eq!(
        (status, list.len(), list[0].0, list[0].1, errors),
        (OK, 1, 17, 1, 1)
    );
    free(doc);
    // SAFETY: handles are checked.
    assert_eq!(unsafe { yaxml_schema_free(empty) }, OK);
}

fn check(status: i32, err: &yaxml_error, expected: (i32, i32)) {
    // On success `err` is untouched.
    let code = if status < 0 { err.diagnostic.code } else { 0 };
    assert_eq!((status, code), expected);
    if status < 0 {
        message(err);
    }
}

#[test]
fn editing_operations() {
    let doc = parse("<r><a/>x</r>");
    let r = root(doc);
    let a = child(doc, r);
    let (mut node, mut err) = (yaxml_node::default(), yaxml_error::default());
    // SAFETY (whole test): valid pointers; handles are checked.
    unsafe {
        let item = spec("item", Some("urn:i"), Some("i"), false);
        check(
            yaxml_append_element(doc, r, &item, &mut node, &mut err),
            &err,
            (OK, 0),
        );
        let item_node = node;
        let auto = spec("auto", Some("urn:i"), None, true);
        check(
            yaxml_append_element(doc, item_node, &auto, &mut node, &mut err),
            &err,
            (OK, 0),
        );
        check(
            yaxml_append_element(doc, r, null(), &mut node, &mut err),
            &err,
            (ERR_NULL, 0),
        );
        let bad = spec("1bad", None, None, false);
        check(
            yaxml_append_element(doc, r, &bad, &mut node, &mut err),
            &err,
            (ERR_EDIT, 8),
        );
        for (kind, target, value, expected) in [
            (3, "", "t", (OK, 0)),
            (4, "", "c", (OK, 0)),
            (8, "", "m", (OK, 0)),
            (7, "pi", "d", (OK, 0)),
            (8, "", "a--b", (ERR_EDIT, 7)),
            (6, "", "", (ERR_ARGUMENT, 0)),
        ] {
            check(
                yaxml_append_leaf(doc, a, kind, s(target), s(value), &mut node, &mut err),
                &err,
                expected,
            );
        }
        let text = node;
        check(
            yaxml_append_leaf(doc, text, 3, yaxml_str::NULL, s("x"), &mut node, &mut err),
            &err,
            (ERR_EDIT, 2),
        );
        let comment = related(doc, a, 3).1;
        let comment = related(doc, comment, 5).1;
        check(
            yaxml_set_value(doc, comment, s("new"), &mut err),
            &err,
            (OK, 0),
        );
        check(
            yaxml_set_value(doc, comment, s("x--"), &mut err),
            &err,
            (ERR_EDIT, 7),
        );
        check(
            yaxml_set_value(doc, a, s("x"), &mut err),
            &err,
            (ERR_EDIT, 2),
        );
        let k = spec("k", None, None, false);
        check(
            yaxml_set_attribute(doc, a, &k, s("v"), &mut err),
            &err,
            (OK, 0),
        );
        check(
            yaxml_remove_attribute(doc, a, yaxml_str::NULL, s("k"), &mut err),
            &err,
            (OK, 0),
        );
        check(
            yaxml_remove_attribute(doc, a, yaxml_str::NULL, s("k"), &mut err),
            &err,
            (NONE, 0),
        );
        check(
            yaxml_declare_namespace(doc, a, s("n"), s("urn:n"), &mut err),
            &err,
            (OK, 0),
        );
        check(
            yaxml_declare_namespace(doc, a, s("n"), s("urn:m"), &mut err),
            &err,
            (ERR_EDIT, 10),
        );
        let renamed = spec("b", None, None, false);
        check(yaxml_set_name(doc, a, &renamed, &mut err), &err, (OK, 0));
        check(
            yaxml_set_name(doc, text, &renamed, &mut err),
            &err,
            (ERR_EDIT, 2),
        );
        check(
            yaxml_set_url(doc, s("file:///x.xml"), &mut err),
            &err,
            (OK, 0),
        );
        check(yaxml_set_url(doc, yaxml_str::NULL, &mut err), &err, (OK, 0));

        // Moves: x (Text) after <b>; then merge it back with append_child.
        let x = related(doc, a, 4).1;
        check(yaxml_move(doc, 2, a, x, &mut node, &mut err), &err, (OK, 0));
        assert_eq!(node, x);
        check(
            yaxml_move(doc, 3, a, x, null_mut(), &mut err),
            &err,
            (OK, 0),
        );
        check(
            yaxml_move(doc, 1, a, r, &mut node, &mut err),
            &err,
            (ERR_EDIT, 3),
        );
        check(
            yaxml_move(doc, 9, a, x, &mut node, &mut err),
            &err,
            (ERR_ARGUMENT, 0),
        );
        check(yaxml_unlink(doc, x, &mut err), &err, (OK, 0));
        check(yaxml_unlink(doc, r, &mut err), &err, (ERR_EDIT, 3));
        check(yaxml_move(doc, 1, a, x, &mut node, &mut err), &err, (OK, 0));
        check(yaxml_copy(doc, a, 1, &mut node, &mut err), &err, (OK, 0));
        let copy = node;
        check(
            yaxml_move(doc, 4, a, copy, &mut node, &mut err),
            &err,
            (OK, 0),
        );
        check(yaxml_remove(doc, a, &mut err), &err, (OK, 0));
        check(yaxml_remove(doc, a, &mut err), &err, (ERR_HANDLE, 0));
        check(yaxml_remove(doc, r, &mut err), &err, (ERR_EDIT, 3));
        check(yaxml_compact(doc, &mut err), &err, (OK, 0));
        check(
            yaxml_copy(doc, r, 0, null_mut(), &mut err),
            &err,
            (ERR_NULL, 0),
        );
        assert_eq!(
            document(doc),
            "<r><b xmlns:n=\"urn:n\">t<![CDATA[c]]><!--new--><?pi d?>x</b><i:item xmlns:i=\"urn:i\"><auto xmlns=\"urn:i\"/></i:item></r>\n"
        );

        // Import from another document; the same document is BUSY.
        let source = parse("<s><n:t xmlns:n='urn:n'/></s>");
        let t = child(source, root(source));
        check(
            yaxml_import(doc, source, t, 1, &mut node, &mut err),
            &err,
            (OK, 0),
        );
        check(
            yaxml_move(doc, 1, r, node, null_mut(), &mut err),
            &err,
            (OK, 0),
        );
        check(
            yaxml_import(doc, doc, r, 1, &mut node, &mut err),
            &err,
            (ERR_BUSY, 0),
        );
        check(
            yaxml_import(doc, source, r, 1, &mut node, &mut err),
            &err,
            (ERR_HANDLE, 0),
        );
        free(source);

        // Limits.
        let limits = yaxml_edit_limits {
            struct_size: size_of::<yaxml_edit_limits>() as u32,
            max_value_bytes: 2,
            ..Default::default()
        };
        check(
            yaxml_doc_set_edit_limits(doc, &limits, &mut err),
            &err,
            (OK, 0),
        );
        check(
            yaxml_append_leaf(doc, r, 3, yaxml_str::NULL, s("long"), &mut node, &mut err),
            &err,
            (ERR_EDIT, 13),
        );
        let short = yaxml_edit_limits {
            struct_size: 4,
            ..limits
        };
        check(
            yaxml_doc_set_edit_limits(doc, &short, &mut err),
            &err,
            (ERR_ARGUMENT, 0),
        );
        check(
            yaxml_doc_set_edit_limits(doc, null(), &mut err),
            &err,
            (OK, 0),
        );
        check(
            yaxml_append_leaf(doc, r, 3, yaxml_str::NULL, s("long"), &mut node, &mut err),
            &err,
            (OK, 0),
        );
    }
    free(doc);
}

#[test]
fn created_and_cloned_documents() {
    let (mut doc, mut err) = (null_mut(), yaxml_error::default());
    let name = spec("root", Some("urn:r"), None, false);
    // SAFETY (whole test): valid pointers; handles are checked.
    unsafe {
        check(yaxml_doc_create(&name, &mut doc, &mut err), &err, (OK, 0));
        assert_eq!(document(doc), "<root xmlns=\"urn:r\"/>\n");
        let mut line = 7;
        assert_eq!(yaxml_node_line(doc, root(doc), &mut line, null_mut()), NONE);
        let bad = spec("", None, None, false);
        let mut other = null_mut();
        check(
            yaxml_doc_create(&bad, &mut other, &mut err),
            &err,
            (ERR_EDIT, 8),
        );
        check(yaxml_doc_clone(doc, &mut other, &mut err), &err, (OK, 0));
        assert_eq!(document(other), document(doc));
        let mut kind = 0;
        // Clones have their own identity.
        assert_eq!(
            yaxml_node_kind(other, root(doc), &mut kind, null_mut()),
            ERR_HANDLE
        );
        check(
            yaxml_doc_clone(null_mut(), &mut other, &mut err),
            &err,
            (ERR_NULL, 0),
        );
        free(other);
    }
    free(doc);
}

fn events(xml: &str, flags: u32) -> Result<Vec<Ev>, (i32, i32)> {
    let (mut reader, mut err) = (null_mut(), yaxml_error::default());
    let o = options(flags);
    // SAFETY: valid pointers.
    let status = unsafe { yaxml_reader_new(xml.as_ptr(), xml.len(), &o, &mut reader, &mut err) };
    if status != OK {
        message(&err);
        return Err((status, err.diagnostic.code));
    }
    let mut list = Vec::new();
    loop {
        let mut event = yaxml_event::default();
        // SAFETY: valid pointers.
        match unsafe { yaxml_reader_next(reader, &mut event, &mut err) } {
            OK => list.push((
                event.kind,
                read(event.name.local).or(read(event.target)),
                read(event.value),
                event.attribute_count,
            )),
            NONE => {
                // SAFETY: as above; EOF repeats.
                assert_eq!(
                    unsafe { yaxml_reader_next(reader, &mut event, null_mut()) },
                    NONE
                );
                break;
            }
            status => {
                message(&err);
                // SAFETY: handles are checked.
                unsafe { yaxml_reader_free(reader) };
                return Err((status, err.diagnostic.code));
            }
        }
    }
    // SAFETY: handles are checked.
    assert_eq!(unsafe { yaxml_reader_free(reader) }, OK);
    // SAFETY: as above.
    assert_eq!(unsafe { yaxml_reader_free(reader) }, ERR_HANDLE);
    Ok(list)
}

#[test]
fn pull_reader_events_and_errors() {
    let list = events("<?xml version='1.0' encoding='UTF-8'?><!DOCTYPE r [<!ENTITY e 'v'>]><r a='1' b='&amp;'>t&e;<![CDATA[c]]><!--m--><?p d?><x/></r>", 0).unwrap();
    let expected = [
        (1, Some("UTF-8"), Some("1.0"), 0),
        (2, None, Some("r"), 0),
        (3, Some("r"), None, 2),
        (6, None, Some("t"), 0),
        (10, None, Some("e"), 0),
        (6, None, Some("v"), 0),
        (11, None, None, 0),
        (7, None, Some("c"), 0),
        (8, None, Some("m"), 0),
        (9, Some("p"), Some("d"), 0),
        (4, Some("x"), None, 0),
        (5, Some("r"), None, 0),
    ];
    let expected: Vec<_> = expected
        .iter()
        .map(|&(k, n, v, c)| (k, n.map(str::to_owned), v.map(str::to_owned), c))
        .collect();
    assert_eq!(list, expected);
    assert_eq!(events("<r><a></b></r>", 0), Err((ERR_XML, 76)));
    // RECOVER: `</b>` ends `a`; DTDVALID needs a tree.
    let recovered = events("<r><a></b></r>", 1).map(|list| list.len());
    assert_eq!(recovered, Ok(4));
    assert_eq!(events("<r/>", 1 << 4), Err((ERR_XML, 115)));
    let mut err = yaxml_error::default();
    let mut event = yaxml_event::default();
    // SAFETY: handles are checked.
    assert_eq!(
        unsafe { yaxml_reader_next(null_mut(), &mut event, &mut err) },
        ERR_NULL
    );
    message(&err);
}

#[derive(Default)]
struct Pushed {
    kinds: Vec<i32>,
    attributes: Vec<(String, String)>,
    stop_at: Option<i32>,
    reentry: Option<*mut yaxml_push>,
    reentry_status: i32,
}

extern "C" fn on_event(user: *mut c_void, event: *const yaxml_event) -> i32 {
    // SAFETY: the tests pass a `Pushed`; the library passes a live event.
    let (pushed, event) = unsafe { (&mut *user.cast::<Pushed>(), &*event) };
    pushed.kinds.push(event.kind);
    // SAFETY: the event lends `attribute_count` attributes.
    for a in unsafe { yaxml_capi_slice(event.attributes, event.attribute_count) } {
        pushed
            .attributes
            .push((read(a.name.qname).unwrap(), read(a.value).unwrap()));
    }
    if let Some(push) = pushed.reentry.take() {
        // SAFETY: re-entrant call on a live handle.
        pushed.reentry_status =
            unsafe { yaxml_push_feed(push, b"x".as_ptr(), 1, None, null_mut(), null_mut()) };
    }
    i32::from(pushed.stop_at == Some(event.kind))
}

fn push(chunks: &[&str], pushed: &mut Pushed, reenter: bool) -> (i32, i32) {
    let (mut parser, mut err) = (null_mut(), yaxml_error::default());
    // SAFETY: valid pointers.
    assert_eq!(
        unsafe { yaxml_push_new(null(), &mut parser, null_mut()) },
        OK
    );
    pushed.reentry = reenter.then_some(parser);
    let user = (pushed as *mut Pushed).cast::<c_void>();
    let mut status = OK;
    for chunk in chunks {
        // SAFETY: `on_event` accepts a Pushed.
        status = unsafe {
            yaxml_push_feed(
                parser,
                chunk.as_ptr(),
                chunk.len(),
                Some(on_event),
                user,
                &mut err,
            )
        };
        if status != OK {
            break;
        }
    }
    if status == OK {
        // SAFETY: as above.
        status = unsafe { yaxml_push_finish(parser, Some(on_event), user, &mut err) };
    }
    let code = err.diagnostic.code;
    if status != OK {
        message(&err);
    }
    // SAFETY: handles are checked.
    assert_eq!(unsafe { yaxml_push_free(parser) }, OK);
    // SAFETY: as above.
    assert_eq!(unsafe { yaxml_push_free(parser) }, ERR_HANDLE);
    (status, code)
}

#[test]
fn push_parser_lends_events_to_the_callback() {
    let mut pushed = Pushed::default();
    assert_eq!(
        push(&["<r a", "='1'>te", "xt</r>"], &mut pushed, false),
        (OK, 0)
    );
    assert_eq!(pushed.kinds, [3, 6, 5]);
    assert_eq!(pushed.attributes, [("a".to_owned(), "1".to_owned())]);

    let mut stopped = Pushed {
        stop_at: Some(3),
        ..Pushed::default()
    };
    assert_eq!(push(&["<r><a/></r>"], &mut stopped, false).0, ERR_CALLBACK);
    assert_eq!(stopped.kinds, [3]);

    let mut malformed = Pushed::default();
    assert_eq!(push(&["<r></x>"], &mut malformed, false), (ERR_XML, 76));

    let mut reentrant = Pushed::default();
    assert_eq!(push(&["<r/>"], &mut reentrant, true), (OK, 0));
    assert_eq!(reentrant.reentry_status, ERR_BUSY);

    // No callback: events are dropped.
    let mut parser = null_mut();
    // SAFETY: valid pointers.
    unsafe {
        assert_eq!(yaxml_push_new(null(), &mut parser, null_mut()), OK);
        assert_eq!(
            yaxml_push_feed(parser, b"<r/>".as_ptr(), 4, None, null_mut(), null_mut()),
            OK
        );
        assert_eq!(yaxml_push_finish(parser, None, null_mut(), null_mut()), OK);
        assert_eq!(
            yaxml_push_feed(parser, null(), 1, None, null_mut(), null_mut()),
            ERR_NULL
        );
        assert_eq!(yaxml_push_free(parser), OK);
        let o = options(1 << 4);
        assert_eq!(yaxml_push_new(&o, &mut parser, null_mut()), ERR_XML);
    }
}
