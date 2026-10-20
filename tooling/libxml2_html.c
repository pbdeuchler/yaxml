/* HTML parser oracle; Cargo never compiles or links this (docs/html.md).
 *
 *   libxml2_html MODE OPTIONS FILE...
 *
 * Parses each FILE with htmlParserOption OPTIONS (a decimal number) and
 * prints one record per line, strings escaped as \n, \r, \t, \\ and \xHH for
 * bytes below 0x20. Every FILE starts with "file\tNAME".
 *
 *   sax       custom SAX callbacks, no tree (htmlNewSAXParserCtxt):
 *             doctype NAME PUBLIC SYSTEM | start NAME [ATTR=VALUE|ATTR]... |
 *             end NAME | text S | ws S | cdata S | comment S
 *   lines     htmlCtxtReadMemory, then the tree in preorder:
 *             node DEPTH TYPE NAME xmlGetLineNo
 *   fragment  context document $YAXML_HTML_CONTEXT (default
 *             "<html><body></body></html>", htmlReadMemory), then
 *             xmlParseInNodeContext(its root's last child, FILE, OPTIONS):
 *             result CODE, then the node list as xmlDebugDumpNodeList prints it
 *
 * Every mode also prints each structured error as
 *             error DOMAIN CODE LEVEL LINE COLUMN MESSAGE
 * A missing string is "-". */
#include <libxml/HTMLparser.h>
#include <libxml/debugXML.h>
#include <libxml/parser.h>
#include <libxml/xmlerror.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static void escaped(const xmlChar *s, int len) {
    putchar('\t');
    if (s == NULL) {
        putchar('-');
        return;
    }
    for (int i = 0; len < 0 ? s[i] != 0 : i < len; i++) {
        unsigned char c = s[i];
        if (c == '\n')
            fputs("\\n", stdout);
        else if (c == '\r')
            fputs("\\r", stdout);
        else if (c == '\t')
            fputs("\\t", stdout);
        else if (c == '\\')
            fputs("\\\\", stdout);
        else if (c < 0x20)
            printf("\\x%02X", c);
        else
            putchar(c);
    }
}

static void record(void *ctx, const xmlError *e) {
    (void)ctx;
    printf("error\t%d\t%d\t%d\t%d\t%d", e->domain, e->code, e->level, e->line, e->int2);
    escaped((const xmlChar *)e->message, -1);
    putchar('\n');
}

static void doctype(void *ctx, const xmlChar *name, const xmlChar *public_id,
                    const xmlChar *system_id) {
    (void)ctx;
    fputs("doctype", stdout);
    escaped(name, -1);
    escaped(public_id, -1);
    escaped(system_id, -1);
    putchar('\n');
}

static void start(void *ctx, const xmlChar *name, const xmlChar **atts) {
    (void)ctx;
    fputs("start", stdout);
    escaped(name, -1);
    for (int i = 0; atts != NULL && atts[i] != NULL; i += 2) {
        escaped(atts[i], -1);
        if (atts[i + 1] != NULL) {
            putchar('=');
            escaped(atts[i + 1], -1);
        }
    }
    putchar('\n');
}

static void end(void *ctx, const xmlChar *name) {
    (void)ctx;
    fputs("end", stdout);
    escaped(name, -1);
    putchar('\n');
}

static void text(void *ctx, const xmlChar *s, int len) {
    (void)ctx;
    fputs("text", stdout);
    escaped(s, len);
    putchar('\n');
}

static void ws(void *ctx, const xmlChar *s, int len) {
    (void)ctx;
    fputs("ws", stdout);
    escaped(s, len);
    putchar('\n');
}

static void cdata(void *ctx, const xmlChar *s, int len) {
    (void)ctx;
    fputs("cdata", stdout);
    escaped(s, len);
    putchar('\n');
}

static void comment(void *ctx, const xmlChar *s) {
    (void)ctx;
    fputs("comment", stdout);
    escaped(s, -1);
    putchar('\n');
}

static void sax(const char *data, int size, int options) {
    htmlSAXHandler handler;
    memset(&handler, 0, sizeof(handler));
    handler.internalSubset = doctype;
    handler.startElement = start;
    handler.endElement = end;
    handler.characters = text;
    handler.ignorableWhitespace = ws;
    handler.cdataBlock = cdata;
    handler.comment = comment;
    handler.initialized = 1;
    htmlParserCtxtPtr ctxt = htmlNewSAXParserCtxt(&handler, NULL);
    xmlCtxtSetErrorHandler(ctxt, record, NULL);
    xmlDocPtr doc = htmlCtxtReadMemory(ctxt, data, size, NULL, NULL, options);
    xmlFreeDoc(doc);
    htmlFreeParserCtxt(ctxt);
}

static void walk(xmlNodePtr node, int depth) {
    for (; node != NULL; node = node->next) {
        printf("node\t%d\t%d", depth, node->type);
        escaped(node->name, -1);
        printf("\t%ld\n", xmlGetLineNo(node));
        if (node->type == XML_ELEMENT_NODE)
            walk(node->children, depth + 1);
    }
}

static void lines(const char *data, int size, int options) {
    htmlParserCtxtPtr ctxt = htmlNewParserCtxt();
    xmlCtxtSetErrorHandler(ctxt, record, NULL);
    xmlDocPtr doc = htmlCtxtReadMemory(ctxt, data, size, NULL, NULL, options);
    if (doc != NULL)
        walk(doc->children, 0);
    xmlFreeDoc(doc);
    htmlFreeParserCtxt(ctxt);
}

static void fragment(const char *data, int size, int options) {
    const char *context = getenv("YAXML_HTML_CONTEXT");
    if (context == NULL)
        context = "<html><body></body></html>";
    xmlDocPtr doc = htmlReadMemory(context, (int)strlen(context), NULL, NULL, HTML_PARSE_NOERROR);
    xmlNodePtr body = xmlDocGetRootElement(doc)->last;
    xmlNodePtr list = NULL;
    xmlSetStructuredErrorFunc(NULL, record);
    int code = xmlParseInNodeContext(body, data, size, options, &list);
    xmlSetStructuredErrorFunc(NULL, NULL);
    printf("result\t%d\n", code);
    fflush(stdout);
    if (list != NULL)
        xmlDebugDumpNodeList(stdout, list, 0);
    fflush(stdout);
    xmlFreeNodeList(list);
    xmlFreeDoc(doc);
}

int main(int argc, char **argv) {
    if (argc < 3) {
        fprintf(stderr, "usage: %s sax|lines|fragment OPTIONS FILE...\n", argv[0]);
        return 2;
    }
    int options = atoi(argv[2]);
    for (int i = 3; i < argc; i++) {
        FILE *f = fopen(argv[i], "rb");
        if (f == NULL) {
            perror(argv[i]);
            return 2;
        }
        char *data = NULL;
        size_t size = 0, cap = 0, n;
        do {
            if (size == cap) {
                cap = cap ? cap * 2 : 4096;
                data = realloc(data, cap);
            }
            n = fread(data + size, 1, cap - size, f);
            size += n;
        } while (n > 0);
        fclose(f);
        printf("file\t%s\n", argv[i]);
        if (strcmp(argv[1], "sax") == 0)
            sax(data, (int)size, options);
        else if (strcmp(argv[1], "lines") == 0)
            lines(data, (int)size, options);
        else
            fragment(data, (int)size, options);
        fflush(stdout);
        free(data);
    }
    return 0;
}
