/* Structured-error oracle; Cargo never compiles or links this (see
 * docs/bindings.md#error-mapping).
 *
 *   libxml2_errors [OPTIONS] FILE...
 *
 * Reads each FILE with xmlReadFile and xmlParserOption OPTIONS (default
 * xmllint's XML_PARSE_COMPACT | XML_PARSE_BIG_LINES) and prints one line per
 * structured error raised while parsing:
 *
 *   FILE \t domain \t code \t level \t line \t column \t str1 \t str2 \t int1 \t message
 *
 * with tabs and newlines in strings written as spaces and a missing string
 * as "-". */
#include <libxml/parser.h>
#include <libxml/xmlerror.h>
#include <stdio.h>
#include <stdlib.h>

static const char *file;

static void field(const char *s) {
    putchar('\t');
    if (s == NULL) {
        putchar('-');
        return;
    }
    for (; *s; s++)
        putchar(*s == '\t' || *s == '\n' || *s == '\r' ? ' ' : *s);
}

static void record(void *ctx, const xmlError *e) {
    (void)ctx;
    printf("%s\t%d\t%d\t%d\t%d\t%d", file, e->domain, e->code, e->level, e->line, e->int2);
    field(e->str1);
    field(e->str2);
    printf("\t%d", e->int1);
    field(e->message);
    putchar('\n');
}

int main(int argc, char **argv) {
    int first = 1;
    int options = XML_PARSE_COMPACT | XML_PARSE_BIG_LINES;
    if (argc > 1 && argv[1][0] >= '0' && argv[1][0] <= '9') {
        options = atoi(argv[1]);
        first = 2;
    }
    xmlSetStructuredErrorFunc(NULL, record);
    for (int i = first; i < argc; i++) {
        file = argv[i];
        xmlFreeDoc(xmlReadFile(file, NULL, options));
    }
    return 0;
}
