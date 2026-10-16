/* External conformance oracle only. Cargo never compiles or links this file. */
#include <libxml/parser.h>
#include <libxml/tree.h>
#include <libxml/xmlsave.h>
#include <libxml/xmlversion.h>

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define MAGIC "YAXML-STRUCTURE-1"
#define ORACLE_INPUT_BYTES 65536U
#define MAX_VALUE 4096U
#define MAX_NODES 256U
#define MAX_ATTRIBUTES 4096U
#define MAX_OUTPUT (8U * 1024U * 1024U)

static size_t written;

static void fail(int code, const char *message) {
    fprintf(stderr, "%s\n", message);
    exit(code);
}

static void output(const void *value, size_t length) {
    if (length > MAX_OUTPUT - written)
        fail(65, "structural oracle output ceiling");
    if (fwrite(value, 1, length, stdout) != length)
        fail(74, "structural oracle stdout failure");
    written += length;
}

static void number(size_t value) {
    char buffer[32];
    int length = snprintf(buffer, sizeof(buffer), "%zu\n", value);
    if (length < 0 || (size_t)length >= sizeof(buffer))
        fail(70, "structural oracle number formatting failure");
    output(buffer, (size_t)length);
}

static void frame(const void *value, size_t length) {
    char buffer[32];
    int size = snprintf(buffer, sizeof(buffer), "%zu:", length);
    if (size < 0 || (size_t)size >= sizeof(buffer))
        fail(70, "structural oracle frame formatting failure");
    output(buffer, (size_t)size);
    if (length != 0)
        output(value, length);
    output("\n", 1);
}

static void string_frame(const xmlChar *value) {
    frame(value, value == NULL ? 0 : strlen((const char *)value));
}

static size_t read_number(int delimiter, size_t maximum) {
    size_t value = 0, digits = 0;
    int byte;
    while ((byte = getchar()) != delimiter) {
        if (byte < '0' || byte > '9' || ++digits > 20)
            fail(65, "invalid structural oracle number");
        if (value > maximum / 10 ||
            (value == maximum / 10 && (size_t)(byte - '0') > maximum % 10))
            fail(65, "structural oracle input ceiling");
        value = value * 10 + (size_t)(byte - '0');
    }
    if (digits == 0)
        fail(65, "empty structural oracle number");
    return value;
}

static xmlChar *read_frame(size_t maximum, int allow_nul, size_t *length) {
    xmlChar *value;
    *length = read_number(':', maximum);
    value = malloc(*length + 1);
    if (value == NULL)
        fail(70, "structural oracle allocation failure");
    if (fread(value, 1, *length, stdin) != *length || getchar() != '\n')
        fail(65, "truncated structural oracle frame");
    if (!allow_nul && memchr(value, 0, *length) != NULL)
        fail(65, "NUL is outside the C-string projection");
    value[*length] = 0;
    return value;
}

/* Iterative preorder, including the document node. Pointers are live only. */
static size_t collect(xmlDoc *doc, xmlNode **nodes) {
    xmlNode *root = (xmlNode *)doc, *node = root;
    size_t count = 0;
    while (node != NULL) {
        if (count == MAX_NODES)
            fail(65, "structural oracle node ceiling");
        switch (node->type) {
            case XML_DOCUMENT_NODE: case XML_ELEMENT_NODE: case XML_TEXT_NODE:
            case XML_CDATA_SECTION_NODE: case XML_COMMENT_NODE: case XML_PI_NODE:
                break;
            default: fail(65, "unsupported structural oracle node kind");
        }
        nodes[count++] = node;
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

static void qualified(const xmlChar *local, const xmlNs *ns) {
    const xmlChar *prefix = ns == NULL ? NULL : ns->prefix;
    size_t left = prefix == NULL ? 0 : strlen((const char *)prefix);
    size_t right = local == NULL ? 0 : strlen((const char *)local);
    char buffer[32];
    size_t length = left + (left != 0) + right;
    int size = snprintf(buffer, sizeof(buffer), "%zu:", length);
    if (size < 0 || (size_t)size >= sizeof(buffer))
        fail(70, "structural oracle name formatting failure");
    output(buffer, (size_t)size);
    if (left != 0) { output(prefix, left); output(":", 1); }
    if (right != 0) output(local, right);
    output("\n", 1);
}

static void snapshot(xmlDoc *doc) {
    xmlNode *nodes[MAX_NODES];
    size_t count = collect(doc, nodes), attributes = 0, index;
    number(count);
    for (index = 0; index < count; ++index) {
        xmlNode *node = nodes[index];
        size_t parent = 0, attr_count = 0;
        char kind;
        xmlAttr *attr;
        switch (node->type) {
            case XML_DOCUMENT_NODE: kind = 'D'; break;
            case XML_ELEMENT_NODE: kind = 'E'; break;
            case XML_TEXT_NODE: kind = 'T'; break;
            case XML_CDATA_SECTION_NODE: kind = 'C'; break;
            case XML_COMMENT_NODE: kind = 'M'; break;
            case XML_PI_NODE: kind = 'P'; break;
            default: fail(65, "unsupported structural oracle node kind");
        }
        output(&kind, 1); output("\n", 1);
        if (index != 0) {
            while (parent < index && nodes[parent] != node->parent) ++parent;
            if (parent == index) fail(70, "non-preorder structural oracle parent");
            ++parent;
        }
        number(parent);
        if (kind == 'E') qualified(node->name, node->ns);
        else if (kind == 'P') string_frame(node->name);
        else frame(NULL, 0);
        string_frame(kind == 'E' && node->ns != NULL ? node->ns->href : NULL);
        string_frame(kind == 'T' || kind == 'C' || kind == 'M' || kind == 'P' ? node->content : NULL);
        if (kind == 'E') {
            for (attr = node->properties; attr != NULL; attr = attr->next) {
                if (++attributes > MAX_ATTRIBUTES) fail(65, "structural oracle attribute ceiling");
                ++attr_count;
            }
        }
        number(attr_count);
        if (kind == 'E') {
            for (attr = node->properties; attr != NULL; attr = attr->next) {
                xmlChar *value = xmlNodeListGetString(doc, attr->children, 1);
                if (value == NULL && attr->children != NULL)
                    fail(70, "structural oracle attribute allocation failure");
                qualified(attr->name, attr->ns);
                string_frame(attr->ns == NULL ? NULL : attr->ns->href);
                string_frame(value);
                xmlFree(value);
            }
        }
    }
    {
        xmlBuffer *buffer = xmlBufferCreate();
        xmlSaveCtxt *save;
        xmlNode *child;
        if (buffer == NULL) fail(70, "structural oracle output allocation failure");
        save = xmlSaveToBuffer(buffer, "UTF-8", XML_SAVE_NO_DECL | XML_SAVE_AS_XML | XML_SAVE_EMPTY);
        if (save == NULL) fail(70, "structural oracle save allocation failure");
        /* Save each top-level node, avoiding document declaration/separators. */
        for (child = doc->children; child != NULL; child = child->next) {
            if (xmlSaveTree(save, child) < 0) fail(70, "structural oracle serialization failure");
        }
        if (xmlSaveClose(save) < 0) fail(70, "structural oracle flush failure");
        frame(xmlBufferContent(buffer), xmlBufferLength(buffer));
        xmlBufferFree(buffer);
    }
}

int main(int argc, char **argv) {
    char header[64];
    size_t action, target, input_length, target_length, value_length, count;
    xmlChar *input, *pi_target, *value;
    xmlDoc *doc;
    xmlNode *nodes[MAX_NODES], *operand, *removed = NULL;
    if (argc == 2 && strcmp(argv[1], "--version") == 0) {
        printf(MAGIC " compiled=%d runtime=%s tree=%d output=%d\n",
            LIBXML_VERSION, xmlParserVersion,
            xmlHasFeature(XML_WITH_TREE), xmlHasFeature(XML_WITH_OUTPUT));
        return 0;
    }
    if (argc != 1) fail(65, "unexpected structural oracle arguments");
    if (fgets(header, sizeof(header), stdin) == NULL || strcmp(header, MAGIC "\n") != 0)
        fail(65, "invalid structural oracle protocol");
    action = read_number('\n', 5);
    target = read_number('\n', MAX_NODES - 1);
    input = read_frame(ORACLE_INPUT_BYTES, 1, &input_length);
    pi_target = read_frame(MAX_VALUE, 0, &target_length);
    value = read_frame(MAX_VALUE, 0, &value_length);
    if (getchar() != EOF || action == 0 || (action != 4 && target_length != 0) ||
        (action == 5 && value_length != 0))
        fail(65, "invalid structural oracle request fields");
    doc = xmlReadMemory((const char *)input, (int)input_length, NULL, NULL,
        XML_PARSE_NONET | XML_PARSE_NO_XXE | XML_PARSE_NOERROR | XML_PARSE_NOWARNING);
    free(input);
    if (doc == NULL) fail(65, "structural oracle input rejected");
    if (doc->intSubset != NULL || doc->extSubset != NULL)
        fail(65, "DTDs are outside the structural oracle projection");
    count = collect(doc, nodes);
    if (target >= count) fail(65, "structural oracle target is out of range");
    operand = nodes[target];
    output(MAGIC "\n", sizeof(MAGIC));
    /* Initial snapshot precedes all mutation, independently checking parsing. */
    snapshot(doc);
    if (action == 5) {
        if (target == 0) fail(65, "unlinking the document object is unsupported");
        removed = operand;
        xmlUnlinkNode(removed);
        number(3);
    } else {
        xmlNode *fresh = NULL, *previous = operand->last, *result;
        int text_parent = operand->type == XML_TEXT_NODE;
        int merge = action == 1 && previous != NULL && previous->type == XML_TEXT_NODE;
        switch (action) {
            case 1: fresh = xmlNewDocText(doc, value); break;
            case 2: fresh = xmlNewCDataBlock(doc, value, (int)value_length); break;
            case 3: fresh = xmlNewDocComment(doc, value); break;
            case 4: fresh = xmlNewDocPI(doc, pi_target, value); break;
            default: fail(65, "invalid structural oracle action");
        }
        if (fresh == NULL) fail(70, "structural oracle constructor failed (ownership/OOM inconclusive)");
        result = xmlAddChild(operand, fresh);
        /* xmlAddChild may free fresh. Never evaluate or dereference it again. */
        if (result == NULL) fail(70, "structural oracle insertion failed (ownership/OOM inconclusive)");
        if ((text_parent && result != operand) ||
            (!text_parent && result != operand->last) ||
            (!text_parent && merge && result != previous))
            fail(70, "structural oracle returned an unexpected survivor");
        number(text_parent ? 2 : merge ? 1 : 0);
    }
    snapshot(doc);
    /* The entire survivor projection and serialization happen before freeing. */
    if (removed != NULL) xmlFreeNode(removed);
    xmlFreeDoc(doc);
    free(pi_target);
    free(value);
    return fflush(stdout) == 0 ? 0 : 74;
}
