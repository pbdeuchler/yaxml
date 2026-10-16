/* External matched parsing benchmark. Cargo never compiles or links this file.
 *
 * Work boundaries: input open/stat/read and CLI validation precede all clocks.
 * DOM parsing and DOM observation are timed separately; xmlFreeDoc is timed
 * separately. Reader creation, complete event drain, and observation are one
 * parse interval. Buffered-reader callback reads and SAX push feeds are capped
 * to the requested chunk; their observation remains inside parse time.
 */
#define _POSIX_C_SOURCE 200809L
#include <libxml/parser.h>
#include <libxml/SAX2.h>
#include <libxml/tree.h>
#include <libxml/xmlerror.h>
#include <libxml/xmlreader.h>
#include <libxml/xmlstring.h>
#include <libxml/xmlversion.h>

#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <limits.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <strings.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

#define MAGIC "YAXML-PARSE-BENCH-1"
#define MAX_INPUT_BYTES (8U * 1024U * 1024U)
#define MAX_ITERATIONS 10000U
#define MAX_CHUNK 65536U
#define MAX_TOTAL_WORK (UINT64_C(512) * 1024U * 1024U)
#define XML_OPTIONS (XML_PARSE_NONET | XML_PARSE_NO_XXE)
#define FNV_OFFSET UINT64_C(14695981039346656037)
#define FNV_PRIME UINT64_C(1099511628211)

enum Mode { MODE_DOM, MODE_READER, MODE_PUSH, MODE_BUFFERED };

struct Stats {
    uint64_t elements;
    uint64_t attributes;
    uint64_t namespaces;
    uint64_t text_bytes;
    uint64_t comments;
    uint64_t pis;
    uint64_t hash;
};

struct Observation {
    struct Stats stats;
    size_t depth;
    int text_pending;
    int failed;
    int doctype;
    xmlParserCtxtPtr parser;
};

struct Input {
    unsigned char *bytes;
    size_t length;
    size_t chunk;
};

struct Cursor {
    const struct Input *input;
    size_t offset;
    int failed;
};

struct Totals {
    uint64_t parse_ns;
    uint64_t observe_ns;
    uint64_t drop_ns;
    struct Stats expected;
};

static void fail(int status, const char *message) {
    (void)fprintf(stderr, "%s\n", message);
    exit(status);
}

static uint64_t now_ns(void) {
    struct timespec value;
    if (clock_gettime(CLOCK_MONOTONIC, &value) != 0)
        fail(70, "benchmark clock failure");
    return (uint64_t)value.tv_sec * UINT64_C(1000000000) +
           (uint64_t)value.tv_nsec;
}

static uint64_t add_u64(uint64_t left, uint64_t right, const char *message) {
    if (right > UINT64_MAX - left)
        fail(70, message);
    return left + right;
}

static uint64_t parse_decimal(const char *text, uint64_t maximum,
                              const char *message) {
    uint64_t value = 0;
    const unsigned char *cursor = (const unsigned char *)text;
    if (*cursor == 0)
        fail(64, message);
    while (*cursor != 0) {
        uint64_t digit;
        if (*cursor < (unsigned char)'0' || *cursor > (unsigned char)'9')
            fail(64, message);
        digit = (uint64_t)(*cursor - (unsigned char)'0');
        if (value > maximum / UINT64_C(10) ||
            (value == maximum / UINT64_C(10) &&
             digit > maximum % UINT64_C(10)))
            fail(64, message);
        value = value * UINT64_C(10) + digit;
        ++cursor;
    }
    return value;
}

static enum Mode parse_mode(const char *text) {
    if (strcmp(text, "dom") == 0) return MODE_DOM;
    if (strcmp(text, "reader") == 0) return MODE_READER;
    if (strcmp(text, "push") == 0) return MODE_PUSH;
    if (strcmp(text, "buffered") == 0) return MODE_BUFFERED;
    fail(64, "invalid benchmark mode");
    return MODE_DOM;
}

static const char *mode_name(enum Mode mode) {
    switch (mode) {
        case MODE_DOM: return "dom";
        case MODE_READER: return "reader";
        case MODE_PUSH: return "push";
        case MODE_BUFFERED: return "buffered";
    }
    return "invalid";
}

static struct Input read_input(const char *path, size_t chunk) {
    struct Input input;
    struct stat path_info;
    struct stat info;
    FILE *file;
    int fd;
    size_t length;
    memset(&input, 0, sizeof(input));
    input.chunk = chunk;
    if (stat(path, &path_info) != 0 || !S_ISREG(path_info.st_mode) ||
        path_info.st_size < 0 ||
        (uintmax_t)path_info.st_size > (uintmax_t)MAX_INPUT_BYTES)
        fail(65, "input must be a regular file no larger than 8 MiB");
    fd = open(path, O_RDONLY | O_NONBLOCK);
    if (fd < 0)
        fail(66, "input open failure");
    if (fstat(fd, &info) != 0 || !S_ISREG(info.st_mode) ||
        info.st_size < 0 ||
        (uintmax_t)info.st_size > (uintmax_t)MAX_INPUT_BYTES ||
        path_info.st_dev != info.st_dev || path_info.st_ino != info.st_ino) {
        (void)close(fd);
        fail(65, "input must be a regular file no larger than 8 MiB");
    }
    file = fdopen(fd, "rb");
    if (file == NULL) {
        (void)close(fd);
        fail(66, "input open failure");
    }
    length = (size_t)info.st_size;
    if (length == SIZE_MAX)
        fail(70, "input allocation size overflow");
    input.bytes = (unsigned char *)malloc(length + 1U);
    if (input.bytes == NULL)
        fail(70, "input allocation failure");
    if (length != 0 && fread(input.bytes, 1, length, file) != length)
        fail(66, "input read failure");
    if (fgetc(file) != EOF || ferror(file))
        fail(66, "input changed or read failed");
    if (fclose(file) != 0)
        fail(66, "input close failure");
    input.length = length;
    input.bytes[length] = 0U;
    return input;
}

static void hash_byte(struct Observation *obs, unsigned char byte) {
    obs->stats.hash ^= (uint64_t)byte;
    obs->stats.hash *= FNV_PRIME;
}

static void hash_bytes(struct Observation *obs, const unsigned char *bytes,
                       size_t length) {
    size_t index;
    for (index = 0; index < length; ++index)
        hash_byte(obs, bytes[index]);
}

static void hash_u64_le(struct Observation *obs, uint64_t value) {
    unsigned int index;
    for (index = 0; index < 8U; ++index) {
        hash_byte(obs, (unsigned char)(value & UINT64_C(0xff)));
        value >>= 8;
    }
}

static void hash_field(struct Observation *obs, const unsigned char *bytes,
                       size_t length) {
    hash_u64_le(obs, (uint64_t)length);
    hash_bytes(obs, bytes, length);
}

static size_t xml_len(const xmlChar *value) {
    return value == NULL ? 0U : (size_t)xmlStrlen(value);
}

static void hash_string_field(struct Observation *obs, const xmlChar *value) {
    hash_field(obs, value, xml_len(value));
}

static void hash_qname(struct Observation *obs, const xmlChar *prefix,
                       const xmlChar *local) {
    size_t prefix_len = xml_len(prefix);
    size_t local_len = xml_len(local);
    size_t total = local_len;
    if (prefix_len != 0U) {
        if (prefix_len > SIZE_MAX - local_len - 1U)
            fail(70, "qualified name length overflow");
        total = prefix_len + 1U + local_len;
    }
    hash_u64_le(obs, (uint64_t)total);
    if (prefix_len != 0U) {
        hash_bytes(obs, prefix, prefix_len);
        hash_byte(obs, (unsigned char)':');
    }
    hash_bytes(obs, local, local_len);
}

/* SAX2 keeps ampersands escaped as &#38; so a tree builder can reparse entity
 * references. With DTDs rejected, this is the only such expansion needed. */
static void hash_sax_attribute_field(struct Observation *obs,
                                     const xmlChar *start,
                                     const xmlChar *end) {
    const xmlChar *cursor;
    const xmlChar *limit;
    size_t source_length;
    size_t logical_length;
    if (start == NULL || end == NULL || end < start) {
        obs->failed = 1;
        return;
    }
    source_length = (size_t)(end - start);
    logical_length = source_length;
    for (cursor = start; (size_t)(end - cursor) >= 5U; ++cursor) {
        if (cursor[0] == (xmlChar)'&' && cursor[1] == (xmlChar)'#' &&
            cursor[2] == (xmlChar)'3' && cursor[3] == (xmlChar)'8' &&
            cursor[4] == (xmlChar)';')
            logical_length -= 4U;
    }
    hash_u64_le(obs, (uint64_t)logical_length);
    cursor = start;
    limit = end;
    while (cursor < limit) {
        if ((size_t)(limit - cursor) >= 5U && cursor[0] == (xmlChar)'&' &&
            cursor[1] == (xmlChar)'#' && cursor[2] == (xmlChar)'3' &&
            cursor[3] == (xmlChar)'8' && cursor[4] == (xmlChar)';') {
            hash_byte(obs, (unsigned char)'&');
            cursor += 5;
        } else {
            hash_byte(obs, *cursor++);
        }
    }
}

static void flush_text(struct Observation *obs) {
    if (obs->text_pending) {
        hash_byte(obs, 0U);
        obs->text_pending = 0;
    }
}

static void observe_text(struct Observation *obs, const xmlChar *bytes,
                         size_t length) {
    if (length == 0U || obs->depth == 0U)
        return;
    if (bytes == NULL || (uint64_t)length > UINT64_MAX - obs->stats.text_bytes) {
        obs->failed = 1;
        return;
    }
    if (!obs->text_pending) {
        hash_byte(obs, (unsigned char)'T');
        obs->text_pending = 1;
    }
    hash_bytes(obs, bytes, length);
    obs->stats.text_bytes += (uint64_t)length;
}

static void observe_start_begin(struct Observation *obs,
                                const xmlChar *local,
                                const xmlChar *prefix,
                                const xmlChar *uri) {
    flush_text(obs);
    hash_byte(obs, (unsigned char)'S');
    hash_qname(obs, prefix, local);
    hash_string_field(obs, uri);
}

static void observe_start_end(struct Observation *obs) {
    hash_byte(obs, (unsigned char)'>');
    obs->stats.elements = add_u64(obs->stats.elements, 1U,
                                  "element count overflow");
}

static void observe_end(struct Observation *obs, const xmlChar *local,
                        const xmlChar *prefix, const xmlChar *uri) {
    flush_text(obs);
    hash_byte(obs, (unsigned char)'E');
    hash_qname(obs, prefix, local);
    hash_string_field(obs, uri);
}

static void observe_comment(struct Observation *obs, const xmlChar *value) {
    flush_text(obs);
    hash_byte(obs, (unsigned char)'C');
    hash_string_field(obs, value);
    obs->stats.comments = add_u64(obs->stats.comments, 1U,
                                  "comment count overflow");
}

static void observe_pi(struct Observation *obs, const xmlChar *target,
                       const xmlChar *data) {
    flush_text(obs);
    hash_byte(obs, (unsigned char)'P');
    hash_string_field(obs, target);
    hash_string_field(obs, data);
    obs->stats.pis = add_u64(obs->stats.pis, 1U, "PI count overflow");
}

static void observe_dom_start(struct Observation *obs, xmlNodePtr node) {
    xmlNsPtr ns;
    xmlAttrPtr attr;
    observe_start_begin(obs, node->name, node->ns == NULL ? NULL : node->ns->prefix,
                       node->ns == NULL ? NULL : node->ns->href);
    for (ns = node->nsDef; ns != NULL; ns = ns->next) {
        hash_byte(obs, (unsigned char)'N');
        hash_string_field(obs, ns->prefix);
        hash_string_field(obs, ns->href);
        obs->stats.namespaces = add_u64(obs->stats.namespaces, 1U,
                                        "namespace count overflow");
    }
    for (attr = node->properties; attr != NULL; attr = attr->next) {
        xmlChar *value = xmlNodeListGetString(node->doc, attr->children, 1);
        hash_byte(obs, (unsigned char)'A');
        hash_qname(obs, attr->ns == NULL ? NULL : attr->ns->prefix,
                   attr->name);
        hash_string_field(obs, attr->ns == NULL ? NULL : attr->ns->href);
        if (value == NULL && attr->children != NULL) {
            obs->failed = 1;
            return;
        }
        hash_string_field(obs, value);
        if (value != NULL)
            xmlFree(value);
        obs->stats.attributes = add_u64(obs->stats.attributes, 1U,
                                        "attribute count overflow");
    }
    observe_start_end(obs);
}

static void observe_dom(struct Observation *obs, xmlDocPtr doc) {
    xmlNodePtr node = doc->children;
    while (node != NULL && !obs->failed) {
        if (node->type == XML_ELEMENT_NODE) {
            observe_dom_start(obs, node);
            obs->depth = (size_t)add_u64((uint64_t)obs->depth, 1U,
                                         "element depth overflow");
            if (node->children != NULL) {
                node = node->children;
                continue;
            }
            observe_end(obs, node->name,
                        node->ns == NULL ? NULL : node->ns->prefix,
                        node->ns == NULL ? NULL : node->ns->href);
            --obs->depth;
        } else if (node->type == XML_TEXT_NODE ||
                   node->type == XML_CDATA_SECTION_NODE) {
            if (node->content != NULL)
                observe_text(obs, node->content, xml_len(node->content));
        } else if (node->type == XML_COMMENT_NODE) {
            observe_comment(obs, node->content);
        } else if (node->type == XML_PI_NODE) {
            observe_pi(obs, node->name, node->content);
        } else if (node->type == XML_DTD_NODE ||
                   node->type == XML_ENTITY_REF_NODE) {
            obs->doctype = 1;
            return;
        }
        while (node != NULL && node->next == NULL) {
            xmlNodePtr parent = node->parent;
            if (parent == NULL || parent->type == XML_DOCUMENT_NODE ||
                parent->type == XML_HTML_DOCUMENT_NODE) {
                node = NULL;
                break;
            }
            if (parent->type == XML_ELEMENT_NODE) {
                observe_end(obs, parent->name,
                            parent->ns == NULL ? NULL : parent->ns->prefix,
                            parent->ns == NULL ? NULL : parent->ns->href);
                if (obs->depth == 0U) {
                    obs->failed = 1;
                    break;
                }
                --obs->depth;
            }
            node = parent;
        }
        if (node != NULL)
            node = node->next;
    }
    flush_text(obs);
}

static void reader_start(struct Observation *obs, xmlTextReaderPtr reader) {
    const xmlChar *local = xmlTextReaderConstLocalName(reader);
    const xmlChar *prefix = xmlTextReaderConstPrefix(reader);
    const xmlChar *uri = xmlTextReaderConstNamespaceUri(reader);
    int result;
    observe_start_begin(obs, local, prefix, uri);
    result = xmlTextReaderMoveToFirstAttribute(reader);
    while (result == 1) {
        if (xmlTextReaderIsNamespaceDecl(reader) == 1) {
            const xmlChar *attr_prefix = xmlTextReaderConstPrefix(reader);
            const xmlChar *attr_local = xmlTextReaderConstLocalName(reader);
            const xmlChar *value = xmlTextReaderConstValue(reader);
            const xmlChar *declared_prefix =
                (attr_prefix != NULL && xmlStrEqual(attr_prefix, BAD_CAST "xmlns"))
                    ? attr_local : NULL;
            hash_byte(obs, (unsigned char)'N');
            hash_string_field(obs, declared_prefix);
            hash_string_field(obs, value);
            obs->stats.namespaces = add_u64(obs->stats.namespaces, 1U,
                                            "namespace count overflow");
        }
        result = xmlTextReaderMoveToNextAttribute(reader);
    }
    if (result < 0) obs->failed = 1;
    if (xmlTextReaderMoveToElement(reader) < 0) obs->failed = 1;
    result = xmlTextReaderMoveToFirstAttribute(reader);
    while (result == 1) {
        if (xmlTextReaderIsNamespaceDecl(reader) != 1) {
            hash_byte(obs, (unsigned char)'A');
            hash_qname(obs, xmlTextReaderConstPrefix(reader),
                       xmlTextReaderConstLocalName(reader));
            hash_string_field(obs, xmlTextReaderConstNamespaceUri(reader));
            hash_string_field(obs, xmlTextReaderConstValue(reader));
            obs->stats.attributes = add_u64(obs->stats.attributes, 1U,
                                            "attribute count overflow");
        }
        result = xmlTextReaderMoveToNextAttribute(reader);
    }
    if (result < 0) obs->failed = 1;
    if (xmlTextReaderMoveToElement(reader) < 0) obs->failed = 1;
    observe_start_end(obs);
}

static void reader_events(struct Observation *obs, xmlTextReaderPtr reader) {
    int result;
    while ((result = xmlTextReaderRead(reader)) == 1) {
        int type = xmlTextReaderNodeType(reader);
        const xmlChar *value;
        switch (type) {
            case XML_READER_TYPE_ELEMENT:
                reader_start(obs, reader);
                if (xmlTextReaderIsEmptyElement(reader) == 1)
                    observe_end(obs, xmlTextReaderConstLocalName(reader),
                                xmlTextReaderConstPrefix(reader),
                                xmlTextReaderConstNamespaceUri(reader));
                else
                    obs->depth = add_u64((uint64_t)obs->depth, 1U,
                                         "element depth overflow");
                break;
            case XML_READER_TYPE_END_ELEMENT:
                if (obs->depth == 0U) {
                    obs->failed = 1;
                    return;
                }
                observe_end(obs, xmlTextReaderConstLocalName(reader),
                            xmlTextReaderConstPrefix(reader),
                            xmlTextReaderConstNamespaceUri(reader));
                --obs->depth;
                break;
            case XML_READER_TYPE_TEXT:
            case XML_READER_TYPE_CDATA:
            case XML_READER_TYPE_WHITESPACE:
            case XML_READER_TYPE_SIGNIFICANT_WHITESPACE:
                value = xmlTextReaderConstValue(reader);
                observe_text(obs, value, xml_len(value));
                break;
            case XML_READER_TYPE_COMMENT:
                observe_comment(obs, xmlTextReaderConstValue(reader));
                break;
            case XML_READER_TYPE_PROCESSING_INSTRUCTION:
                observe_pi(obs, xmlTextReaderConstName(reader),
                           xmlTextReaderConstValue(reader));
                break;
            case XML_READER_TYPE_DOCUMENT_TYPE:
                obs->doctype = 1;
                return;
            case XML_READER_TYPE_ENTITY_REFERENCE:
            case XML_READER_TYPE_ENTITY:
                obs->failed = 1;
                return;
            default:
                break;
        }
        if (obs->failed)
            return;
    }
    if (result < 0 || obs->depth != 0U)
        obs->failed = 1;
    flush_text(obs);
}

static int reader_io(void *context, char *buffer, int length) {
    struct Cursor *cursor = (struct Cursor *)context;
    size_t remaining;
    size_t count;
    if (length < 0) {
        cursor->failed = 1;
        return -1;
    }
    remaining = cursor->input->length - cursor->offset;
    if (remaining == 0U || length == 0)
        return 0;
    count = (size_t)length;
    if (count > cursor->input->chunk)
        count = cursor->input->chunk;
    if (count > remaining)
        count = remaining;
    memcpy(buffer, cursor->input->bytes + cursor->offset, count);
    cursor->offset += count;
    if (count > (size_t)INT_MAX) {
        cursor->failed = 1;
        return -1;
    }
    return (int)count;
}

static int reader_close(void *context) {
    (void)context;
    return 0;
}

static void reader_error(void *context, const char *message,
                         xmlParserSeverities severity,
                         xmlTextReaderLocatorPtr locator) {
    struct Observation *obs = (struct Observation *)context;
    (void)message;
    (void)locator;
    if (severity >= XML_PARSER_SEVERITY_ERROR)
        obs->failed = 1;
}

static void structured_error(void *context, const xmlError *error) {
    struct Observation *obs = (struct Observation *)context;
    if (error != NULL && error->level >= XML_ERR_ERROR)
        obs->failed = 1;
}

static void sax_error(void *context, const char *message, ...) {
    struct Observation *obs = (struct Observation *)context;
    (void)message;
    obs->failed = 1;
}

static void sax_start(void *context, const xmlChar *local,
                      const xmlChar *prefix, const xmlChar *uri,
                      int namespace_count, const xmlChar **namespaces,
                      int attribute_count, int defaulted,
                      const xmlChar **attributes) {
    struct Observation *obs = (struct Observation *)context;
    int index;
    (void)defaulted;
    if (namespace_count < 0 || attribute_count < 0) {
        obs->failed = 1;
        return;
    }
    observe_start_begin(obs, local, prefix, uri);
    for (index = 0; index < namespace_count; ++index) {
        const xmlChar *declared_prefix = namespaces[(size_t)index * 2U];
        const xmlChar *declared_uri = namespaces[(size_t)index * 2U + 1U];
        hash_byte(obs, (unsigned char)'N');
        hash_string_field(obs, declared_prefix);
        hash_string_field(obs, declared_uri);
        obs->stats.namespaces = add_u64(obs->stats.namespaces, 1U,
                                        "namespace count overflow");
    }
    for (index = 0; index < attribute_count; ++index) {
        size_t base = (size_t)index * 5U;
        hash_byte(obs, (unsigned char)'A');
        hash_qname(obs, attributes[base + 1U], attributes[base]);
        hash_string_field(obs, attributes[base + 2U]);
        hash_sax_attribute_field(obs, attributes[base + 3U],
                                 attributes[base + 4U]);
        obs->stats.attributes = add_u64(obs->stats.attributes, 1U,
                                        "attribute count overflow");
    }
    observe_start_end(obs);
    obs->depth = (size_t)add_u64((uint64_t)obs->depth, 1U,
                                 "element depth overflow");
}

static void sax_end(void *context, const xmlChar *local,
                    const xmlChar *prefix, const xmlChar *uri) {
    struct Observation *obs = (struct Observation *)context;
    if (obs->depth == 0U) {
        obs->failed = 1;
        return;
    }
    observe_end(obs, local, prefix, uri);
    --obs->depth;
}

static void sax_characters(void *context, const xmlChar *bytes, int length) {
    struct Observation *obs = (struct Observation *)context;
    if (length < 0) {
        obs->failed = 1;
        return;
    }
    observe_text(obs, bytes, (size_t)length);
}

static void sax_comment(void *context, const xmlChar *value) {
    observe_comment((struct Observation *)context, value);
}

static void sax_pi(void *context, const xmlChar *target,
                   const xmlChar *data) {
    observe_pi((struct Observation *)context, target, data);
}

static void sax_doctype(void *context, const xmlChar *name,
                        const xmlChar *external_id,
                        const xmlChar *system_id) {
    struct Observation *obs = (struct Observation *)context;
    (void)name;
    (void)external_id;
    (void)system_id;
    obs->doctype = 1;
    if (obs->parser != NULL)
        xmlStopParser(obs->parser);
}

static xmlEntityPtr sax_get_entity(void *context, const xmlChar *name) {
    (void)context;
    return xmlGetPredefinedEntity(name);
}

static struct Observation new_observation(void) {
    struct Observation obs;
    memset(&obs, 0, sizeof(obs));
    obs.stats.hash = FNV_OFFSET;
    return obs;
}

static int accepted_version(const xmlChar *value) {
    return value == NULL || xmlStrEqual(value, BAD_CAST "1.0");
}

static int accepted_encoding(const xmlChar *value) {
    return value == NULL || strcasecmp((const char *)value, "UTF-8") == 0;
}

static void validate_dom(xmlDocPtr doc, struct Observation *obs) {
    if (doc == NULL || doc->intSubset != NULL || doc->extSubset != NULL ||
        !accepted_version(doc->version) || !accepted_encoding(doc->encoding))
        obs->failed = 1;
}

static void run_dom(const struct Input *input, size_t iterations,
                    struct Totals *totals) {
    size_t iteration;
    for (iteration = 0; iteration < iterations; ++iteration) {
        struct Observation obs = new_observation();
        xmlDocPtr doc;
        uint64_t started = now_ns();
        xmlSetStructuredErrorFunc(&obs, structured_error);
        doc = xmlReadMemory((const char *)input->bytes, (int)input->length,
                            NULL, NULL, XML_OPTIONS);
        xmlSetStructuredErrorFunc(NULL, NULL);
        totals->parse_ns = add_u64(totals->parse_ns, now_ns() - started,
                                   "parse time overflow");
        validate_dom(doc, &obs);
        if (doc != NULL && !obs.failed && !obs.doctype) {
            started = now_ns();
            observe_dom(&obs, doc);
            totals->observe_ns = add_u64(totals->observe_ns,
                                         now_ns() - started,
                                         "observe time overflow");
        }
        if (doc != NULL) {
            started = now_ns();
            xmlFreeDoc(doc);
            totals->drop_ns = add_u64(totals->drop_ns, now_ns() - started,
                                      "drop time overflow");
        }
        if (doc == NULL || obs.failed || obs.doctype)
            fail(65, "input is not supported well-formed XML");
        if (iteration == 0U)
            totals->expected = obs.stats;
        else if (memcmp(&totals->expected, &obs.stats, sizeof(obs.stats)) != 0)
            fail(70, "semantic result changed between iterations");
    }
}

static void validate_reader(xmlTextReaderPtr reader,
                            struct Observation *obs) {
    if (reader == NULL ||
        !accepted_version(xmlTextReaderConstXmlVersion(reader)) ||
        !accepted_encoding(xmlTextReaderConstEncoding(reader)))
        obs->failed = 1;
}

static void run_reader_mode(const struct Input *input, size_t iterations,
                            struct Totals *totals, int buffered) {
    size_t iteration;
    for (iteration = 0; iteration < iterations; ++iteration) {
        struct Observation obs = new_observation();
        struct Cursor cursor;
        xmlTextReaderPtr reader;
        uint64_t started;
        memset(&cursor, 0, sizeof(cursor));
        cursor.input = input;
        started = now_ns();
        if (buffered) {
            reader = xmlReaderForIO(reader_io, reader_close, &cursor, NULL,
                                    NULL, XML_OPTIONS);
        } else {
            /* xmlReaderForMemory copies its input into an input buffer. */
            reader = xmlReaderForMemory((const char *)input->bytes,
                                        (int)input->length, NULL, NULL,
                                        XML_OPTIONS);
        }
        if (reader == NULL)
            obs.failed = 1;
        else {
            xmlTextReaderSetErrorHandler(reader, reader_error, &obs);
            reader_events(&obs, reader);
            validate_reader(reader, &obs);
        }
        totals->parse_ns = add_u64(totals->parse_ns, now_ns() - started,
                                   "parse time overflow");
        if (reader != NULL) {
            started = now_ns();
            xmlFreeTextReader(reader);
            totals->drop_ns = add_u64(totals->drop_ns, now_ns() - started,
                                      "drop time overflow");
        }
        if (cursor.failed || (buffered && cursor.offset != input->length) ||
            obs.failed || obs.doctype)
            fail(65, "input is not supported well-formed XML");
        if (iteration == 0U)
            totals->expected = obs.stats;
        else if (memcmp(&totals->expected, &obs.stats, sizeof(obs.stats)) != 0)
            fail(70, "semantic result changed between iterations");
    }
}

static void run_push(const struct Input *input, size_t iterations,
                     struct Totals *totals) {
    size_t iteration;
    for (iteration = 0; iteration < iterations; ++iteration) {
        struct Observation obs = new_observation();
        xmlSAXHandler sax;
        xmlParserCtxtPtr ctxt;
        uint64_t started;
        size_t offset = 0U;
        int result = 0;
        memset(&sax, 0, sizeof(sax));
        sax.initialized = XML_SAX2_MAGIC;
        sax.startElementNs = sax_start;
        sax.endElementNs = sax_end;
        sax.characters = sax_characters;
        sax.ignorableWhitespace = sax_characters;
        sax.cdataBlock = sax_characters;
        sax.comment = sax_comment;
        sax.processingInstruction = sax_pi;
        sax.internalSubset = sax_doctype;
        sax.externalSubset = sax_doctype;
        sax.getEntity = sax_get_entity;
        sax.warning = sax_error;
        sax.error = sax_error;
        sax.fatalError = sax_error;
        started = now_ns();
        ctxt = xmlCreatePushParserCtxt(&sax, &obs, NULL, 0, NULL);
        if (ctxt == NULL)
            obs.failed = 1;
        else {
            obs.parser = ctxt;
            (void)xmlCtxtSetOptions(ctxt, XML_OPTIONS);
            xmlCtxtSetErrorHandler(ctxt, structured_error, &obs);
            while (offset < input->length && result == 0) {
                size_t count = input->length - offset;
                if (count > input->chunk)
                    count = input->chunk;
                result = xmlParseChunk(ctxt,
                                       (const char *)input->bytes + offset,
                                       (int)count, 0);
                offset += count;
                if (obs.doctype)
                    result = -1;
            }
            if (result == 0)
                result = xmlParseChunk(ctxt, NULL, 0, 1);
            if (ctxt->wellFormed == 0 || !accepted_version(ctxt->version) ||
                !accepted_encoding(xmlCtxtGetDeclaredEncoding(ctxt)))
                obs.failed = 1;
        }
        if (!obs.failed && !obs.doctype && obs.depth == 0U)
            flush_text(&obs);
        totals->parse_ns = add_u64(totals->parse_ns, now_ns() - started,
                                   "parse time overflow");
        if (ctxt != NULL) {
            started = now_ns();
            xmlFreeParserCtxt(ctxt);
            totals->drop_ns = add_u64(totals->drop_ns, now_ns() - started,
                                      "drop time overflow");
        }
        if (result != 0 || obs.failed || obs.doctype || obs.depth != 0U)
            fail(65, "input is not supported well-formed XML");
        if (iteration == 0U)
            totals->expected = obs.stats;
        else if (memcmp(&totals->expected, &obs.stats, sizeof(obs.stats)) != 0)
            fail(70, "semantic result changed between iterations");
    }
}

static void check_version(void) {
    if (LIBXML_VERSION != 21504 || xmlParserVersion == NULL ||
        strcmp(xmlParserVersion, "21504") != 0)
        fail(70, "libxml2 compile/runtime version must be 2.15.4 (21504)");
}

int main(int argc, char **argv) {
    enum Mode mode;
    struct Input input;
    struct Totals totals;
    uint64_t iterations64;
    uint64_t chunk64;
    uint64_t work;
    size_t iterations;
    size_t chunk;
    xmlInitParser();
    check_version();
    if (argc == 2 && strcmp(argv[1], "--version") == 0) {
        (void)printf("YAXML-PARSE-BENCH-1\tengine=libxml2\tversion=21504\n");
        return fflush(stdout) == 0 ? 0 : 74;
    }
    if (argc != 5)
        fail(64, "usage: libxml2-parse-bench MODE INPUT ITERATIONS CHUNK");
    mode = parse_mode(argv[1]);
    iterations64 = parse_decimal(argv[3], MAX_ITERATIONS,
                                 "invalid iterations (expected 1..10000)");
    chunk64 = parse_decimal(argv[4], MAX_CHUNK,
                             "invalid chunk (expected 1..65536)");
    if (iterations64 == 0U || chunk64 == 0U)
        fail(64, "iterations and chunk must be positive");
    iterations = (size_t)iterations64;
    chunk = (size_t)chunk64;
    input = read_input(argv[2], chunk);
    if (memchr(input.bytes, 0, input.length) != NULL ||
        xmlCheckUTF8(input.bytes) == 0)
        fail(65, "input must use UTF-8");
    if (input.length > UINT64_MAX / iterations64)
        fail(64, "aggregate work size overflow");
    work = (uint64_t)input.length * iterations64;
    if (work > MAX_TOTAL_WORK)
        fail(64, "aggregate work exceeds 512 MiB");
    memset(&totals, 0, sizeof(totals));
    if (mode == MODE_DOM)
        run_dom(&input, iterations, &totals);
    else if (mode == MODE_READER)
        run_reader_mode(&input, iterations, &totals, 0);
    else if (mode == MODE_BUFFERED)
        run_reader_mode(&input, iterations, &totals, 1);
    else
        run_push(&input, iterations, &totals);
    (void)printf(MAGIC "\tengine=libxml2\tmode=%s\tbytes=%zu\titerations=%zu"
                 "\tchunk=%zu\tparse_ns=%" PRIu64 "\tobserve_ns=%" PRIu64
                 "\tdrop_ns=%" PRIu64 "\telements=%" PRIu64
                 "\tattributes=%" PRIu64 "\tnamespaces=%" PRIu64
                 "\ttext_bytes=%" PRIu64 "\tcomments=%" PRIu64
                 "\tpis=%" PRIu64 "\thash=%016" PRIx64 "\n",
                 mode_name(mode), input.length, iterations, chunk,
                 totals.parse_ns, totals.observe_ns, totals.drop_ns,
                 totals.expected.elements, totals.expected.attributes,
                 totals.expected.namespaces, totals.expected.text_bytes,
                 totals.expected.comments, totals.expected.pis,
                 totals.expected.hash);
    free(input.bytes);
    xmlCleanupParser();
    return fflush(stdout) == 0 ? 0 : 74;
}
