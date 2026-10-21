/* XML Schema oracle; Cargo never compiles or links this.
 *
 * Reads one request per stdin line: a hex-encoded schema document and a
 * hex-encoded instance separated by a tab ("0" is the empty string). Prints
 * one line per request:
 *   S codes...      xmlSchemaParse failed; the schema error codes
 *   V r codes...    xmlSchemaValidateDoc returned r (0 valid, >0 invalid,
 *                   <0 internal); the validity error codes
 *   X               the instance is not well-formed
 * Warnings are not listed. The schema's URI is "schema.xsd". With the
 * argument "stream", well-formed instances are validated again from memory
 * by xmlSchemaValidateStream and "V" reports that instead. */
#include <libxml/parser.h>
#include <libxml/xmlschemas.h>
#include <libxml/xmlerror.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static char codes[65536];

static void collect(void *ctx, const xmlError *error) {
    (void)ctx;
    if (error->level < XML_ERR_ERROR)
        return;
    if (error->domain != XML_FROM_SCHEMASP && error->domain != XML_FROM_SCHEMASV)
        return;
    size_t used = strlen(codes);
    if (used + 16 < sizeof(codes))
        snprintf(codes + used, sizeof(codes) - used, " %d", error->code);
}

static void quiet(void *ctx, const xmlError *error) { (void)ctx; (void)error; }

static int nibble(int c) {
    return c <= '9' ? c - '0' : (c | 32) - 'a' + 10;
}

static char *unhex(const char *h, size_t *len) {
    size_t n = strcmp(h, "0") ? strlen(h) / 2 : 0;
    char *out = malloc(n + 1);
    for (size_t i = 0; i < n; i++)
        out[i] = (char)(nibble(h[2 * i]) << 4 | nibble(h[2 * i + 1]));
    out[n] = 0;
    *len = n;
    return out;
}

int main(int argc, char **argv) {
    int stream = argc > 1 && !strcmp(argv[1], "stream");
    static char line[1 << 22];
    xmlSetStructuredErrorFunc(NULL, quiet);
    while (fgets(line, sizeof(line), stdin)) {
        line[strcspn(line, "\r\n")] = 0;
        char *tab = strchr(line, '\t');
        if (!tab)
            continue;
        *tab = 0;
        size_t slen, ilen;
        char *schema_text = unhex(line, &slen);
        char *instance_text = unhex(tab + 1, &ilen);
        codes[0] = 0;
        xmlSchemaParserCtxtPtr pctxt = xmlSchemaNewMemParserCtxt(schema_text, (int)slen);
        xmlSchemaSetParserStructuredErrors(pctxt, collect, NULL);
        xmlSchemaPtr schema = xmlSchemaParse(pctxt);
        xmlSchemaFreeParserCtxt(pctxt);
        if (!schema) {
            printf("S%s\n", codes);
        } else {
            codes[0] = 0;
            xmlDocPtr doc = xmlReadMemory(instance_text, (int)ilen, "doc.xml", NULL, 0);
            if (!doc) {
                printf("X\n");
            } else {
                xmlSchemaValidCtxtPtr vctxt = xmlSchemaNewValidCtxt(schema);
                xmlSchemaSetValidStructuredErrors(vctxt, collect, NULL);
                int ret;
                if (stream) {
                    xmlParserInputBufferPtr input = xmlParserInputBufferCreateMem(
                        instance_text, (int)ilen, XML_CHAR_ENCODING_NONE);
                    ret = xmlSchemaValidateStream(vctxt, input, XML_CHAR_ENCODING_NONE, NULL, NULL);
                } else {
                    ret = xmlSchemaValidateDoc(vctxt, doc);
                }
                printf("V %d%s\n", ret, codes);
                xmlSchemaFreeValidCtxt(vctxt);
                xmlFreeDoc(doc);
            }
            xmlSchemaFree(schema);
        }
        fflush(stdout);
        free(schema_text);
        free(instance_text);
    }
    return 0;
}
