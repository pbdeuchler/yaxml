//! The C structs of include/yaxml.h, field for field. `tests/abi.rs` pins
//! their sizes and offsets, and `tests/c/abi_test.c` pins the same numbers
//! on the C side.
#![allow(non_camel_case_types)]

use std::ptr;

macro_rules! opaque {
    ($($name:ident),*) => {$(
        /// An opaque handle type; pointers to it are registry tokens.
        #[repr(C)]
        pub struct $name {
            _private: [u8; 0],
        }
    )*};
}

opaque!(yaxml_doc, yaxml_reader, yaxml_push, yaxml_schema);

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct yaxml_str {
    pub ptr: *const u8,
    pub len: usize,
}

impl Default for yaxml_str {
    fn default() -> Self {
        Self::NULL
    }
}

impl yaxml_str {
    pub const NULL: Self = Self {
        ptr: ptr::null(),
        len: 0,
    };

    pub fn of(s: &str) -> Self {
        Self {
            ptr: s.as_ptr(),
            len: s.len(),
        }
    }

    pub fn opt(s: Option<&str>) -> Self {
        s.map_or(Self::NULL, Self::of)
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct yaxml_buffer {
    pub ptr: *mut u8,
    pub len: usize,
}

impl Default for yaxml_buffer {
    fn default() -> Self {
        Self {
            ptr: ptr::null_mut(),
            len: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct yaxml_node {
    pub doc: u64,
    pub slot: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct yaxml_name {
    pub qname: yaxml_str,
    pub prefix: yaxml_str,
    pub local: yaxml_str,
    pub uri: yaxml_str,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct yaxml_attribute {
    pub name: yaxml_name,
    pub value: yaxml_str,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct yaxml_diagnostic {
    pub domain: i32,
    pub code: i32,
    pub level: i32,
    pub reserved: i32,
    pub line: u64,
    pub column: u64,
    pub message: yaxml_str,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct yaxml_error {
    pub status: i32,
    pub reserved: i32,
    pub offset: u64,
    pub diagnostic: yaxml_diagnostic,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct yaxml_diagnostics {
    pub items: *const yaxml_diagnostic,
    pub len: usize,
    pub errors: usize,
}

impl Default for yaxml_diagnostics {
    fn default() -> Self {
        Self {
            items: ptr::null(),
            len: 0,
            errors: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct yaxml_parse_options {
    pub struct_size: u32,
    pub flags: u32,
    pub base: yaxml_str,
    pub max_input_bytes: u64,
    pub max_token_bytes: u64,
    pub max_depth: u64,
    pub max_attributes: u64,
    pub max_events: u64,
    pub max_owned_bytes: u64,
    pub max_dtd_bytes: u64,
    pub max_entity_expansion_bytes: u64,
    pub max_entity_amplification: u64,
    pub max_entity_references: u64,
    pub max_entity_depth: u64,
    pub max_external_loads: u64,
    pub max_external_bytes: u64,
    pub max_decoded_bytes: u64,
    pub max_decoded_token_bytes: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct yaxml_edit_limits {
    pub struct_size: u32,
    pub reserved: u32,
    pub max_value_bytes: u64,
    pub max_owned_bytes: u64,
    pub max_created_nodes: u64,
    pub max_name_bytes: u64,
    pub max_supplied_bytes: u64,
    pub max_attributes: u64,
    pub max_depth: u64,
    pub max_created_attributes: u64,
    pub max_namespace_work: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct yaxml_name_spec {
    pub local: yaxml_str,
    pub uri: yaxml_str,
    pub prefix: yaxml_str,
    pub automatic: i32,
    pub reserved: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct yaxml_ns {
    pub prefix: yaxml_str,
    pub uri: yaxml_str,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct yaxml_xpath_item {
    pub node: yaxml_node,
    pub r#type: i32,
    pub index: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct yaxml_xpath_value {
    pub r#type: i32,
    pub boolean: i32,
    pub number: f64,
    pub string: yaxml_buffer,
    pub items: *const yaxml_xpath_item,
    pub len: usize,
}

impl Default for yaxml_xpath_value {
    fn default() -> Self {
        Self {
            r#type: 0,
            boolean: 0,
            number: 0.0,
            string: yaxml_buffer::default(),
            items: ptr::null(),
            len: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct yaxml_event {
    pub kind: i32,
    pub standalone: i32,
    pub name: yaxml_name,
    pub value: yaxml_str,
    pub target: yaxml_str,
    pub system_id: yaxml_str,
    pub internal_subset: yaxml_str,
    pub attributes: *const yaxml_attribute,
    pub attribute_count: usize,
    pub start: u64,
    pub end: u64,
}

impl Default for yaxml_event {
    fn default() -> Self {
        Self {
            kind: 0,
            standalone: -1,
            name: yaxml_name::default(),
            value: yaxml_str::NULL,
            target: yaxml_str::NULL,
            system_id: yaxml_str::NULL,
            internal_subset: yaxml_str::NULL,
            attributes: ptr::null(),
            attribute_count: 0,
            start: u64::MAX,
            end: u64::MAX,
        }
    }
}
