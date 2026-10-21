/* In-context parsing oracle; Cargo never compiles or links this file.
 *
 * Reads one request per line: hex(document) TAB node TAB options TAB
 * hex(chunk) TAB mode TAB insert. The document is read with xmlReadMemory
 * and default options; node indexes its preorder node list (see `walk`).
 * Mode I calls xmlParseInNodeContext(node, chunk, options), mode B
 * xmlParseBalancedChunkMemory(document, chunk) (node and options unused);
 * mode W reads the chunk as a whole document with options, a control
 * whose "list" is its root element's children (CODE is the last error's,
 * or 0). Prints "ERR" when the document does not parse, else
 *   S CODE FIRST COUNT   the return code, the first structured error's code
 *                        ("-" for none) and the number of errors
 * then the returned list in preorder, not entering entity references:
 *   E DEPTH hex(prefix) hex(local) hex(uri)    element, then its namespace
 *   D DEPTH hex(prefix) hex(uri)               declarations and attributes
 *   A DEPTH hex(prefix) hex(local) hex(uri) hex(value)
 *   T|C|M DEPTH hex(content)                   text, CDATA, comment
 *   P DEPTH hex(target) hex(content)           processing instruction
 *   R DEPTH hex(name)                          entity reference
 * and, when insert is 1, "DOC" hex(xmlDocDumpMemory) after xmlAddChildList
 * of the list to the element libxml2 parsed in; then "END". */
#include <libxml/parser.h>
#include <libxml/tree.h>
#include <libxml/xmlerror.h>
#include <libxml/xmlmemory.h>
#include <libxml/xmlversion.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static int nibble(int c) {
    return c <= '9' ? c - '0' : (c | 32) - 'a' + 10;
}

static char *unhex(const char *h, size_t *length) {
    size_t n = strlen(h) / 2;
    char *out = malloc(n + 1);
    for (size_t i = 0; i < n; i++)
        out[i] = (char)(nibble(h[2 * i]) << 4 | nibble(h[2 * i + 1]));
    out[n] = 0;
    if (length) *length = n;
    return out;
}

static void hex(const xmlChar *s) {
    fputc(' ', stdout);
    if (s == NULL) { fputs("-", stdout); return; }
    if (*s == 0) { fputs("0", stdout); return; }
    for (; *s; s++) printf("%02x", *s);
}

typedef struct { xmlNodePtr *nodes; size_t count, capacity; } List;

static void keep(List *list, xmlNodePtr node) {
    if (list->count == list->capacity) {
        list->capacity = list->capacity ? 2 * list->capacity : 64;
        list->nodes = realloc(list->nodes, list->capacity * sizeof *list->nodes);
    }
    list->nodes[list->count++] = node;
}

/* Preorder as yaxml stores it: entity references contain their entity's
 * content, the DTD node has no children. */
static void walk(List *list, xmlNodePtr node) {
    for (; node != NULL; node = node->next) {
        keep(list, node);
        if (node->type == XML_ENTITY_REF_NODE) {
            xmlEntityPtr entity = (xmlEntityPtr)node->children;
            if (entity != NULL) walk(list, entity->children);
        } else if (node->type == XML_ELEMENT_NODE) {
            walk(list, node->children);
        }
    }
}

static int errors, first;

static void record(void *ctx, const xmlError *e) {
    (void)ctx;
    if (errors++ == 0) first = e->code;
}

static void dump(xmlDocPtr doc, xmlNodePtr node, int depth) {
    for (; node != NULL; node = node->next) {
        switch (node->type) {
        case XML_ELEMENT_NODE:
            printf("E %d", depth);
            hex(node->ns ? node->ns->prefix : NULL);
            hex(node->name);
            hex(node->ns ? node->ns->href : NULL);
            fputc('\n', stdout);
            for (xmlNsPtr ns = node->nsDef; ns != NULL; ns = ns->next) {
                printf("D %d", depth);
                hex(ns->prefix);
                hex(ns->href);
                fputc('\n', stdout);
            }
            for (xmlAttrPtr a = node->properties; a != NULL; a = a->next) {
                xmlChar *value = xmlNodeListGetString(doc, a->children, 1);
                printf("A %d", depth);
                hex(a->ns ? a->ns->prefix : NULL);
                hex(a->name);
                hex(a->ns ? a->ns->href : NULL);
                hex(value ? value : (const xmlChar *)"");
                fputc('\n', stdout);
                xmlFree(value);
            }
            dump(doc, node->children, depth + 1);
            continue;
        case XML_TEXT_NODE: printf("T %d", depth); break;
        case XML_CDATA_SECTION_NODE: printf("C %d", depth); break;
        case XML_COMMENT_NODE: printf("M %d", depth); break;
        case XML_PI_NODE: printf("P %d", depth); hex(node->name); break;
        case XML_ENTITY_REF_NODE:
            printf("R %d", depth);
            hex(node->name);
            fputc('\n', stdout);
            continue;
        default: printf("? %d %d\n", depth, node->type); continue;
        }
        hex(node->content);
        fputc('\n', stdout);
    }
}

int main(void) {
    char *line = NULL;
    size_t cap = 0;
    List nodes = {0};
    if (LIBXML_VERSION != 21504) { fputs("wrong libxml2\n", stderr); return 2; }
    xmlSetStructuredErrorFunc(NULL, record);
    while (getline(&line, &cap, stdin) > 0) {
        line[strcspn(line, "\n")] = 0;
        char *fields[6] = {0};
        char *rest = line;
        for (int i = 0; i < 6; i++) fields[i] = strsep(&rest, "\t");
        size_t size;
        char *xml = unhex(fields[0], NULL);
        char *chunk = unhex(fields[3], &size);
        errors = 0;
        if (fields[4][0] == 'W') {
            xmlDocPtr whole = xmlReadMemory(chunk, (int)size, NULL, NULL, atoi(fields[2]));
            xmlNodePtr root = xmlDocGetRootElement(whole);
            printf("S %d ", whole ? 0 : xmlGetLastError()->code);
            if (errors) printf("%d", first); else fputc('-', stdout);
            printf(" %d\n", errors);
            if (root != NULL) dump(whole, root->children, 0);
            xmlFreeDoc(whole);
            puts("END");
            fflush(stdout);
            free(xml);
            free(chunk);
            continue;
        }
        xmlDocPtr doc = xmlReadMemory(xml, (int)strlen(xml), NULL, NULL, XML_PARSE_NOERROR);
        if (doc == NULL) {
            puts("ERR");
        } else {
            nodes.count = 0;
            keep(&nodes, (xmlNodePtr)doc);
            walk(&nodes, doc->children);
            long index = atol(fields[1]);
            xmlNodePtr node = index < (long)nodes.count ? nodes.nodes[index] : NULL;
            xmlNodePtr list = NULL;
            int code;
            errors = 0;
            if (fields[4][0] == 'B') {
                code = xmlParseBalancedChunkMemory(doc, NULL, NULL, 0, (xmlChar *)chunk, &list);
            } else {
                code = xmlParseInNodeContext(node, chunk, (int)size, atoi(fields[2]), &list);
            }
            printf("S %d ", code);
            if (errors) printf("%d", first); else fputc('-', stdout);
            printf(" %d\n", errors);
            dump(doc, list, 0);
            /* The node libxml2 parsed in. */
            while (node != NULL && node->type != XML_ELEMENT_NODE &&
                   node->type != XML_DOCUMENT_NODE)
                node = node->parent;
            if (atoi(fields[5]) && node != NULL && node->type == XML_ELEMENT_NODE) {
                xmlAddChildList(node, list);
                list = NULL;
                xmlChar *out = NULL;
                int length = 0;
                xmlDocDumpMemory(doc, &out, &length);
                fputs("DOC", stdout);
                hex(out);
                fputc('\n', stdout);
                xmlFree(out);
            }
            xmlFreeNodeList(list);
            xmlFreeDoc(doc);
        }
        puts("END");
        fflush(stdout);
        free(xml);
        free(chunk);
    }
    free(line);
    free(nodes.nodes);
    return 0;
}
