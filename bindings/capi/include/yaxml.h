/*
 * yaxml C ABI. yaxml-specific: it is not libxml2 symbol-compatible, but it
 * keeps libxml2's numbers (parser flags, save flags, error domains and codes,
 * node types). Reference: docs/bindings.md#c-abi.
 *
 * Conventions
 * - Every function returns a yaxml_status: YAXML_OK, YAXML_NONE (the asked-for
 *   node or value does not exist; out-parameters are set to null values), or a
 *   negative error. On an error the optional `err` is filled and nothing else
 *   is written; its message is owned (free it with yaxml_free).
 * - Handles (yaxml_doc*, yaxml_reader*, yaxml_push*, yaxml_schema*) are opaque
 *   tokens checked on every call: a freed, stale or invented handle is
 *   YAXML_ERR_HANDLE, and a second free is harmless.
 * - Node handles are values: a document identity plus a slot. Every call
 *   validates them; a removed or foreign node is YAXML_ERR_HANDLE.
 * - Strings are UTF-8 pointer/length pairs and are never NUL-terminated. In
 *   input, ptr == NULL means "absent" where documented (else len must be 0).
 *   yaxml_str results are borrowed: from a document they stay valid until the
 *   next editing call on it or yaxml_doc_free; from a reader until the next
 *   call on that reader; in an event callback until it returns. yaxml_buffer
 *   results and every array result are owned: free them with yaxml_free.
 * - No panic crosses this boundary: one becomes YAXML_ERR_PANIC and poisons
 *   the handle involved (later calls: YAXML_ERR_POISONED; free still works).
 * - Threads: any handle may move between threads. Concurrent reads of one
 *   document are allowed; a call that would block (an edit during another
 *   call on the same document, or a re-entrant call from a callback) returns
 *   YAXML_ERR_BUSY instead of waiting.
 */
#ifndef YAXML_H
#define YAXML_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef int32_t yaxml_status;
#define YAXML_OK 0
#define YAXML_NONE 1
#define YAXML_ERR_NULL (-1)     /* a required pointer was NULL */
#define YAXML_ERR_ARGUMENT (-2) /* invalid UTF-8, unknown enum value, bad struct_size */
#define YAXML_ERR_HANDLE (-3)   /* unknown, freed or stale handle or node */
#define YAXML_ERR_BUSY (-4)     /* the handle is in use by another call */
#define YAXML_ERR_POISONED (-5) /* an earlier call panicked on this handle */
#define YAXML_ERR_PANIC (-6)    /* this call panicked; the handle is now poisoned */
#define YAXML_ERR_XML (-7)      /* parse or serialization error (libxml2 diagnostic) */
#define YAXML_ERR_EDIT (-8)     /* editing error (domain YAXML_DOMAIN_EDIT) */
#define YAXML_ERR_XPATH (-9)    /* XPath error (domain 12) */
#define YAXML_ERR_SCHEMA (-10)  /* a schema did not compile */
#define YAXML_ERR_C14N (-11)    /* canonicalization error (domain 21) */
#define YAXML_ERR_CALLBACK (-12) /* a callback stopped the operation or failed */

/* yaxml_diagnostic.domain: libxml2's xmlErrorDomain (1 parser, 3 namespace,
   8 I/O, 12 XPath, 16/17 XSD, 18/19 RELAX NG, 21 C14N, 23 DTD validity,
   28 Schematron), plus yaxml's own. */
#define YAXML_DOMAIN_NONE 0
#define YAXML_DOMAIN_EDIT 1000 /* code: YAXML_EDIT_* */

/* Domain YAXML_DOMAIN_EDIT codes. */
#define YAXML_EDIT_INVALID_HANDLE 1
#define YAXML_EDIT_WRONG_NODE_KIND 2
#define YAXML_EDIT_PROTECTED_NODE 3
#define YAXML_EDIT_MISSING_ATTRIBUTE 4
#define YAXML_EDIT_NAMESPACE_DECLARATION 5
#define YAXML_EDIT_INVALID_CHARACTER 6
#define YAXML_EDIT_INVALID_VALUE 7
#define YAXML_EDIT_INVALID_NAME 8
#define YAXML_EDIT_NAMESPACE 9
#define YAXML_EDIT_DUPLICATE_ATTRIBUTE 10
#define YAXML_EDIT_HIERARCHY 11
#define YAXML_EDIT_IDENTIFIER_EXHAUSTED 12
#define YAXML_EDIT_LIMIT_EXCEEDED 13
#define YAXML_EDIT_ALLOCATION 14

/* Node types: libxml2's xmlElementType. */
#define YAXML_ELEMENT_NODE 1
#define YAXML_ATTRIBUTE_NODE 2 /* XPath items only */
#define YAXML_TEXT_NODE 3
#define YAXML_CDATA_SECTION_NODE 4
#define YAXML_ENTITY_REF_NODE 5
#define YAXML_PI_NODE 7
#define YAXML_COMMENT_NODE 8
#define YAXML_DOCUMENT_NODE 9
#define YAXML_HTML_DOCUMENT_NODE 13
#define YAXML_DTD_NODE 14
#define YAXML_NAMESPACE_DECL 18 /* XPath items only */

/* yaxml_node_relative */
#define YAXML_PARENT 1
#define YAXML_FIRST_CHILD 2
#define YAXML_LAST_CHILD 3
#define YAXML_NEXT_SIBLING 4
#define YAXML_PREVIOUS_SIBLING 5

/* yaxml_move */
#define YAXML_APPEND_CHILD 1  /* xmlAddChild: may merge Text into the last child */
#define YAXML_INSERT_BEFORE 2 /* xmlAddPrevSibling */
#define YAXML_INSERT_AFTER 3  /* xmlAddNextSibling */
#define YAXML_REPLACE 4       /* xmlReplaceNode: target is detached */

/* yaxml_schema_new */
#define YAXML_SCHEMA_XSD 1
#define YAXML_SCHEMA_RELAXNG 2
#define YAXML_SCHEMA_SCHEMATRON 3

/* yaxml_c14n modes: libxml2's xmlC14NMode. */
#define YAXML_C14N_1_0 0
#define YAXML_C14N_EXCLUSIVE_1_0 1
#define YAXML_C14N_1_1 2

/* yaxml_xpath_value.type: libxml2's xmlXPathObjectType. */
#define YAXML_XPATH_NODESET 1
#define YAXML_XPATH_BOOLEAN 2
#define YAXML_XPATH_NUMBER 3
#define YAXML_XPATH_STRING 4

/* yaxml_event.kind */
#define YAXML_EVENT_DECLARATION 1 /* value: version, target: encoding, standalone */
#define YAXML_EVENT_DOCTYPE 2     /* value: name, target: public id, system_id, internal_subset */
#define YAXML_EVENT_START 3       /* name, attributes */
#define YAXML_EVENT_EMPTY 4       /* name, attributes */
#define YAXML_EVENT_END 5         /* name */
#define YAXML_EVENT_TEXT 6        /* value */
#define YAXML_EVENT_CDATA 7       /* value */
#define YAXML_EVENT_COMMENT 8     /* value */
#define YAXML_EVENT_PI 9          /* target, value: data */
#define YAXML_EVENT_ENTITY_START 10 /* value: entity name */
#define YAXML_EVENT_ENTITY_END 11

/* Parser flags: libxml2's xmlParserOption values (yaxml::options). */
#define YAXML_PARSE_RECOVER (1u << 0)
#define YAXML_PARSE_NOENT (1u << 1)
#define YAXML_PARSE_DTDLOAD (1u << 2)
#define YAXML_PARSE_DTDATTR (1u << 3)
#define YAXML_PARSE_DTDVALID (1u << 4)
#define YAXML_PARSE_NOBLANKS (1u << 8)
#define YAXML_PARSE_NSCLEAN (1u << 13)
#define YAXML_PARSE_NOCDATA (1u << 14)
#define YAXML_PARSE_HUGE (1u << 19)
#define YAXML_PARSE_NO_XXE (1u << 23)
#define YAXML_PARSE_SKIP_IDS (1u << 27)

/* Save flags: libxml2's xmlSaveOption values (yaxml::save). */
#define YAXML_SAVE_FORMAT (1u << 0)
#define YAXML_SAVE_NO_DECL (1u << 1)
#define YAXML_SAVE_NO_EMPTY (1u << 2)
#define YAXML_SAVE_AS_XML (1u << 5)
#define YAXML_SAVE_AS_HTML (1u << 6)

typedef struct yaxml_doc yaxml_doc;
typedef struct yaxml_reader yaxml_reader;
typedef struct yaxml_push yaxml_push;
typedef struct yaxml_schema yaxml_schema;

typedef struct yaxml_str {
  const char *ptr;
  size_t len;
} yaxml_str;

/* Owned bytes; free ptr with yaxml_free. Empty results have ptr == NULL. */
typedef struct yaxml_buffer {
  uint8_t *ptr;
  size_t len;
} yaxml_buffer;

/* doc == 0 is the null node. */
typedef struct yaxml_node {
  uint64_t doc;
  uint64_t slot;
} yaxml_node;

/* prefix.ptr and uri.ptr are NULL when absent. */
typedef struct yaxml_name {
  yaxml_str qname;
  yaxml_str prefix;
  yaxml_str local;
  yaxml_str uri;
} yaxml_name;

typedef struct yaxml_attribute {
  yaxml_name name;
  yaxml_str value;
} yaxml_attribute;

/* libxml2's xmlError: domain, code, level (1 warning, 2 error, 3 fatal),
   line and column (0 when unknown). */
typedef struct yaxml_diagnostic {
  int32_t domain;
  int32_t code;
  int32_t level;
  int32_t reserved;
  uint64_t line;
  uint64_t column;
  yaxml_str message;
} yaxml_diagnostic;

/* status is the returned status; offset is the input byte offset of a parse
   error (0 otherwise). diagnostic.message is owned: yaxml_free its ptr. */
typedef struct yaxml_error {
  int32_t status;
  int32_t reserved;
  uint64_t offset;
  yaxml_diagnostic diagnostic;
} yaxml_error;

/* items (and the messages they point to) are one allocation: yaxml_free(items).
   errors counts the items whose level is error or fatal. */
typedef struct yaxml_diagnostics {
  const yaxml_diagnostic *items;
  size_t len;
  size_t errors;
} yaxml_diagnostics;

/* Grows by appending fields: set struct_size = sizeof(yaxml_parse_options).
   A limit of 0 keeps yaxml's default (yaxml::Limits / DecodeLimits). */
typedef struct yaxml_parse_options {
  uint32_t struct_size;
  uint32_t flags; /* YAXML_PARSE_* */
  yaxml_str base; /* document URI, ptr NULL for none */
  uint64_t max_input_bytes;
  uint64_t max_token_bytes;
  uint64_t max_depth;
  uint64_t max_attributes;
  uint64_t max_events;
  uint64_t max_owned_bytes;
  uint64_t max_dtd_bytes;
  uint64_t max_entity_expansion_bytes;
  uint64_t max_entity_amplification;
  uint64_t max_entity_references;
  uint64_t max_entity_depth;
  uint64_t max_external_loads;
  uint64_t max_external_bytes;
  uint64_t max_decoded_bytes;
  uint64_t max_decoded_token_bytes;
} yaxml_parse_options;

/* Grows like yaxml_parse_options; 0 keeps the default (yaxml::EditLimits). */
typedef struct yaxml_edit_limits {
  uint32_t struct_size;
  uint32_t reserved;
  uint64_t max_value_bytes;
  uint64_t max_owned_bytes;
  uint64_t max_created_nodes;
  uint64_t max_name_bytes;
  uint64_t max_supplied_bytes;
  uint64_t max_attributes;
  uint64_t max_depth;
  uint64_t max_created_attributes;
  uint64_t max_namespace_work;
} yaxml_edit_limits;

/* A name for creation: uri.ptr NULL is no namespace; prefix.ptr NULL is no
   prefix; automatic != 0 lets yaxml choose the prefix (prefix ignored). */
typedef struct yaxml_name_spec {
  yaxml_str local;
  yaxml_str uri;
  yaxml_str prefix;
  int32_t automatic;
  int32_t reserved;
} yaxml_name_spec;

/* An XPath namespace binding. */
typedef struct yaxml_ns {
  yaxml_str prefix;
  yaxml_str uri;
} yaxml_ns;

/* type: 0 for a tree node, YAXML_ATTRIBUTE_NODE (index: the element's
   attribute position) or YAXML_NAMESPACE_DECL (index: XPath namespace axis
   position, 0 = xml). */
typedef struct yaxml_xpath_item {
  yaxml_node node;
  int32_t type;
  uint32_t index;
} yaxml_xpath_item;

/* Free string.ptr and items with yaxml_free. */
typedef struct yaxml_xpath_value {
  int32_t type;
  int32_t boolean;
  double number;
  yaxml_buffer string;
  const yaxml_xpath_item *items;
  size_t len;
} yaxml_xpath_value;

/* start/end: the event's input byte range, UINT64_MAX when it has none.
   standalone: -1 absent, 0 no, 1 yes. */
typedef struct yaxml_event {
  int32_t kind;
  int32_t standalone;
  yaxml_name name;
  yaxml_str value;
  yaxml_str target;
  yaxml_str system_id;
  yaxml_str internal_subset;
  const yaxml_attribute *attributes;
  size_t attribute_count;
  uint64_t start;
  uint64_t end;
} yaxml_event;

/* Return 0 to continue; anything else stops with YAXML_ERR_CALLBACK. */
typedef int32_t (*yaxml_event_fn)(void *user, const yaxml_event *event);
/* Write all len bytes and return 0, or nonzero to fail with YAXML_ERR_CALLBACK. */
typedef int32_t (*yaxml_write_fn)(void *user, const uint8_t *bytes, size_t len);

/* ---- library ---- */
const char *yaxml_version(void); /* NUL-terminated, static */
int32_t yaxml_has_feature(int32_t feature); /* libxml2 XML_WITH_* numbers */
void yaxml_free(const void *ptr); /* NULL, unknown and already freed: no-op */
/* Live handles and unfreed allocations. Library tables hold them, so leak
   checkers see them as reachable: a leak test checks both are 0 at exit. */
yaxml_status yaxml_live_objects(size_t *handles, size_t *allocations);

/* ---- documents ---- */
yaxml_status yaxml_parse(const uint8_t *bytes, size_t len,
                         const yaxml_parse_options *options, /* NULL: defaults */
                         yaxml_doc **out, yaxml_error *err);
yaxml_status yaxml_doc_create(const yaxml_name_spec *root, yaxml_doc **out,
                              yaxml_error *err);
yaxml_status yaxml_doc_clone(yaxml_doc *doc, yaxml_doc **out, yaxml_error *err);
yaxml_status yaxml_doc_free(yaxml_doc *doc);
yaxml_status yaxml_doc_root(yaxml_doc *doc, yaxml_node *out, yaxml_error *err);
yaxml_status yaxml_doc_element(yaxml_doc *doc, yaxml_node *out, yaxml_error *err);
yaxml_status yaxml_doc_element_by_id(yaxml_doc *doc, yaxml_str id,
                                     yaxml_node *out, yaxml_error *err);
/* Namespace errors, RECOVER errors and DTDVALID validity errors found while
   parsing. */
yaxml_status yaxml_doc_diagnostics(yaxml_doc *doc, yaxml_diagnostics *out,
                                   yaxml_error *err);
yaxml_status yaxml_doc_set_edit_limits(yaxml_doc *doc,
                                       const yaxml_edit_limits *limits,
                                       yaxml_error *err);

/* ---- nodes ---- */
yaxml_status yaxml_node_kind(yaxml_doc *doc, yaxml_node node, int32_t *out,
                             yaxml_error *err);
yaxml_status yaxml_node_relative(yaxml_doc *doc, yaxml_node node,
                                 int32_t relation, yaxml_node *out,
                                 yaxml_error *err);
/* Elements: their name; PIs: local = target; entity references: local = name. */
yaxml_status yaxml_node_name(yaxml_doc *doc, yaxml_node node, yaxml_name *out,
                             yaxml_error *err);
/* Text, CDATA, comment and PI data. */
yaxml_status yaxml_node_value(yaxml_doc *doc, yaxml_node node, yaxml_str *out,
                              yaxml_error *err);
/* xmlNodeGetContent. */
yaxml_status yaxml_node_content(yaxml_doc *doc, yaxml_node node,
                                yaxml_buffer *out, yaxml_error *err);
/* xmlGetNodePath. */
yaxml_status yaxml_node_path(yaxml_doc *doc, yaxml_node node, yaxml_buffer *out,
                             yaxml_error *err);
/* xmlGetLineNo in the parsed input; NONE for created nodes. */
yaxml_status yaxml_node_line(yaxml_doc *doc, yaxml_node node, uint64_t *out,
                             yaxml_error *err);
yaxml_status yaxml_node_attribute_count(yaxml_doc *doc, yaxml_node node,
                                        size_t *out, yaxml_error *err);
/* Attributes in source order, namespace declarations included. */
yaxml_status yaxml_node_attribute_at(yaxml_doc *doc, yaxml_node node,
                                     size_t index, yaxml_attribute *out,
                                     yaxml_error *err);
/* xmlGetProp: any namespace, then the DTD default. */
yaxml_status yaxml_node_attribute(yaxml_doc *doc, yaxml_node node,
                                  yaxml_str name, yaxml_str *out,
                                  yaxml_error *err);
/* xmlGetNsProp; uri.ptr NULL is no namespace. */
yaxml_status yaxml_node_attribute_ns(yaxml_doc *doc, yaxml_node node,
                                     yaxml_str local, yaxml_str uri,
                                     yaxml_str *out, yaxml_error *err);
/* xmlSearchNs; prefix.ptr NULL is the default namespace. */
yaxml_status yaxml_node_namespace_uri(yaxml_doc *doc, yaxml_node node,
                                      yaxml_str prefix, yaxml_str *out,
                                      yaxml_error *err);
/* xmlSearchNsByHref; OK with out->ptr NULL is the default namespace. */
yaxml_status yaxml_node_namespace_prefix(yaxml_doc *doc, yaxml_node node,
                                         yaxml_str uri, yaxml_str *out,
                                         yaxml_error *err);

/* ---- XPath, output, validation ---- */
yaxml_status yaxml_xpath_eval(yaxml_doc *doc, yaxml_node context,
                              yaxml_str expression, const yaxml_ns *namespaces,
                              size_t namespace_count, yaxml_xpath_value *out,
                              yaxml_error *err);
/* xmlSaveDoc for the document node, xmlNodeDump otherwise. encoding.ptr NULL:
   the document's. */
yaxml_status yaxml_save(yaxml_doc *doc, yaxml_node node, uint32_t flags,
                        yaxml_str encoding, yaxml_buffer *out, yaxml_error *err);
yaxml_status yaxml_save_to(yaxml_doc *doc, yaxml_node node, uint32_t flags,
                           yaxml_str encoding, yaxml_write_fn write, void *user,
                           yaxml_error *err);
/* nodes == NULL: the whole document; else the node-set to render. */
yaxml_status yaxml_c14n(yaxml_doc *doc, int32_t mode, int32_t with_comments,
                        const yaxml_xpath_item *nodes, size_t node_count,
                        const yaxml_str *inclusive_prefixes, size_t prefix_count,
                        yaxml_buffer *out, yaxml_error *err);
/* xmlValidateDocument against the document's DTD. */
yaxml_status yaxml_validate_dtd(yaxml_doc *doc, yaxml_diagnostics *out,
                                yaxml_error *err);
/* Nothing is loaded: includes and imports fail or are skipped. */
yaxml_status yaxml_schema_new(int32_t kind, const uint8_t *bytes, size_t len,
                              yaxml_str base, yaxml_schema **out,
                              yaxml_error *err);
yaxml_status yaxml_schema_free(yaxml_schema *schema);
yaxml_status yaxml_validate(yaxml_doc *doc, yaxml_schema *schema,
                            yaxml_diagnostics *out, yaxml_error *err);

/* ---- editing (each call is atomic: on error nothing changed) ---- */
yaxml_status yaxml_append_element(yaxml_doc *doc, yaxml_node parent,
                                  const yaxml_name_spec *name, yaxml_node *out,
                                  yaxml_error *err);
/* kind: YAXML_TEXT_NODE, YAXML_CDATA_SECTION_NODE, YAXML_COMMENT_NODE or
   YAXML_PI_NODE (target used only for PIs). */
yaxml_status yaxml_append_leaf(yaxml_doc *doc, yaxml_node parent, int32_t kind,
                               yaxml_str target, yaxml_str value,
                               yaxml_node *out, yaxml_error *err);
yaxml_status yaxml_set_value(yaxml_doc *doc, yaxml_node node, yaxml_str value,
                             yaxml_error *err);
yaxml_status yaxml_set_attribute(yaxml_doc *doc, yaxml_node element,
                                 const yaxml_name_spec *name, yaxml_str value,
                                 yaxml_error *err);
/* NONE when the element has no such attribute. */
yaxml_status yaxml_remove_attribute(yaxml_doc *doc, yaxml_node element,
                                    yaxml_str uri, yaxml_str local,
                                    yaxml_error *err);
yaxml_status yaxml_declare_namespace(yaxml_doc *doc, yaxml_node element,
                                     yaxml_str prefix, yaxml_str uri,
                                     yaxml_error *err);
yaxml_status yaxml_set_name(yaxml_doc *doc, yaxml_node element,
                            const yaxml_name_spec *name, yaxml_error *err);
yaxml_status yaxml_set_url(yaxml_doc *doc, yaxml_str url, yaxml_error *err);
/* out (may be NULL) receives the node holding the moved content. */
yaxml_status yaxml_move(yaxml_doc *doc, int32_t operation, yaxml_node target,
                        yaxml_node node, yaxml_node *out, yaxml_error *err);
yaxml_status yaxml_unlink(yaxml_doc *doc, yaxml_node node, yaxml_error *err);
yaxml_status yaxml_remove(yaxml_doc *doc, yaxml_node node, yaxml_error *err);
yaxml_status yaxml_copy(yaxml_doc *doc, yaxml_node node, int32_t deep,
                        yaxml_node *out, yaxml_error *err);
/* source must be another document (the same one is YAXML_ERR_BUSY). */
yaxml_status yaxml_import(yaxml_doc *doc, yaxml_doc *source, yaxml_node node,
                          int32_t deep, yaxml_node *out, yaxml_error *err);
yaxml_status yaxml_compact(yaxml_doc *doc, yaxml_error *err);

/* ---- streaming ---- */
/* The reader copies the input. */
yaxml_status yaxml_reader_new(const uint8_t *bytes, size_t len,
                              const yaxml_parse_options *options,
                              yaxml_reader **out, yaxml_error *err);
/* NONE at the end of the document. */
yaxml_status yaxml_reader_next(yaxml_reader *reader, yaxml_event *out,
                               yaxml_error *err);
/* Up to max (>= 1) events in one call: *events points to *count events
   that, with their strings and attributes, the reader owns until the next
   call on it. OK with *count >= 1 (fewer than max only at the end of the
   document or before an error); NONE with *events NULL and *count 0 at the
   end. Events and errors arrive as from yaxml_reader_next: an error is
   returned by the call that reaches it with no event yet in its batch, and
   every later call repeats it. */
yaxml_status yaxml_reader_next_batch(yaxml_reader *reader, size_t max,
                                     const yaxml_event **events, size_t *count,
                                     yaxml_error *err);
yaxml_status yaxml_reader_free(yaxml_reader *reader);
yaxml_status yaxml_push_new(const yaxml_parse_options *options, yaxml_push **out,
                            yaxml_error *err);
yaxml_status yaxml_push_feed(yaxml_push *push, const uint8_t *bytes, size_t len,
                             yaxml_event_fn callback, void *user,
                             yaxml_error *err);
yaxml_status yaxml_push_finish(yaxml_push *push, yaxml_event_fn callback,
                               void *user, yaxml_error *err);
yaxml_status yaxml_push_free(yaxml_push *push);

#ifdef __cplusplus
}
#endif

#endif /* YAXML_H */
