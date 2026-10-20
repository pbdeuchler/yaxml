/* Fixed tree-API oracle only; Cargo never compiles or links this file.
 *
 * Reads one request per line: hex(xml) TAB hex(url) or "-" TAB operations,
 * where operations are space-separated tokens over the initial preorder
 * node list (see `walk`):
 *   A,i,j  xmlAddChild(n[i], n[j])        B,i,j  xmlAddPrevSibling(n[i], n[j])
 *   N,i,j  xmlAddNextSibling(n[i], n[j])  U,i    xmlUnlinkNode(n[i])
 *   R,i,j  xmlReplaceNode(n[i], n[j])     T,i,h  xmlAddChild(n[i], new text h)
 * The caller sends only operations yaxml accepted. Prints "ERR" when the
 * document does not parse, else "DOC" hex(xmlDocDumpMemory) and one "N" line
 * per node of the final tree: hex(xmlGetNodePath) hex(xmlNodeGetBase)
 * hex(xmlNodeGetLang) xmlNodeGetSpacePreserve hex(xmlNodeGetContent)
 * ("-" for NULL), then "END". */
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

static xmlNodePtr *nodes;
static size_t count, capacity;

static void keep(xmlNodePtr node) {
    if (count == capacity) {
        capacity = capacity ? 2 * capacity : 64;
        nodes = realloc(nodes, capacity * sizeof *nodes);
    }
    nodes[count++] = node;
}

/* Preorder as yaxml stores it: entity references contain their entity's
 * content, the DTD node has no children. */
static void walk(xmlNodePtr node) {
    for (; node != NULL; node = node->next) {
        keep(node);
        if (node->type == XML_ENTITY_REF_NODE) {
            xmlEntityPtr entity = (xmlEntityPtr)node->children;
            if (entity != NULL) walk(entity->children);
        } else if (node->type == XML_ELEMENT_NODE) {
            walk(node->children);
        }
    }
}

static void collect(xmlDocPtr doc) {
    count = 0;
    keep((xmlNodePtr)doc);
    walk(doc->children);
}

static xmlNodePtr at(char **cursor) {
    long index = strtol(*cursor, cursor, 10);
    if (**cursor == ',') (*cursor)++;
    return index >= 0 && (size_t)index < count ? nodes[index] : NULL;
}

static void apply(xmlDocPtr doc, char *op) {
    char kind = op[0];
    char *cursor = op + 2;
    xmlNodePtr first = at(&cursor);
    xmlNodePtr second = (kind == 'U' || kind == 'T') ? NULL : at(&cursor);
    switch (kind) {
    case 'A': xmlAddChild(first, second); break;
    case 'B': xmlAddPrevSibling(first, second); break;
    case 'N': xmlAddNextSibling(first, second); break;
    case 'U': xmlUnlinkNode(first); break;
    case 'R': xmlReplaceNode(first, second); break;
    case 'T': {
        char *text = unhex(cursor);
        xmlNodePtr node = xmlNewDocText(doc, (xmlChar *)text);
        if (xmlAddChild(first, node) != node && node->parent == NULL) {
            /* merged and freed by xmlAddChild */
        }
        free(text);
        break;
    }
    default: fputs("bad op\n", stderr); exit(2);
    }
}

int main(void) {
    char *line = NULL;
    size_t cap = 0;
    if (LIBXML_VERSION != 21504) { fputs("wrong libxml2\n", stderr); return 2; }
    while (getline(&line, &cap, stdin) > 0) {
        line[strcspn(line, "\n")] = 0;
        char *fields[3] = {0};
        char *rest = line;
        for (int i = 0; i < 3; i++) fields[i] = strsep(&rest, "\t");
        char *xml = unhex(fields[0]);
        char *url = (fields[1] && strcmp(fields[1], "-")) ? unhex(fields[1]) : NULL;
        xmlDocPtr doc = xmlReadMemory(xml, (int)strlen(xml), url, NULL, 0);
        if (doc == NULL) {
            puts("ERR");
        } else {
            collect(doc);
            char *ops = fields[2] ? fields[2] : "";
            for (char *op = strtok(ops, " "); op; op = strtok(NULL, " "))
                apply(doc, op);
            xmlChar *dump = NULL;
            int size = 0;
            xmlDocDumpMemory(doc, &dump, &size);
            fputs("DOC", stdout);
            hex(dump);
            fputc('\n', stdout);
            xmlFree(dump);
            collect(doc);
            for (size_t i = 0; i < count; i++) {
                xmlNodePtr node = nodes[i];
                xmlChar *path = xmlGetNodePath(node);
                xmlChar *base = xmlNodeGetBase(doc, node);
                xmlChar *lang = xmlNodeGetLang(node);
                xmlChar *content = xmlNodeGetContent(node);
                fputs("N", stdout);
                hex(path);
                hex(base);
                hex(lang);
                printf(" %d", xmlNodeGetSpacePreserve(node));
                hex(content);
                fputc('\n', stdout);
                xmlFree(path); xmlFree(base); xmlFree(lang); xmlFree(content);
            }
            puts("END");
            xmlFreeDoc(doc);
        }
        fflush(stdout);
        free(xml);
        free(url);
    }
    free(line);
    free(nodes);
    return 0;
}
