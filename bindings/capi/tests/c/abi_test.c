/* Calls every function of yaxml.h, happy and sad paths, and frees every
   allocation. Built with -fsanitize=address,undefined (and run under LSan or
   macOS `leaks`) by bindings/capi/tests/c/run.sh. */
#include <stdbool.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "yaxml.h"

/* Layout pins, the same numbers as tests/abi.rs. */
_Static_assert(sizeof(void *) != 8 || sizeof(yaxml_str) == 16, "yaxml_str");
_Static_assert(sizeof(void *) != 8 || sizeof(yaxml_name) == 64, "yaxml_name");
_Static_assert(sizeof(void *) != 8 || sizeof(yaxml_attribute) == 80, "yaxml_attribute");
_Static_assert(sizeof(void *) != 8 || sizeof(yaxml_diagnostic) == 48, "yaxml_diagnostic");
_Static_assert(sizeof(void *) != 8 || sizeof(yaxml_error) == 64, "yaxml_error");
_Static_assert(sizeof(void *) != 8 || sizeof(yaxml_diagnostics) == 24, "yaxml_diagnostics");
_Static_assert(sizeof(void *) != 8 || sizeof(yaxml_parse_options) == 144, "options");
_Static_assert(sizeof(void *) != 8 || sizeof(yaxml_edit_limits) == 80, "limits");
_Static_assert(sizeof(void *) != 8 || sizeof(yaxml_name_spec) == 56, "name spec");
_Static_assert(sizeof(void *) != 8 || sizeof(yaxml_ns) == 32, "ns");
_Static_assert(sizeof(void *) != 8 || sizeof(yaxml_xpath_item) == 24, "item");
_Static_assert(sizeof(void *) != 8 || sizeof(yaxml_xpath_value) == 48, "value");
_Static_assert(sizeof(void *) != 8 || sizeof(yaxml_event) == 168, "event");
_Static_assert(sizeof(void *) != 8 || offsetof(yaxml_error, diagnostic) == 16, "error.diagnostic");
_Static_assert(sizeof(void *) != 8 || offsetof(yaxml_diagnostic, message) == 32, "message");
_Static_assert(sizeof(void *) != 8 || offsetof(yaxml_parse_options, base) == 8, "base");
_Static_assert(sizeof(void *) != 8 || offsetof(yaxml_parse_options, max_decoded_token_bytes) == 136, "options end");
_Static_assert(sizeof(void *) != 8 || offsetof(yaxml_edit_limits, max_namespace_work) == 72, "limits end");
_Static_assert(sizeof(void *) != 8 || offsetof(yaxml_name_spec, automatic) == 48, "automatic");
_Static_assert(sizeof(void *) != 8 || offsetof(yaxml_xpath_value, string) == 16, "string");
_Static_assert(sizeof(void *) != 8 || offsetof(yaxml_event, name) == 8, "event.name");
_Static_assert(sizeof(void *) != 8 || offsetof(yaxml_event, attributes) == 136, "event.attributes");
_Static_assert(sizeof(void *) != 8 || offsetof(yaxml_event, end) == 160, "event.end");

static int failures = 0;

#define CHECK(cond)                                                            \
  do {                                                                         \
    if (!(cond)) {                                                             \
      fprintf(stderr, "%s:%d: CHECK failed: %s\n", __FILE__, __LINE__, #cond); \
      failures++;                                                              \
    }                                                                          \
  } while (0)

/* Expects `want`; on an error checks and frees err's message. */
#define EXPECT(call, want)                                                   \
  do {                                                                         \
    yaxml_status got_ = (call);                                                \
    if (got_ != (want)) {                                                    \
      fprintf(stderr, "%s:%d: %s = %d, expected %d (%.*s)\n", __FILE__,        \
              __LINE__, #call, got_, (want),                                 \
              (int)err.diagnostic.message.len, err.diagnostic.message.ptr);    \
      failures++;                                                              \
    }                                                                          \
    if (got_ < 0) {                                                            \
      CHECK(err.status == got_);                                               \
      yaxml_free(err.diagnostic.message.ptr);                                  \
      yaxml_free(err.diagnostic.message.ptr); /* second free: no-op */         \
    }                                                                          \
    memset(&err, 0, sizeof err);                                               \
  } while (0)

static yaxml_error err;

static yaxml_str S(const char *s) {
  yaxml_str out = {s, strlen(s)};
  return out;
}

static const yaxml_str NUL_STR = {NULL, 0};

static bool eq(yaxml_str s, const char *expected) {
  return s.ptr != NULL && s.len == strlen(expected) &&
         memcmp(s.ptr, expected, s.len) == 0;
}

static bool buf_eq(yaxml_buffer b, const char *expected) {
  yaxml_str s = {(const char *)b.ptr, b.len};
  bool ok = (b.ptr == NULL && expected[0] == 0) || eq(s, expected);
  yaxml_free(b.ptr);
  return ok;
}

static yaxml_doc *parse(const char *xml, const yaxml_parse_options *o) {
  yaxml_doc *doc = NULL;
  EXPECT(yaxml_parse((const uint8_t *)xml, strlen(xml), o, &doc, &err),
         YAXML_OK);
  return doc;
}

static yaxml_node rel(yaxml_doc *doc, yaxml_node n, int32_t r) {
  yaxml_node out = {0, 0};
  yaxml_node_relative(doc, n, r, &out, NULL);
  return out;
}

static yaxml_node element(yaxml_doc *doc) {
  yaxml_node out = {0, 0};
  EXPECT(yaxml_doc_element(doc, &out, &err), YAXML_OK);
  return out;
}

static void library(void) {
  CHECK(strlen(yaxml_version()) > 0);
  CHECK(yaxml_has_feature(4) == 1);
  CHECK(yaxml_has_feature(-3) == 0);
  yaxml_free(NULL);
}

static void documents(void) {
  yaxml_parse_options o;
  memset(&o, 0, sizeof o);
  o.struct_size = sizeof o;
  o.flags = YAXML_PARSE_DTDVALID;
  o.base = S("file:///doc.xml");
  yaxml_doc *doc = parse("<!DOCTYPE r [<!ELEMENT r (a)><!ELEMENT a EMPTY>]>\n"
                         "<r xml:id='x'><b/></r>",
                         &o);

  yaxml_diagnostics d = {0};
  EXPECT(yaxml_doc_diagnostics(doc, &d, &err), YAXML_OK);
  CHECK(d.len >= 1 && d.errors >= 1 && d.items[0].domain == 23);
  CHECK(d.items[0].line == 2 && d.items[0].message.len > 0);
  yaxml_free(d.items);
  EXPECT(yaxml_validate_dtd(doc, &d, &err), YAXML_OK);
  CHECK(d.len >= 1);
  yaxml_free(d.items);

  yaxml_node root = {0, 0}, r = element(doc), found = {0, 0};
  EXPECT(yaxml_doc_root(doc, &root, &err), YAXML_OK);
  CHECK(rel(doc, r, YAXML_PARENT).slot == root.slot);
  EXPECT(yaxml_doc_element_by_id(doc, S("x"), &found, &err), YAXML_OK);
  CHECK(found.slot == r.slot);
  EXPECT(yaxml_doc_element_by_id(doc, S("y"), &found, &err), YAXML_NONE);

  yaxml_edit_limits limits;
  memset(&limits, 0, sizeof limits);
  limits.struct_size = sizeof limits;
  limits.max_depth = 4;
  EXPECT(yaxml_doc_set_edit_limits(doc, &limits, &err), YAXML_OK);
  limits.struct_size = 3;
  EXPECT(yaxml_doc_set_edit_limits(doc, &limits, &err), YAXML_ERR_ARGUMENT);

  yaxml_doc *copy = NULL;
  EXPECT(yaxml_doc_clone(doc, &copy, &err), YAXML_OK);
  CHECK(yaxml_doc_free(copy) == YAXML_OK);
  CHECK(yaxml_doc_free(copy) == YAXML_ERR_HANDLE);
  CHECK(yaxml_doc_free(NULL) == YAXML_ERR_NULL);
  EXPECT(yaxml_doc_root(copy, &root, &err), YAXML_ERR_HANDLE);

  yaxml_name_spec name = {S("made"), S("urn:m"), S("m"), 0, 0};
  EXPECT(yaxml_doc_create(&name, &copy, &err), YAXML_OK);
  yaxml_doc_free(copy);
  name.local = S("1bad");
  EXPECT(yaxml_doc_create(&name, &copy, &err), YAXML_ERR_EDIT);

  /* Sad parses. */
  yaxml_doc *bad = NULL;
  EXPECT(yaxml_parse((const uint8_t *)"<a><b></a>", 10, NULL, &bad, &err),
         YAXML_ERR_XML);
  {
    yaxml_error e = {0};
    yaxml_parse((const uint8_t *)"<a>\n<b></c>", 11, NULL, &bad, &e);
    CHECK(e.diagnostic.domain == 1 && e.diagnostic.code == 76 &&
          e.diagnostic.line == 2);
    yaxml_free(e.diagnostic.message.ptr);
  }
  EXPECT(yaxml_parse(NULL, 5, NULL, &bad, &err), YAXML_ERR_NULL);
  EXPECT(yaxml_parse((const uint8_t *)"<r/>", 4, NULL, NULL, &err),
         YAXML_ERR_NULL);
  o.struct_size = 4;
  EXPECT(yaxml_parse((const uint8_t *)"<r/>", 4, &o, &bad, &err),
         YAXML_ERR_ARGUMENT);
  yaxml_doc_free(doc);
}

static void nodes(void) {
  yaxml_doc *doc = parse("<p:r xmlns:p='urn:p' xmlns='urn:d' a='1' p:b='2'>"
                         "t<?pi d?><!--c--><k/></p:r>",
                         NULL);
  yaxml_node r = element(doc), text = rel(doc, r, YAXML_FIRST_CHILD);
  yaxml_node pi = rel(doc, text, YAXML_NEXT_SIBLING);
  yaxml_node k = rel(doc, r, YAXML_LAST_CHILD), out;
  int32_t kind = 0;
  EXPECT(yaxml_node_kind(doc, pi, &kind, &err), YAXML_OK);
  CHECK(kind == YAXML_PI_NODE);
  EXPECT(yaxml_node_kind(doc, pi, NULL, &err), YAXML_ERR_NULL);
  EXPECT(yaxml_node_relative(doc, text, YAXML_PREVIOUS_SIBLING, &out, &err),
         YAXML_NONE);
  CHECK(out.doc == 0);
  EXPECT(yaxml_node_relative(doc, text, 77, &out, &err), YAXML_ERR_ARGUMENT);

  yaxml_name name;
  EXPECT(yaxml_node_name(doc, r, &name, &err), YAXML_OK);
  CHECK(eq(name.qname, "p:r") && eq(name.prefix, "p") && eq(name.local, "r") &&
        eq(name.uri, "urn:p"));
  EXPECT(yaxml_node_name(doc, k, &name, &err), YAXML_OK);
  CHECK(name.prefix.ptr == NULL && eq(name.uri, "urn:d"));
  EXPECT(yaxml_node_name(doc, text, &name, &err), YAXML_NONE);

  yaxml_str value;
  EXPECT(yaxml_node_value(doc, text, &value, &err), YAXML_OK);
  CHECK(eq(value, "t"));
  EXPECT(yaxml_node_value(doc, r, &value, &err), YAXML_NONE);

  yaxml_buffer buffer;
  EXPECT(yaxml_node_content(doc, r, &buffer, &err), YAXML_OK);
  CHECK(buf_eq(buffer, "t"));
  EXPECT(yaxml_node_path(doc, k, &buffer, &err), YAXML_OK);
  CHECK(buf_eq(buffer, "/p:r/*"));
  uint64_t line = 0;
  EXPECT(yaxml_node_line(doc, k, &line, &err), YAXML_OK);
  CHECK(line == 1);

  size_t count = 0;
  EXPECT(yaxml_node_attribute_count(doc, r, &count, &err), YAXML_OK);
  CHECK(count == 4);
  yaxml_attribute attribute;
  EXPECT(yaxml_node_attribute_at(doc, r, 3, &attribute, &err), YAXML_OK);
  CHECK(eq(attribute.name.local, "b") && eq(attribute.value, "2"));
  EXPECT(yaxml_node_attribute_at(doc, r, 4, &attribute, &err), YAXML_NONE);
  EXPECT(yaxml_node_attribute(doc, r, S("a"), &value, &err), YAXML_OK);
  CHECK(eq(value, "1"));
  EXPECT(yaxml_node_attribute(doc, r, S("zz"), &value, &err), YAXML_NONE);
  yaxml_str invalid = {"\xff", 1};
  EXPECT(yaxml_node_attribute(doc, r, invalid, &value, &err),
         YAXML_ERR_ARGUMENT);
  EXPECT(yaxml_node_attribute_ns(doc, r, S("b"), S("urn:p"), &value, &err),
         YAXML_OK);
  EXPECT(yaxml_node_attribute_ns(doc, r, S("b"), NUL_STR, &value, &err),
         YAXML_NONE);
  EXPECT(yaxml_node_namespace_uri(doc, k, NUL_STR, &value, &err), YAXML_OK);
  CHECK(eq(value, "urn:d"));
  EXPECT(yaxml_node_namespace_uri(doc, k, S("q"), &value, &err), YAXML_NONE);
  EXPECT(yaxml_node_namespace_prefix(doc, k, S("urn:p"), &value, &err),
         YAXML_OK);
  CHECK(eq(value, "p"));
  EXPECT(yaxml_node_namespace_prefix(doc, k, S("urn:x"), &value, &err),
         YAXML_NONE);

  yaxml_node forged = {r.doc, 1u << 30};
  EXPECT(yaxml_node_kind(doc, forged, &kind, &err), YAXML_ERR_HANDLE);
  yaxml_doc_free(doc);
}

static void xpath_and_c14n(void) {
  yaxml_doc *doc = parse("<r xmlns:p='urn:p' b='2' a='1'><!--c--><p:x k='1'/></r>", NULL);
  yaxml_node r = element(doc);
  yaxml_ns ns = {S("q"), S("urn:p")};
  yaxml_xpath_value v;
  EXPECT(yaxml_xpath_eval(doc, r, S("//q:x | //q:x/@k"), &ns, 1, &v, &err),
         YAXML_OK);
  CHECK(v.type == YAXML_XPATH_NODESET && v.len == 2);
  CHECK(v.items[0].type == 0 && v.items[1].type == YAXML_ATTRIBUTE_NODE);
  yaxml_buffer out;
  EXPECT(yaxml_c14n(doc, YAXML_C14N_EXCLUSIVE_1_0, 0, v.items, 1, NULL, 0,
                    &out, &err),
         YAXML_OK);
  /* The namespace node is not in the set, so its declaration is not output. */
  CHECK(buf_eq(out, "<p:x></p:x>"));
  yaxml_free(v.items);
  EXPECT(yaxml_xpath_eval(doc, r, S("string(@a)"), NULL, 0, &v, &err), YAXML_OK);
  CHECK(v.type == YAXML_XPATH_STRING && buf_eq(v.string, "1"));
  EXPECT(yaxml_xpath_eval(doc, r, S("count(*) = 1"), NULL, 0, &v, &err), YAXML_OK);
  CHECK(v.type == YAXML_XPATH_BOOLEAN && v.boolean == 1);
  EXPECT(yaxml_xpath_eval(doc, r, S("count(//*)"), NULL, 0, &v, &err), YAXML_OK);
  CHECK(v.type == YAXML_XPATH_NUMBER && v.number == 2.0);
  {
    yaxml_error e = {0};
    CHECK(yaxml_xpath_eval(doc, r, S("'open"), NULL, 0, &v, &e) == YAXML_ERR_XPATH);
    CHECK(e.diagnostic.domain == 12 && e.diagnostic.code == 1202);
    yaxml_free(e.diagnostic.message.ptr);
  }

  EXPECT(yaxml_c14n(doc, YAXML_C14N_1_0, 1, NULL, 0, NULL, 0, &out, &err), YAXML_OK);
  CHECK(buf_eq(out, "<r xmlns:p=\"urn:p\" a=\"1\" b=\"2\"><!--c--><p:x k=\"1\"></p:x></r>"));
  EXPECT(yaxml_c14n(doc, YAXML_C14N_1_1, 0, NULL, 0, NULL, 0, &out, &err), YAXML_OK);
  yaxml_free(out.ptr);
  EXPECT(yaxml_c14n(doc, 42, 0, NULL, 0, NULL, 0, &out, &err), YAXML_ERR_ARGUMENT);
  yaxml_doc_free(doc);
}

static int32_t collect(void *user, const uint8_t *bytes, size_t len) {
  size_t *total = user;
  (void)bytes;
  *total += len;
  return 0;
}

static int32_t fail_write(void *user, const uint8_t *bytes, size_t len) {
  (void)user, (void)bytes, (void)len;
  return 1;
}

static void output(void) {
  yaxml_doc *doc = parse("<r><a>caf\xc3\xa9</a></r>", NULL);
  yaxml_node root = {0, 0}, r = element(doc);
  yaxml_doc_root(doc, &root, NULL);
  yaxml_buffer out;
  EXPECT(yaxml_save(doc, root, YAXML_SAVE_NO_DECL, NUL_STR, &out, &err), YAXML_OK);
  CHECK(buf_eq(out, "<r><a>caf&#xE9;</a></r>\n"));
  EXPECT(yaxml_save(doc, r, YAXML_SAVE_FORMAT, S("UTF-8"), &out, &err), YAXML_OK);
  CHECK(buf_eq(out, "<r>\n  <a>caf\xc3\xa9</a>\n</r>"));
  EXPECT(yaxml_save(doc, r, 0, S("nope"), &out, &err), YAXML_ERR_XML);
  size_t total = 0;
  EXPECT(yaxml_save_to(doc, root, 0, NUL_STR, collect, &total, &err), YAXML_OK);
  CHECK(total == strlen("<?xml version=\"1.0\"?>\n<r><a>caf&#xE9;</a></r>\n"));
  EXPECT(yaxml_save_to(doc, root, 0, NUL_STR, fail_write, NULL, &err),
         YAXML_ERR_CALLBACK);
  EXPECT(yaxml_save_to(doc, root, 0, NUL_STR, NULL, NULL, &err), YAXML_ERR_NULL);
  yaxml_doc_free(doc);
}

static void schemas(void) {
  static const char *xsd =
      "<xs:schema xmlns:xs='http://www.w3.org/2001/XMLSchema'>"
      "<xs:element name='r' type='xs:int'/></xs:schema>";
  static const char *rng =
      "<element name='r' xmlns='http://relaxng.org/ns/structure/1.0'><text/></element>";
  static const char *sch =
      "<schema xmlns='http://purl.oclc.org/dsdl/schematron'><pattern id='p'>"
      "<rule context='i'><assert test='@ok'>needs ok</assert></rule></pattern></schema>";
  const char *texts[] = {xsd, rng, sch};
  const char *bad[] = {"<r>x</r>", "<r><x/></r>", "<r><i/></r>"};
  int32_t domains[] = {17, 19, 28};
  for (int i = 0; i < 3; i++) {
    yaxml_schema *schema = NULL;
    EXPECT(yaxml_schema_new(i + 1, (const uint8_t *)texts[i], strlen(texts[i]),
                            S("s.xml"), &schema, &err),
           YAXML_OK);
    yaxml_doc *doc = parse(bad[i], NULL);
    yaxml_diagnostics d = {0};
    EXPECT(yaxml_validate(doc, schema, &d, &err), YAXML_OK);
    CHECK(d.len >= 1 && d.errors >= 1 && d.items[0].domain == domains[i]);
    yaxml_free(d.items);
    CHECK(yaxml_schema_free(schema) == YAXML_OK);
    CHECK(yaxml_schema_free(schema) == YAXML_ERR_HANDLE);
    EXPECT(yaxml_validate(doc, schema, &d, &err), YAXML_ERR_HANDLE);
    yaxml_doc_free(doc);
    EXPECT(yaxml_schema_new(i + 1, (const uint8_t *)"<x/>", 4, NUL_STR, &schema, &err),
           YAXML_ERR_SCHEMA);
  }
  yaxml_schema *schema = NULL;
  EXPECT(yaxml_schema_new(9, (const uint8_t *)xsd, strlen(xsd), NUL_STR, &schema, &err),
         YAXML_ERR_ARGUMENT);
}

static void editing(void) {
  yaxml_doc *doc = parse("<r><a/>x</r>", NULL), *other = parse("<s><t/></s>", NULL);
  yaxml_node r = element(doc), a = rel(doc, r, YAXML_FIRST_CHILD);
  yaxml_node x = rel(doc, a, YAXML_NEXT_SIBLING), n, leaf;
  yaxml_name_spec item = {S("item"), S("urn:i"), NUL_STR, 1, 0};
  EXPECT(yaxml_append_element(doc, r, &item, &n, &err), YAXML_OK);
  EXPECT(yaxml_append_element(doc, r, NULL, &n, &err), YAXML_ERR_NULL);
  EXPECT(yaxml_append_leaf(doc, a, YAXML_TEXT_NODE, NUL_STR, S("t"), &leaf, &err), YAXML_OK);
  EXPECT(yaxml_append_leaf(doc, a, YAXML_CDATA_SECTION_NODE, NUL_STR, S("c"), &n, &err), YAXML_OK);
  EXPECT(yaxml_append_leaf(doc, a, YAXML_COMMENT_NODE, NUL_STR, S("m"), &n, &err), YAXML_OK);
  EXPECT(yaxml_append_leaf(doc, a, YAXML_PI_NODE, S("pi"), S("d"), &n, &err), YAXML_OK);
  EXPECT(yaxml_append_leaf(doc, a, 99, NUL_STR, S("d"), &n, &err), YAXML_ERR_ARGUMENT);
  EXPECT(yaxml_set_value(doc, leaf, S("T"), &err), YAXML_OK);
  EXPECT(yaxml_set_value(doc, a, S("T"), &err), YAXML_ERR_EDIT);
  yaxml_name_spec k = {S("k"), NUL_STR, NUL_STR, 0, 0};
  EXPECT(yaxml_set_attribute(doc, a, &k, S("v"), &err), YAXML_OK);
  EXPECT(yaxml_remove_attribute(doc, a, NUL_STR, S("k"), &err), YAXML_OK);
  EXPECT(yaxml_remove_attribute(doc, a, NUL_STR, S("k"), &err), YAXML_NONE);
  EXPECT(yaxml_declare_namespace(doc, a, S("n"), S("urn:n"), &err), YAXML_OK);
  EXPECT(yaxml_declare_namespace(doc, a, S("xml"), S("urn:n"), &err), YAXML_ERR_EDIT);
  yaxml_name_spec b = {S("b"), NUL_STR, NUL_STR, 0, 0};
  EXPECT(yaxml_set_name(doc, a, &b, &err), YAXML_OK);
  EXPECT(yaxml_set_url(doc, S("file:///d.xml"), &err), YAXML_OK);
  EXPECT(yaxml_move(doc, YAXML_INSERT_BEFORE, a, x, &n, &err), YAXML_OK);
  EXPECT(yaxml_move(doc, YAXML_INSERT_AFTER, a, x, NULL, &err), YAXML_OK);
  EXPECT(yaxml_move(doc, YAXML_APPEND_CHILD, a, r, &n, &err), YAXML_ERR_EDIT);
  EXPECT(yaxml_move(doc, 0, a, x, &n, &err), YAXML_ERR_ARGUMENT);
  EXPECT(yaxml_unlink(doc, x, &err), YAXML_OK);
  EXPECT(yaxml_move(doc, YAXML_APPEND_CHILD, a, x, &n, &err), YAXML_OK);
  EXPECT(yaxml_copy(doc, a, 1, &n, &err), YAXML_OK);
  EXPECT(yaxml_move(doc, YAXML_REPLACE, a, n, NULL, &err), YAXML_OK);
  EXPECT(yaxml_remove(doc, a, &err), YAXML_OK);
  EXPECT(yaxml_remove(doc, a, &err), YAXML_ERR_HANDLE);
  EXPECT(yaxml_remove(doc, r, &err), YAXML_ERR_EDIT);
  EXPECT(yaxml_unlink(doc, r, &err), YAXML_ERR_EDIT);
  yaxml_node t = rel(other, element(other), YAXML_FIRST_CHILD);
  EXPECT(yaxml_import(doc, other, t, 1, &n, &err), YAXML_OK);
  EXPECT(yaxml_import(doc, doc, r, 1, &n, &err), YAXML_ERR_BUSY);
  EXPECT(yaxml_import(doc, other, r, 1, &n, &err), YAXML_ERR_HANDLE);
  EXPECT(yaxml_compact(doc, &err), YAXML_OK);
  EXPECT(yaxml_compact(NULL, &err), YAXML_ERR_NULL);
  yaxml_doc_free(other);
  yaxml_doc_free(doc);
}

static void streaming_reader(void) {
  const char *xml = "<?xml version='1.0'?><r a='1'>t<!--c--></r>";
  yaxml_reader *reader = NULL;
  EXPECT(yaxml_reader_new((const uint8_t *)xml, strlen(xml), NULL, &reader, &err), YAXML_OK);
  int32_t kinds[8] = {0};
  int n = 0;
  yaxml_event e;
  yaxml_status status;
  while ((status = yaxml_reader_next(reader, &e, &err)) == YAXML_OK && n < 8) {
    kinds[n++] = e.kind;
    if (e.kind == YAXML_EVENT_START) {
      CHECK(e.attribute_count == 1 && eq(e.attributes[0].value, "1"));
      CHECK(e.start == 21);
    }
  }
  CHECK(status == YAXML_NONE && n == 5);
  CHECK(kinds[0] == YAXML_EVENT_DECLARATION && kinds[4] == YAXML_EVENT_END);
  CHECK(yaxml_reader_free(reader) == YAXML_OK);
  CHECK(yaxml_reader_free(reader) == YAXML_ERR_HANDLE);
  EXPECT(yaxml_reader_next(reader, &e, &err), YAXML_ERR_HANDLE);

  EXPECT(yaxml_reader_new((const uint8_t *)"<r></x>", 7, NULL, &reader, &err), YAXML_OK);
  EXPECT(yaxml_reader_next(reader, &e, &err), YAXML_OK);
  EXPECT(yaxml_reader_next(reader, &e, &err), YAXML_ERR_XML);
  yaxml_reader_free(reader);
  yaxml_parse_options o;
  memset(&o, 0, sizeof o);
  o.struct_size = sizeof o;
  o.flags = YAXML_PARSE_RECOVER;
  /* Recovery: `</x>` ends `r`, and the document ends. */
  EXPECT(yaxml_reader_new((const uint8_t *)"<r></x>", 7, &o, &reader, &err), YAXML_OK);
  EXPECT(yaxml_reader_next(reader, &e, &err), YAXML_OK);
  EXPECT(yaxml_reader_next(reader, &e, &err), YAXML_OK);
  CHECK(e.kind == YAXML_EVENT_END);
  EXPECT(yaxml_reader_next(reader, &e, &err), YAXML_NONE);
  yaxml_reader_free(reader);
}

struct pushed {
  int events;
  int stop_after;
};

static int32_t on_event(void *user, const yaxml_event *event) {
  struct pushed *p = user;
  (void)event;
  p->events++;
  return p->stop_after != 0 && p->events >= p->stop_after;
}

static void streaming_push(void) {
  yaxml_push *push = NULL;
  struct pushed p = {0, 0};
  EXPECT(yaxml_push_new(NULL, &push, &err), YAXML_OK);
  EXPECT(yaxml_push_feed(push, (const uint8_t *)"<r><a/>te", 9, on_event, &p, &err), YAXML_OK);
  EXPECT(yaxml_push_feed(push, (const uint8_t *)"xt</r>", 6, on_event, &p, &err), YAXML_OK);
  EXPECT(yaxml_push_finish(push, on_event, &p, &err), YAXML_OK);
  CHECK(p.events == 4);
  CHECK(yaxml_push_free(push) == YAXML_OK);
  CHECK(yaxml_push_free(push) == YAXML_ERR_HANDLE);

  struct pushed stop = {0, 1};
  EXPECT(yaxml_push_new(NULL, &push, &err), YAXML_OK);
  EXPECT(yaxml_push_feed(push, (const uint8_t *)"<r><a/></r>", 11, on_event, &stop, &err),
         YAXML_ERR_CALLBACK);
  EXPECT(yaxml_push_finish(push, NULL, NULL, &err), YAXML_ERR_CALLBACK);
  yaxml_push_free(push);

  EXPECT(yaxml_push_new(NULL, &push, &err), YAXML_OK);
  EXPECT(yaxml_push_feed(push, (const uint8_t *)"<r></x>", 7, NULL, NULL, &err), YAXML_ERR_XML);
  yaxml_push_free(push);
  yaxml_parse_options o;
  memset(&o, 0, sizeof o);
  o.struct_size = sizeof o;
  o.flags = YAXML_PARSE_DTDVALID;
  EXPECT(yaxml_push_new(&o, &push, &err), YAXML_ERR_XML);
}

int main(void) {
  library();
  documents();
  nodes();
  xpath_and_c14n();
  output();
  schemas();
  editing();
  streaming_reader();
  streaming_push();
  size_t handles = 1, allocations = 1;
  CHECK(yaxml_live_objects(&handles, &allocations) == YAXML_OK);
  CHECK(handles == 0 && allocations == 0);
  CHECK(yaxml_live_objects(NULL, &allocations) == YAXML_ERR_NULL);
  if (failures != 0) {
    fprintf(stderr, "%d failures\n", failures);
    return 1;
  }
  puts("abi_test: ok");
  return 0;
}
