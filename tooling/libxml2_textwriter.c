/* xmlTextWriter oracle only; Cargo never compiles or links this file.
 *
 * Reads one call script per line: space-separated calls `OP` or
 * `OP:arg,arg,...` against a fresh xmlNewTextWriterMemory writer. An argument
 * is hex bytes, `.` for the empty string or `-` for NULL; integer arguments
 * (`IN`, `SY`, `WI`, `WJ`, `WY` pe) are hex too. Prints one line per script:
 * each call's result (`0` success, `1` for -1), a TAB, then
 * hex(output after xmlFreeTextWriter) or `.` when empty. See
 * tests/text_writer_oracle.rs for the operation codes. */
#include <libxml/xmlwriter.h>
#include <libxml/xmlmemory.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static int nibble(int c) {
    return c <= '9' ? c - '0' : (c | 32) - 'a' + 10;
}

/* Decodes argument `a`; NULL for `-`. */
static xmlChar *arg(const char *a, size_t *length) {
    *length = 0;
    if (strcmp(a, "-") == 0)
        return NULL;
    if (strcmp(a, ".") == 0)
        return (xmlChar *)calloc(1, 1);
    size_t n = strlen(a) / 2;
    xmlChar *out = malloc(n + 1);
    for (size_t i = 0; i < n; i++)
        out[i] = (xmlChar)(nibble(a[2 * i]) << 4 | nibble(a[2 * i + 1]));
    out[n] = 0;
    *length = n;
    return out;
}

static int number(const xmlChar *a) {
    return a == NULL ? 0 : atoi((const char *)a);
}

static int call(xmlTextWriterPtr w, const char *op, xmlChar **a, size_t *n) {
#define S(i) ((const char *)a[i])
#define OP(code) (strcmp(op, code) == 0)
    if (OP("SD")) return xmlTextWriterStartDocument(w, S(0), S(1), S(2));
    if (OP("ED")) return xmlTextWriterEndDocument(w);
    if (OP("SE")) return xmlTextWriterStartElement(w, a[0]);
    if (OP("SN")) return xmlTextWriterStartElementNS(w, a[0], a[1], a[2]);
    if (OP("EE")) return xmlTextWriterEndElement(w);
    if (OP("FE")) return xmlTextWriterFullEndElement(w);
    if (OP("SA")) return xmlTextWriterStartAttribute(w, a[0]);
    if (OP("SB")) return xmlTextWriterStartAttributeNS(w, a[0], a[1], a[2]);
    if (OP("EA")) return xmlTextWriterEndAttribute(w);
    if (OP("WA")) return xmlTextWriterWriteAttribute(w, a[0], a[1]);
    if (OP("WB")) return xmlTextWriterWriteAttributeNS(w, a[0], a[1], a[2], a[3]);
    if (OP("WE")) return xmlTextWriterWriteElement(w, a[0], a[1]);
    if (OP("WF")) return xmlTextWriterWriteElementNS(w, a[0], a[1], a[2], a[3]);
    if (OP("WS")) return xmlTextWriterWriteString(w, a[0]);
    if (OP("WR")) return xmlTextWriterWriteRaw(w, a[0]);
    if (OP("W6")) return xmlTextWriterWriteBase64(w, S(0), 0, (int)n[0]);
    if (OP("WH")) return xmlTextWriterWriteBinHex(w, S(0), 0, (int)n[0]);
    if (OP("SC")) return xmlTextWriterStartComment(w);
    if (OP("EC")) return xmlTextWriterEndComment(w);
    if (OP("WC")) return xmlTextWriterWriteComment(w, a[0]);
    if (OP("SP")) return xmlTextWriterStartPI(w, a[0]);
    if (OP("EP")) return xmlTextWriterEndPI(w);
    if (OP("WP")) return xmlTextWriterWritePI(w, a[0], a[1]);
    if (OP("SX")) return xmlTextWriterStartCDATA(w);
    if (OP("EX")) return xmlTextWriterEndCDATA(w);
    if (OP("WX")) return xmlTextWriterWriteCDATA(w, a[0]);
    if (OP("ST")) return xmlTextWriterStartDTD(w, a[0], a[1], a[2]);
    if (OP("ET")) return xmlTextWriterEndDTD(w);
    if (OP("WT")) return xmlTextWriterWriteDTD(w, a[0], a[1], a[2], a[3]);
    if (OP("SL")) return xmlTextWriterStartDTDElement(w, a[0]);
    if (OP("EL")) return xmlTextWriterEndDTDElement(w);
    if (OP("WL")) return xmlTextWriterWriteDTDElement(w, a[0], a[1]);
    if (OP("SK")) return xmlTextWriterStartDTDAttlist(w, a[0]);
    if (OP("EK")) return xmlTextWriterEndDTDAttlist(w);
    if (OP("WK")) return xmlTextWriterWriteDTDAttlist(w, a[0], a[1]);
    if (OP("SY")) return xmlTextWriterStartDTDEntity(w, number(a[0]), a[1]);
    if (OP("EY")) return xmlTextWriterEndDTDEntity(w);
    if (OP("WI")) return xmlTextWriterWriteDTDInternalEntity(w, number(a[0]), a[1], a[2]);
    if (OP("WJ")) return xmlTextWriterWriteDTDExternalEntity(w, number(a[0]), a[1], a[2], a[3], a[4]);
    if (OP("WG")) return xmlTextWriterWriteDTDExternalEntityContents(w, a[0], a[1], a[2]);
    if (OP("WY")) return xmlTextWriterWriteDTDEntity(w, number(a[0]), a[1], a[2], a[3], a[4], a[5]);
    if (OP("WN")) return xmlTextWriterWriteDTDNotation(w, a[0], a[1], a[2]);
    if (OP("IN")) return xmlTextWriterSetIndent(w, number(a[0]));
    if (OP("IS")) return xmlTextWriterSetIndentString(w, a[0]);
    if (OP("QC")) return xmlTextWriterSetQuoteChar(w, a[0] == NULL ? 0 : a[0][0]);
    if (OP("FL")) return xmlTextWriterFlush(w);
    fprintf(stderr, "unknown operation %s\n", op);
    exit(2);
#undef S
#undef OP
}

int main(void) {
    static char line[1 << 22];
    while (fgets(line, sizeof line, stdin) != NULL) {
        line[strcspn(line, "\n")] = 0;
        xmlBufferPtr buffer = xmlBufferCreate();
        xmlTextWriterPtr writer = xmlNewTextWriterMemory(buffer, 0);
        for (char *save = NULL, *token = strtok_r(line, " ", &save); token != NULL;
             token = strtok_r(NULL, " ", &save)) {
            xmlChar *args[6] = {0};
            size_t lengths[6] = {0};
            char *colon = strchr(token, ':');
            if (colon != NULL) {
                *colon = 0;
                int i = 0;
                for (char *inner = NULL, *a = strtok_r(colon + 1, ",", &inner);
                     a != NULL && i < 6; a = strtok_r(NULL, ",", &inner))
                    args[i] = arg(a, &lengths[i]), i++;
            }
            putchar(call(writer, token, args, lengths) < 0 ? '1' : '0');
            for (int i = 0; i < 6; i++)
                free(args[i]);
        }
        xmlFreeTextWriter(writer);
        putchar('\t');
        if (xmlBufferLength(buffer) == 0)
            putchar('.');
        for (int i = 0; i < xmlBufferLength(buffer); i++)
            printf("%02x", xmlBufferContent(buffer)[i]);
        putchar('\n');
        xmlBufferFree(buffer);
    }
    return 0;
}
