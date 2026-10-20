/* Fixed tree-API oracle only; Cargo never compiles or links this file.
 *
 * Reads one request per line: hex(xml) TAB hex(url) or "-" TAB operations
 * TAB hex(source xml) or "-", where operations are space-separated tokens
 * over the initial preorder node lists of the document (n) and the source
 * document (s; see `walk`):
 *   A,i,j  xmlAddChild(n[i], n[j])        B,i,j  xmlAddPrevSibling(n[i], n[j])
 *   N,i,j  xmlAddNextSibling(n[i], n[j])  U,i    xmlUnlinkNode(n[i])
 *   R,i,j  xmlReplaceNode(n[i], n[j])     T,i,h  xmlAddChild(n[i], new text h)
 *   C,i,j,d  xmlAddChild(n[j], xmlDocCopyNode(s[i], doc, d ? 1 : 2))
 *   c,i,j,d  xmlAddChild(n[j], xmlDocCopyNode(n[i], doc, d ? 1 : 2))
 *   D      report xmlCopyDoc(doc, 1) instead of the document
 * The caller sends only operations yaxml accepted. Prints "ERR" when the
 * document does not parse, else "DOC" hex(xmlDocDumpMemory) and one "N" line
 * per node of the final tree: hex(xmlGetNodePath) hex(xmlNodeGetBase)
 * hex(xmlNodeGetLang) xmlNodeGetSpacePreserve hex(xmlNodeGetContent)
 * hex(xmlNodeListGetString(doc, node, 1)) hex(xmlNodeListGetString(doc,
 * node, 0)) ("-" for NULL), then "END". */
#include <libxml/parser.h>
#include <libxml/tree.h>
#include <libxml/entities.h>
#include <libxml/xmlmemory.h>
#include <libxml/xmlversion.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static int nibble(int c) {
    return c <= '9' ? c - '0' : (c | 32) - 'a' + 10;
}

static char *unhex(const char *h) {
    size_t n = strlen(h) / 2;
    char *out = malloc(n + 1);
    for (size_t i = 0; i < n; i++)
        out[i] = (char)(nibble(h[2 * i]) << 4 | nibble(h[2 * i + 1]));
    out[n] = 0;
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

static void collect(List *list, xmlDocPtr doc) {
    list->count = 0;
    if (doc == NULL) return;
    keep(list, (xmlNodePtr)doc);
    walk(list, doc->children);
}

static xmlNodePtr at(List *list, char **cursor) {
    long index = strtol(*cursor, cursor, 10);
    if (**cursor == ',') (*cursor)++;
    return index >= 0 && (size_t)index < list->count ? list->nodes[index] : NULL;
}

static List nodes, sources;

/* Applies one operation; returns 1 for "D". */
static int apply(xmlDocPtr doc, char *op) {
    char kind = op[0];
    char *cursor = op + 2;
    if (kind == 'D') return 1;
    xmlNodePtr first = at(kind == 'C' ? &sources : &nodes, &cursor);
    xmlNodePtr second = (kind == 'U' || kind == 'T') ? NULL : at(&nodes, &cursor);
    switch (kind) {
    case 'A': xmlAddChild(first, second); break;
    case 'B': xmlAddPrevSibling(first, second); break;
    case 'N': xmlAddNextSibling(first, second); break;
    case 'U': xmlUnlinkNode(first); break;
    case 'R': xmlReplaceNode(first, second); break;
    case 'T': {
        char *text = unhex(cursor);
        /* xmlAddChild frees the node when it merges. */
        xmlAddChild(first, xmlNewDocText(doc, (xmlChar *)text));
        free(text);
        break;
    }
    case 'C':
    case 'c':
        xmlAddChild(second, xmlDocCopyNode(first, doc, atoi(cursor) ? 1 : 2));
        break;
    default: fputs("bad op\n", stderr); exit(2);
    }
    return 0;
}

static void report(xmlDocPtr doc) {
    xmlChar *dump = NULL;
    int size = 0;
    xmlDocDumpMemory(doc, &dump, &size);
    fputs("DOC", stdout);
    hex(dump);
    fputc('\n', stdout);
    xmlFree(dump);
    collect(&nodes, doc);
    for (size_t i = 0; i < nodes.count; i++) {
        xmlNodePtr node = nodes.nodes[i];
        xmlChar *values[] = {
            xmlGetNodePath(node), xmlNodeGetBase(doc, node), xmlNodeGetLang(node),
            NULL, xmlNodeGetContent(node), xmlNodeListGetString(doc, node, 1),
            xmlNodeListGetString(doc, node, 0),
        };
        fputs("N", stdout);
        for (size_t v = 0; v < sizeof values / sizeof *values; v++) {
            if (v == 3) printf(" %d", xmlNodeGetSpacePreserve(node));
            else hex(values[v]);
            xmlFree(values[v]);
        }
        fputc('\n', stdout);
    }
    puts("END");
}

int main(void) {
    char *line = NULL;
    size_t cap = 0;
    if (LIBXML_VERSION != 21504) { fputs("wrong libxml2\n", stderr); return 2; }
    while (getline(&line, &cap, stdin) > 0) {
        line[strcspn(line, "\n")] = 0;
        char *fields[4] = {0};
        char *rest = line;
        for (int i = 0; i < 4; i++) fields[i] = strsep(&rest, "\t");
        char *xml = unhex(fields[0]);
        char *url = (fields[1] && strcmp(fields[1], "-")) ? unhex(fields[1]) : NULL;
        char *source_xml = (fields[3] && strcmp(fields[3], "-")) ? unhex(fields[3]) : NULL;
        xmlDocPtr doc = xmlReadMemory(xml, (int)strlen(xml), url, NULL, 0);
        xmlDocPtr source = source_xml
            ? xmlReadMemory(source_xml, (int)strlen(source_xml), NULL, NULL, 0) : NULL;
        if (doc == NULL) {
            puts("ERR");
        } else {
            int copy = 0;
            collect(&nodes, doc);
            collect(&sources, source);
            char *ops = fields[2] ? fields[2] : "";
            for (char *op = strtok(ops, " "); op; op = strtok(NULL, " "))
                copy |= apply(doc, op);
            if (copy) {
                xmlDocPtr clone = xmlCopyDoc(doc, 1);
                report(clone);
                xmlFreeDoc(clone);
            } else {
                report(doc);
            }
            xmlFreeDoc(doc);
        }
        xmlFreeDoc(source);
        fflush(stdout);
        free(xml);
        free(url);
        free(source_xml);
    }
    free(line);
    free(nodes.nodes);
    free(sources.nodes);
    return 0;
}
