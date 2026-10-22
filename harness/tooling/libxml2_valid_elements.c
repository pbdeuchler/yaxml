/* xmlValidGetValidElements dump for tests/valid_elements.rs:
 * `libxml2_valid_elements FILE` parses FILE (default options) and, for every
 * element in document order (numbered from 0) with children 0..n, prints the
 * names for each insertion point: `prepend`, `append`, `before i` and
 * `replace i`. Regenerate: cd tests/fixtures/valid_elements &&
 *   libxml2_valid_elements NAME.xml > NAME.txt */
#include <stdio.h>
#include <stdlib.h>
#include <libxml/parser.h>
#include <libxml/valid.h>

static void line(const char *what, int index, xmlNode *prev, xmlNode *next) {
    const xmlChar *names[256];
    int n = xmlValidGetValidElements(prev, next, names, 256);
    if (index < 0)
        printf("  %s:", what);
    else
        printf("  %s %d:", what, index);
    if (n < 0)
        printf(" -1");
    for (int i = 0; i < n; i++)
        printf(" %s", (const char *)names[i]);
    printf("\n");
}

static void visit(xmlNode *node, int *ordinal) {
    for (; node != NULL; node = node->next) {
        if (node->type != XML_ELEMENT_NODE)
            continue;
        printf("%d %s\n", (*ordinal)++, (const char *)node->name);
        if (node->children != NULL) {
            line("prepend", -1, NULL, node->children);
            line("append", -1, node->last, NULL);
            int i = 0;
            for (xmlNode *child = node->children; child != NULL; child = child->next, i++) {
                line("before", i, child->prev, child);
                line("replace", i, child->prev, child->next);
            }
        }
        visit(node->children, ordinal);
    }
}

int main(int argc, char **argv) {
    if (argc != 2) {
        fprintf(stderr, "usage: %s FILE\n", argv[0]);
        return 2;
    }
    xmlDoc *doc = xmlReadFile(argv[1], NULL, 0);
    if (doc == NULL)
        return 1;
    int ordinal = 0;
    visit(doc->children, &ordinal);
    xmlFreeDoc(doc);
    return 0;
}
