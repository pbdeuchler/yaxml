/* External matched benchmark only. Cargo never compiles or links this file. */
#define _POSIX_C_SOURCE 200809L
#include <libxml/parser.h>
#include <libxml/tree.h>
#include <libxml/xmlsave.h>
#include <libxml/xmlversion.h>

#include <errno.h>
#include <inttypes.h>
#include <limits.h>
#include <stdint.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#define MAGIC "YAXML-STRUCTURE-BENCH-1"
#define MAX_FRAME (128U * 1024U * 1024U)
#define MAX_PAYLOAD (1024U * 1024U)
#define MAX_DIMENSION 1000000U

enum CaseKind {
    APPEND_TEXT,
    APPEND_CDATA,
    APPEND_COMMENT,
    APPEND_PI,
    REMOVE_FIRST,
    REMOVE_MIDDLE,
    REMOVE_LAST,
    REMOVE_DEEP,
    CHURN
};

struct Request {
    enum CaseKind kind;
    const char *case_label;
    char encoding[16];
    size_t width;
    size_t depth;
    size_t payload_bytes;
    size_t iterations;
    size_t operations;
    size_t traversal_passes;
    size_t serialization_passes;
    unsigned char *input;
    size_t input_length;
    unsigned char *payload;
    size_t payload_length;
    unsigned char *expected;
    size_t expected_length;
};

struct Target {
    xmlNode *node;
    size_t subtree_nodes;
};

struct Sink {
    size_t bytes;
    size_t writes;
    int failed;
};

static void fail(int code, const char *message) {
    fprintf(stderr, "%s\n", message);
    exit(code);
}

static void *allocate(size_t count, size_t size) {
    void *value;
    if (count != 0 && size > SIZE_MAX / count)
        fail(65, "matched benchmark allocation size overflow");
    value = calloc(count == 0 ? 1 : count, size);
    if (value == NULL)
        fail(70, "matched benchmark allocation failure");
    return value;
}

static uint64_t now_ns(void) {
    struct timespec value;
    if (clock_gettime(CLOCK_MONOTONIC, &value) != 0)
        fail(70, "matched benchmark clock failure");
    return (uint64_t)value.tv_sec * UINT64_C(1000000000) +
           (uint64_t)value.tv_nsec;
}

static uint64_t add_u64(uint64_t left, uint64_t right, const char *message) {
    if (right > UINT64_MAX - left)
        fail(70, message);
    return left + right;
}

static size_t add_size(size_t left, size_t right, const char *message) {
    if (right > SIZE_MAX - left)
        fail(70, message);
    return left + right;
}

static size_t read_number(FILE *input, int delimiter, size_t maximum) {
    size_t value = 0;
    size_t digits = 0;
    int byte;
    while ((byte = fgetc(input)) != delimiter) {
        if (byte == EOF || byte < '0' || byte > '9' || ++digits > 20)
            fail(65, "invalid matched benchmark number");
        if (value > maximum / 10 ||
            (value == maximum / 10 && (size_t)(byte - '0') > maximum % 10))
            fail(65, "matched benchmark number exceeds ceiling");
        value = value * 10 + (size_t)(byte - '0');
    }
    if (digits == 0)
        fail(65, "empty matched benchmark number");
    return value;
}

static void read_line(FILE *input, char *output, size_t capacity,
                      const char *message) {
    size_t length;
    if (fgets(output, (int)capacity, input) == NULL)
        fail(65, message);
    length = strlen(output);
    if (length == 0 || output[length - 1] != '\n')
        fail(65, message);
    output[length - 1] = 0;
}

static unsigned char *read_frame(FILE *input, size_t maximum, size_t *length) {
    unsigned char *value;
    *length = read_number(input, ':', maximum);
    value = allocate(add_size(*length, 1, "matched frame size overflow"), 1);
    if (fread(value, 1, *length, input) != *length || fgetc(input) != '\n')
        fail(65, "truncated matched benchmark frame");
    value[*length] = 0;
    return value;
}

static enum CaseKind parse_case(const char *value) {
    if (strcmp(value, "append_text") == 0) return APPEND_TEXT;
    if (strcmp(value, "append_cdata") == 0) return APPEND_CDATA;
    if (strcmp(value, "append_comment") == 0) return APPEND_COMMENT;
    if (strcmp(value, "append_pi") == 0) return APPEND_PI;
    if (strcmp(value, "remove_first") == 0) return REMOVE_FIRST;
    if (strcmp(value, "remove_middle") == 0) return REMOVE_MIDDLE;
    if (strcmp(value, "remove_last") == 0) return REMOVE_LAST;
    if (strcmp(value, "remove_deep") == 0) return REMOVE_DEEP;
    if (strcmp(value, "churn") == 0) return CHURN;
    fail(65, "unknown matched benchmark case");
    return APPEND_TEXT;
}

static struct Request read_request(const char *path) {
    struct Request request;
    char line[64];
    FILE *input = fopen(path, "rb");
    if (input == NULL)
        fail(66, "matched benchmark request open failure");
    memset(&request, 0, sizeof(request));
    read_line(input, line, sizeof(line), "invalid matched benchmark header");
    if (strcmp(line, MAGIC) != 0)
        fail(65, "invalid matched benchmark header");
    read_line(input, line, sizeof(line), "invalid matched benchmark case");
    request.kind = parse_case(line);
    request.case_label = strdup(line);
    if (request.case_label == NULL)
        fail(70, "matched benchmark case allocation failure");
    read_line(input, request.encoding, sizeof(request.encoding),
              "invalid matched benchmark encoding");
    if (strcmp(request.encoding, "utf8") != 0 &&
        strcmp(request.encoding, "utf16le") != 0 &&
        strcmp(request.encoding, "utf16be") != 0)
        fail(65, "unknown matched benchmark encoding");
    request.width = read_number(input, '\n', MAX_DIMENSION);
    request.depth = read_number(input, '\n', MAX_DIMENSION);
    request.payload_bytes = read_number(input, '\n', MAX_PAYLOAD);
    request.iterations = read_number(input, '\n', MAX_DIMENSION);
    request.operations = read_number(input, '\n', MAX_DIMENSION);
    request.traversal_passes = read_number(input, '\n', MAX_DIMENSION);
    request.serialization_passes = read_number(input, '\n', MAX_DIMENSION);
    if (request.width == 0 || request.depth == 0 || request.iterations == 0 ||
        request.operations == 0 || request.traversal_passes == 0 ||
        request.serialization_passes == 0)
        fail(65, "matched benchmark dimensions must be positive");
    request.input = read_frame(input, MAX_FRAME, &request.input_length);
    request.payload = read_frame(input, MAX_PAYLOAD, &request.payload_length);
    request.expected = read_frame(input, MAX_FRAME, &request.expected_length);
    if (request.payload_length != request.payload_bytes)
        fail(65, "matched benchmark payload length mismatch");
    if (memchr(request.payload, 0, request.payload_length) != NULL)
        fail(65, "matched benchmark payload contains NUL");
    if (request.input_length > INT_MAX || request.payload_length > INT_MAX)
        fail(65, "matched benchmark frame exceeds libxml2 API range");
    if (fgetc(input) != EOF)
        fail(65, "trailing matched benchmark request data");
    if (fclose(input) != 0)
        fail(74, "matched benchmark request close failure");
    return request;
}

static int is_removal(enum CaseKind kind) {
    return kind == REMOVE_FIRST || kind == REMOVE_MIDDLE ||
           kind == REMOVE_LAST || kind == REMOVE_DEEP;
}

static xmlNode *first_element_child(xmlNode *parent) {
    xmlNode *node;
    for (node = parent->children; node != NULL; node = node->next) {
        if (node->type == XML_ELEMENT_NODE)
            return node;
    }
    return NULL;
}

static xmlNode *selected_parent(xmlDoc *doc, const struct Request *request) {
    xmlNode *parent = xmlDocGetRootElement(doc);
    size_t level;
    if (parent == NULL)
        fail(65, "matched benchmark document has no root");
    if (request->kind == REMOVE_DEEP)
        return parent;
    for (level = 1; level < request->depth; ++level) {
        parent = first_element_child(parent);
        if (parent == NULL)
            fail(65, "matched benchmark parent depth mismatch");
    }
    return parent;
}

static size_t visit_tree(xmlNode *root, uint64_t *checksum) {
    xmlNode *node = root;
    size_t count = 0;
    while (node != NULL) {
        const xmlChar *text = NULL;
        xmlAttr *attribute;
        size_t length;
        ++count;
        *checksum ^= (uint64_t)node->type + UINT64_C(0x9e3779b97f4a7c15);
        *checksum = (*checksum << 13) | (*checksum >> 51);
        if (node->name != NULL) {
            length = strlen((const char *)node->name);
            if (node->ns != NULL && node->ns->prefix != NULL)
                length = add_size(length,
                    add_size(strlen((const char *)node->ns->prefix), 1,
                             "qualified name length overflow"),
                    "qualified name length overflow");
            *checksum ^= (uint64_t)length;
        }
        if (node->ns != NULL && node->ns->href != NULL)
            *checksum ^= (uint64_t)strlen((const char *)node->ns->href);
        if (node->type == XML_TEXT_NODE || node->type == XML_CDATA_SECTION_NODE ||
            node->type == XML_COMMENT_NODE || node->type == XML_PI_NODE)
            text = node->content;
        if (text != NULL) {
            length = strlen((const char *)text);
            *checksum ^= (uint64_t)length;
            if (length != 0)
                *checksum ^= (uint64_t)text[0];
        }
        for (attribute = node->properties; attribute != NULL;
             attribute = attribute->next) {
            xmlChar *value = xmlNodeListGetString(node->doc, attribute->children, 1);
            length = attribute->name == NULL ? 0 :
                     strlen((const char *)attribute->name);
            if (attribute->ns != NULL && attribute->ns->prefix != NULL)
                length = add_size(length,
                    add_size(strlen((const char *)attribute->ns->prefix), 1,
                             "attribute name length overflow"),
                    "attribute name length overflow");
            *checksum ^= (uint64_t)length;
            if (value != NULL) {
                *checksum ^= (uint64_t)strlen((const char *)value);
                xmlFree(value);
            } else if (attribute->children != NULL) {
                fail(70, "matched benchmark attribute value allocation failure");
            }
        }
        if (node->children != NULL) {
            node = node->children;
            continue;
        }
        while (node != root && node->next == NULL)
            node = node->parent;
        if (node == root)
            break;
        node = node->next;
    }
    return count;
}

static size_t document_nodes(xmlDoc *doc, uint64_t *checksum) {
    xmlNode *node;
    size_t count = 1;
    *checksum ^= (uint64_t)XML_DOCUMENT_NODE + UINT64_C(0x9e3779b97f4a7c15);
    *checksum = (*checksum << 13) | (*checksum >> 51);
    for (node = doc->children; node != NULL; node = node->next)
        count = add_size(count, visit_tree(node, checksum),
                         "document node count overflow");
    return count;
}

static struct Target *prepare_targets(xmlNode *parent,
                                      const struct Request *request,
                                      size_t *planned) {
    xmlNode **children;
    size_t *remaining;
    struct Target *targets;
    xmlNode *node;
    size_t child_count = 0, index, left;
    for (node = parent->children; node != NULL; node = node->next) {
        if (node->type == XML_ELEMENT_NODE)
            ++child_count;
    }
    if (child_count != request->width)
        fail(65, "matched benchmark removal width mismatch");
    *planned = child_count < request->operations ? child_count : request->operations;
    children = allocate(child_count, sizeof(*children));
    remaining = allocate(child_count, sizeof(*remaining));
    targets = allocate(*planned, sizeof(*targets));
    index = 0;
    for (node = parent->children; node != NULL; node = node->next) {
        if (node->type == XML_ELEMENT_NODE) {
            children[index] = node;
            remaining[index] = index;
            ++index;
        }
    }
    left = child_count;
    for (index = 0; index < *planned; ++index) {
        size_t position;
        uint64_t ignored = 0;
        if (request->kind == REMOVE_FIRST)
            position = 0;
        else if (request->kind == REMOVE_LAST)
            position = left - 1;
        else
            position = left / 2;
        targets[index].node = children[remaining[position]];
        targets[index].subtree_nodes = visit_tree(targets[index].node, &ignored);
        if (position + 1 < left)
            memmove(&remaining[position], &remaining[position + 1],
                    (left - position - 1) * sizeof(*remaining));
        --left;
    }
    free(children);
    free(remaining);
    return targets;
}

static int sink_write(void *context, const char *buffer, int length) {
    struct Sink *sink = context;
    (void)buffer;
    if (length < 0 || (size_t)length > SIZE_MAX - sink->bytes) {
        sink->failed = 1;
        return -1;
    }
    if (length > 0) {
        sink->bytes += (size_t)length;
        ++sink->writes;
    }
    return length;
}

static int sink_close(void *context) {
    struct Sink *sink = context;
    return sink->failed ? -1 : 0;
}

static void save_document(xmlDoc *doc, xmlSaveCtxt *save) {
    xmlNode *child;
    for (child = doc->children; child != NULL; child = child->next) {
        if (xmlSaveTree(save, child) < 0)
            fail(70, "matched benchmark serialization failure");
    }
}

static struct Sink count_serialization(xmlDoc *doc) {
    struct Sink sink;
    xmlSaveCtxt *save;
    memset(&sink, 0, sizeof(sink));
    save = xmlSaveToIO(sink_write, sink_close, &sink, "UTF-8",
                       XML_SAVE_NO_DECL | XML_SAVE_AS_XML | XML_SAVE_EMPTY);
    if (save == NULL)
        fail(70, "matched benchmark save context allocation failure");
    save_document(doc, save);
    if (xmlSaveClose(save) < 0 || sink.failed)
        fail(70, "matched benchmark serialization close failure");
    return sink;
}

static xmlBuffer *buffer_serialization(xmlDoc *doc) {
    xmlBuffer *buffer = xmlBufferCreate();
    xmlSaveCtxt *save;
    if (buffer == NULL)
        fail(70, "matched benchmark validation buffer allocation failure");
    save = xmlSaveToBuffer(buffer, "UTF-8",
                           XML_SAVE_NO_DECL | XML_SAVE_AS_XML | XML_SAVE_EMPTY);
    if (save == NULL)
        fail(70, "matched benchmark validation save allocation failure");
    save_document(doc, save);
    if (xmlSaveClose(save) < 0)
        fail(70, "matched benchmark validation close failure");
    return buffer;
}

static uint64_t fnv(const unsigned char *bytes, size_t length) {
    uint64_t value = UINT64_C(0xcbf29ce484222325);
    size_t index;
    for (index = 0; index < length; ++index)
        value = (value ^ bytes[index]) * UINT64_C(0x100000001b3);
    return value;
}

static xmlNode *construct(xmlDoc *doc, enum CaseKind kind,
                          const unsigned char *payload, size_t length) {
    switch (kind) {
        case APPEND_TEXT:
        case CHURN:
            return xmlNewDocText(doc, (const xmlChar *)payload);
        case APPEND_CDATA:
            return xmlNewCDataBlock(doc, (const xmlChar *)payload, (int)length);
        case APPEND_COMMENT:
            return xmlNewDocComment(doc, (const xmlChar *)payload);
        case APPEND_PI:
            return xmlNewDocPI(doc, BAD_CAST "bench", (const xmlChar *)payload);
        default:
            return NULL;
    }
}

int main(int argc, char **argv) {
    struct Request request;
    uint64_t setup_ns = 0, edit_ns = 0, traversal_ns = 0, serialization_ns = 0;
    uint64_t validation_ns = 0, drop_ns = 0, traversal_visits = 0;
    uint64_t counted_bytes = 0, validation_bytes = 0, sink_writes = 0;
    uint64_t requested_calls = 0, completed_calls = 0, created_nodes = 0;
    uint64_t merged_nodes = 0, removed_nodes = 0, completed_units = 0;
    uint64_t expected_hash = 0, traversal_checksum = 0;
    size_t initial_live = 0, final_live = 0, planned_units = 0;
    size_t iteration;
    if (argc == 2 && strcmp(argv[1], "--version") == 0) {
        printf(MAGIC " compiled=%d runtime=%s tree=%d output=%d\n",
               LIBXML_VERSION, xmlParserVersion,
               xmlHasFeature(XML_WITH_TREE), xmlHasFeature(XML_WITH_OUTPUT));
        return 0;
    }
    if (argc != 2)
        fail(64, "usage: libxml2_structure_bench REQUEST");
    request = read_request(argv[1]);
    xmlInitParser();
    for (iteration = 0; iteration < request.iterations; ++iteration) {
        xmlDoc *doc;
        xmlNode *parent;
        struct Target *targets = NULL;
        size_t row_planned = request.operations;
        size_t row_initial, row_final, unit;
        uint64_t started, ignored = 0;
        started = now_ns();
        doc = xmlReadMemory((const char *)request.input, (int)request.input_length,
                            NULL, NULL, XML_PARSE_NONET | XML_PARSE_NO_XXE |
                            XML_PARSE_NOERROR | XML_PARSE_NOWARNING);
        if (doc == NULL)
            fail(65, "matched benchmark input rejected");
        if (doc->intSubset != NULL || doc->extSubset != NULL)
            fail(65, "DTDs are outside the matched benchmark projection");
        parent = selected_parent(doc, &request);
        if (is_removal(request.kind))
            targets = prepare_targets(parent, &request, &row_planned);
        setup_ns = add_u64(setup_ns, now_ns() - started, "setup time overflow");
        /* Exact initial-count validation is outside every phase clock. */
        row_initial = document_nodes(doc, &ignored);
        if (iteration == 0) {
            initial_live = row_initial;
            planned_units = row_planned;
        } else if (initial_live != row_initial || planned_units != row_planned) {
            fail(70, "matched benchmark setup changed across iterations");
        }

        started = now_ns();
        if (is_removal(request.kind)) {
            for (unit = 0; unit < row_planned; ++unit) {
                xmlNode *target = targets[unit].node;
                ++requested_calls;
                xmlUnlinkNode(target);
                xmlFreeNode(target);
                ++completed_calls;
                removed_nodes = add_u64(removed_nodes, targets[unit].subtree_nodes,
                                        "removed node count overflow");
                ++completed_units;
            }
        } else {
            for (unit = 0; unit < row_planned; ++unit) {
                xmlNode *fresh, *previous, *result;
                int merge;
                ++requested_calls;
                previous = parent->last;
                merge = request.kind == APPEND_TEXT && previous != NULL &&
                        previous->type == XML_TEXT_NODE;
                fresh = construct(doc, request.kind, request.payload,
                                  request.payload_length);
                if (fresh == NULL)
                    fail(70, "matched benchmark constructor failure");
                result = xmlAddChild(parent, fresh);
                /* xmlAddChild can free fresh. Never inspect fresh again. */
                if (result == NULL)
                    fail(70, "matched benchmark xmlAddChild failure");
                if ((merge && result != previous) || (!merge && result != parent->last))
                    fail(70, "matched benchmark unexpected add-child survivor");
                ++completed_calls;
                ++created_nodes;
                if (merge)
                    ++merged_nodes;
                if (request.kind == CHURN) {
                    ++requested_calls;
                    xmlUnlinkNode(result);
                    xmlFreeNode(result);
                    /* result is dead and is never evaluated again. */
                    ++completed_calls;
                    ++removed_nodes;
                }
                ++completed_units;
            }
        }
        edit_ns = add_u64(edit_ns, now_ns() - started, "edit time overflow");
        free(targets);

        started = now_ns();
        row_final = 0;
        for (unit = 0; unit < request.traversal_passes; ++unit) {
            uint64_t pass_checksum = UINT64_C(0xcbf29ce484222325);
            /* Keep every observational pass inside its measured loop at -O3. */
            atomic_signal_fence(memory_order_seq_cst);
            size_t visits = document_nodes(doc, &pass_checksum);
            atomic_signal_fence(memory_order_seq_cst);
            if (unit == 0)
                row_final = visits;
            else if (row_final != visits)
                fail(70, "matched benchmark live traversal changed");
            traversal_visits = add_u64(traversal_visits, visits,
                                       "traversal visit count overflow");
            traversal_checksum =
                (traversal_checksum ^ pass_checksum) * UINT64_C(1099511628211);
        }
        traversal_ns = add_u64(traversal_ns, now_ns() - started,
                               "traversal time overflow");
        if (iteration == 0)
            final_live = row_final;
        else if (final_live != row_final)
            fail(70, "matched benchmark final live count changed");

        started = now_ns();
        for (unit = 0; unit < request.serialization_passes; ++unit) {
            struct Sink sink = count_serialization(doc);
            counted_bytes = add_u64(counted_bytes, sink.bytes,
                                    "serialized byte count overflow");
            sink_writes = add_u64(sink_writes, sink.writes,
                                  "serialized write count overflow");
        }
        serialization_ns = add_u64(serialization_ns, now_ns() - started,
                                   "serialization time overflow");

        started = now_ns();
        {
            xmlBuffer *buffer = buffer_serialization(doc);
            size_t length = xmlBufferLength(buffer);
            const unsigned char *bytes = xmlBufferContent(buffer);
            uint64_t hash;
            if (length != request.expected_length ||
                memcmp(bytes, request.expected, length) != 0)
                fail(65, "matched benchmark final UTF-8 bytes differ");
            hash = fnv(bytes, length);
            if (iteration == 0)
                expected_hash = hash;
            else if (expected_hash != hash)
                fail(70, "matched benchmark output hash changed");
            validation_bytes = add_u64(validation_bytes, length,
                                       "validation byte count overflow");
            xmlBufferFree(buffer);
        }
        validation_ns = add_u64(validation_ns, now_ns() - started,
                                "validation time overflow");

        started = now_ns();
        xmlFreeDoc(doc);
        drop_ns = add_u64(drop_ns, now_ns() - started, "drop time overflow");
    }
    printf("benchmark=owned_structure schema=2 implementation=libxml2 libxml_version=%s "
           "case=%s encoding=%s reference_model=matched comparison_status=matched "
           "width=%zu depth=%zu payload_bytes=%zu input_bytes=%zu iterations=%zu "
           "configured_units_per_iteration=%zu planned_units_per_iteration=%zu "
           "completed_units_total=%" PRIu64 " requested_edit_calls_total=%" PRIu64 " "
           "completed_edit_calls_total=%" PRIu64 " created_nodes_total=%" PRIu64 " "
           "merged_nodes_total=%" PRIu64 " removed_nodes_total=%" PRIu64 " "
           "initial_live_nodes_per_iteration=%zu final_live_nodes_per_iteration=%zu "
           "traversal_passes=%zu traversal_visits_total=%" PRIu64 " "
           "serialization_passes=%zu counted_output_bytes_total=%" PRIu64 " "
           "validation_output_bytes_total=%" PRIu64 " sink_writes_total=%" PRIu64 " "
           "status=complete setup_ns=%" PRIu64 " edit_ns=%" PRIu64 " "
           "traversal_ns=%" PRIu64 " serialization_ns=%" PRIu64 " "
           "validation_ns=%" PRIu64 " drop_ns=%" PRIu64 " output_hash=%016" PRIx64 " "
           "checksum=%016" PRIx64 " edit_calls_per_s=%.2f\n",
           xmlParserVersion, request.case_label, request.encoding, request.width,
           request.depth, request.payload_bytes, request.input_length,
           request.iterations, request.operations, planned_units, completed_units,
           requested_calls, completed_calls, created_nodes, merged_nodes,
           removed_nodes, initial_live, final_live, request.traversal_passes,
           traversal_visits, request.serialization_passes, counted_bytes,
           validation_bytes, sink_writes, setup_ns, edit_ns, traversal_ns,
           serialization_ns, validation_ns, drop_ns, expected_hash,
           traversal_checksum,
           edit_ns == 0 ? 0.0 : (double)completed_calls * 1000000000.0 /
                                      (double)edit_ns);
    free((void *)request.case_label);
    free(request.input);
    free(request.payload);
    free(request.expected);
    xmlCleanupParser();
    return fflush(stdout) == 0 ? 0 : 74;
}
