/* Schematron oracle; Cargo never compiles or links this. Needs a libxml2
 * 2.15.4 built with -DLIBXML2_WITH_SCHEMATRON=ON (see docs/schematron.md).
 *
 *   libxml2_schematron FLAGS[:PARSE] SCHEMA INSTANCE...
 *
 * Compiles SCHEMA as xmllint and runtest do (xmlSchematronNewParserCtxt),
 * then validates each INSTANCE, read with xmlParserOption PARSE (default
 * 0), with xmlSchematronValidOptions FLAGS. All
 * output goes to stderr in libxml2's order: the text report, structured
 * errors through xmlFormatError, and runtest's verdict line per instance. */
#include <libxml/parser.h>
#include <libxml/schematron.h>
#include <libxml/xmlerror.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>

static void out(void *ctx, const char *msg, ...) {
    va_list ap;
    (void)ctx;
    va_start(ap, msg);
    vfprintf(stderr, msg, ap);
    va_end(ap);
}

static void format(void *ctx, const xmlError *error) {
    (void)ctx;
    xmlFormatError(error, out, NULL);
}

int main(int argc, char **argv) {
    if (argc < 3)
        return 2;
    char *rest;
    int flags = (int)strtol(argv[1], &rest, 10);
    int parse = *rest == ':' ? atoi(rest + 1) : 0;
    xmlSetStructuredErrorFunc(NULL, format);
    xmlSchematronParserCtxtPtr pctxt = xmlSchematronNewParserCtxt(argv[2]);
    xmlSchematronPtr schema = xmlSchematronParse(pctxt);
    xmlSchematronFreeParserCtxt(pctxt);
    if (schema == NULL) {
        fprintf(stderr, "Schematron schema %s failed to compile\n", argv[2]);
        return 0;
    }
    for (int i = 3; i < argc; i++) {
        xmlDocPtr doc = xmlReadFile(argv[i], NULL, parse);
        if (doc == NULL) {
            fprintf(stderr, "failed to parse instance %s\n", argv[i]);
            continue;
        }
        xmlSchematronValidCtxtPtr ctxt = xmlSchematronNewValidCtxt(schema, flags);
        if (flags & XML_SCHEMATRON_OUT_ERROR)
            xmlSchematronSetValidStructuredErrors(ctxt, format, NULL);
        int ret = xmlSchematronValidateDoc(ctxt, doc);
        if (ret == 0)
            fprintf(stderr, "%s validates\n", argv[i]);
        else if (ret > 0)
            fprintf(stderr, "%s fails to validate\n", argv[i]);
        else
            fprintf(stderr, "%s validation generated an internal error\n", argv[i]);
        xmlSchematronFreeValidCtxt(ctxt);
        xmlFreeDoc(doc);
    }
    xmlSchematronFree(schema);
    return 0;
}
