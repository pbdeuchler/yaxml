/* chvalid.h and DTD declaration oracle; Cargo never compiles or links this.
 *
 *   libxml2_dtd chars
 *     prints "CLASS LOW HIGH" (hex) for every maximal code point run in
 *     0..=0x10FFFF of xmlIsChar, xmlIsBlank, xmlIsBaseChar, xmlIsDigit,
 *     xmlIsCombining, xmlIsExtender, xmlIsIdeographic and xmlIsPubidChar, and
 *     "CLASS_ch LOW HIGH" for the byte macros over 0..=0xFF.
 *   libxml2_dtd encode
 *     reads hex-encoded strings ("0" is empty) from stdin and prints, tab
 *     separated: xmlEncodeEntitiesReentrant for a document with an encoding,
 *     without one and for an HTML document, then xmlEncodeSpecialChars.
 *   libxml2_dtd dtd ROOT FLAGS
 *     chdir(ROOT), then reads relative file names from stdin, parses each
 *     with xmlReadFile(name, NULL, FLAGS) and prints "file NAME", every
 *     declaration of both subsets (entity, element, attribute, notation
 *     lines with the subset first) and the lookups xmlGetDocEntity
 *     ("doc"), xmlGetParameterEntity ("pe") and xmlGetDtdEntity ("dtd") of
 *     every declared name and the predefined names, then "end". Fields are
 *     tab separated with \\, \t, \n and \r escaped; "-" is NULL. */
#include <libxml/parser.h>
#include <libxml/HTMLtree.h>
#include <libxml/chvalid.h>
#include <libxml/entities.h>
#include <libxml/valid.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static void quiet(void *ctx, const xmlError *error) { (void)ctx; (void)error; }

static void field(const xmlChar *s) {
    putchar('\t');
    if (s == NULL) {
        putchar('-');
        return;
    }
    for (; *s; s++) {
        switch (*s) {
        case '\\': fputs("\\\\", stdout); break;
        case '\t': fputs("\\t", stdout); break;
        case '\n': fputs("\\n", stdout); break;
        case '\r': fputs("\\r", stdout); break;
        default: putchar(*s);
        }
    }
}

static void owned(xmlChar *s) {
    field(s);
    xmlFree(s);
}

static void runs(const char *name, int (*is)(unsigned int), unsigned int last) {
    unsigned int c, low = 0;
    int inside = 0;
    for (c = 0; c <= last + 1; c++) {
        int now = c <= last && is(c);
        if (now && !inside)
            low = c;
        else if (!now && inside)
            printf("%s %x %x\n", name, low, c - 1);
        inside = now;
    }
}

#define BYTE(macro) \
    static int macro##_byte(unsigned int c) { return macro((unsigned char)c) != 0; }
BYTE(xmlIsChar_ch)
BYTE(xmlIsBlank_ch)
BYTE(xmlIsBaseChar_ch)
BYTE(xmlIsDigit_ch)
BYTE(xmlIsExtender_ch)
BYTE(xmlIsPubidChar_ch)

static void chars(void) {
    runs("Char", xmlIsChar, 0x10ffff);
    runs("Blank", xmlIsBlank, 0x10ffff);
    runs("BaseChar", xmlIsBaseChar, 0x10ffff);
    runs("Digit", xmlIsDigit, 0x10ffff);
    runs("Combining", xmlIsCombining, 0x10ffff);
    runs("Extender", xmlIsExtender, 0x10ffff);
    runs("Ideographic", xmlIsIdeographic, 0x10ffff);
    runs("PubidChar", xmlIsPubidChar, 0x10ffff);
    runs("Char_ch", xmlIsChar_ch_byte, 0xff);
    runs("Blank_ch", xmlIsBlank_ch_byte, 0xff);
    runs("BaseChar_ch", xmlIsBaseChar_ch_byte, 0xff);
    runs("Digit_ch", xmlIsDigit_ch_byte, 0xff);
    runs("Extender_ch", xmlIsExtender_ch_byte, 0xff);
    runs("PubidChar_ch", xmlIsPubidChar_ch_byte, 0xff);
}

static int nibble(int c) { return c <= '9' ? c - '0' : (c | 32) - 'a' + 10; }

static void encode(void) {
    char line[1 << 16];
    xmlDocPtr encoded = xmlNewDoc(BAD_CAST "1.0");
    xmlDocPtr html = htmlNewDocNoDtD(NULL, NULL);
    encoded->encoding = xmlStrdup(BAD_CAST "UTF-8");
    while (fgets(line, sizeof line, stdin)) {
        size_t n = strcspn(line, "\n"), i;
        xmlChar text[1 << 15];
        if (n == 1 && line[0] == '0')
            n = 0;
        for (i = 0; i < n / 2; i++)
            text[i] = (xmlChar)(nibble(line[2 * i]) << 4 | nibble(line[2 * i + 1]));
        text[n / 2] = 0;
        fputs("enc", stdout);
        owned(xmlEncodeEntitiesReentrant(encoded, text));
        owned(xmlEncodeEntitiesReentrant(NULL, text));
        owned(xmlEncodeEntitiesReentrant(html, text));
        owned(xmlEncodeSpecialChars(NULL, text));
        putchar('\n');
    }
    xmlFreeDoc(encoded);
    xmlFreeDoc(html);
}

static const char *subset;

static void entity(void *payload, void *data, const xmlChar *name) {
    xmlEntityPtr e = payload;
    (void)data;
    (void)name;
    printf("entity\t%s\t%d", subset, (int)e->etype);
    field(e->name);
    field(e->ExternalID);
    field(e->SystemID);
    field(e->URI);
    field(e->content);
    putchar('\n');
}

static void qualified(const xmlChar *prefix, const xmlChar *name) {
    xmlChar *q = prefix ? xmlBuildQName(name, prefix, NULL, 0) : xmlStrdup(name);
    owned(q);
}

static void element(void *payload, void *data, const xmlChar *name) {
    xmlElementPtr e = payload;
    (void)data;
    (void)name;
    printf("element\t%s\t%d", subset, (int)e->etype);
    qualified(e->prefix, e->name);
    if (e->etype == XML_ELEMENT_TYPE_MIXED || e->etype == XML_ELEMENT_TYPE_ELEMENT) {
        /* "<!ELEMENT name MODEL>\n" without the markup around MODEL. */
        xmlBufferPtr buffer = xmlBufferCreate();
        const xmlChar *text, *space;
        xmlDumpElementDecl(buffer, e);
        text = xmlBufferContent(buffer);
        space = (const xmlChar *)strchr((const char *)text + 10, ' ');
        xmlChar *model = xmlStrndup(space + 1, xmlStrlen(space + 1) - 2);
        owned(model);
        xmlBufferFree(buffer);
    } else {
        field(NULL);
    }
    putchar('\n');
}

static void attribute(void *payload, void *data, const xmlChar *name) {
    xmlAttributePtr a = payload;
    xmlEnumerationPtr v;
    xmlChar *values = NULL;
    (void)data;
    (void)name;
    printf("attribute\t%s", subset);
    field(a->elem);
    qualified(a->prefix, a->name);
    printf("\t%d\t%d", (int)a->atype, (int)a->def);
    field(a->defaultValue);
    for (v = a->tree; v != NULL; v = v->next) {
        if (values != NULL)
            values = xmlStrcat(values, BAD_CAST "|");
        values = xmlStrcat(values, v->name);
    }
    owned(values);
    putchar('\n');
}

static void notation(void *payload, void *data, const xmlChar *name) {
    xmlNotationPtr n = payload;
    (void)data;
    (void)name;
    printf("notation\t%s", subset);
    field(n->name);
    field(n->PublicID);
    field(n->SystemID);
    putchar('\n');
}

static void lookup(const char *kind, xmlEntityPtr e, const xmlChar *name) {
    printf("%s", kind);
    field(name);
    if (e == NULL)
        fputs("\t-", stdout);
    else
        printf("\t%d", (int)e->etype);
    putchar('\n');
}

static void lookups(void *payload, void *data, const xmlChar *name) {
    xmlDocPtr doc = data;
    (void)payload;
    lookup("doc", xmlGetDocEntity(doc, name), name);
    lookup("pe", xmlGetParameterEntity(doc, name), name);
    lookup("dtd", xmlGetDtdEntity(doc, name), name);
}

static void scan(const char *which, xmlDtdPtr dtd, xmlDocPtr doc) {
    if (dtd == NULL)
        return;
    subset = which;
    xmlHashScan(dtd->entities, entity, NULL);
    xmlHashScan(dtd->pentities, entity, NULL);
    xmlHashScan(dtd->elements, element, NULL);
    xmlHashScan(dtd->attributes, attribute, NULL);
    xmlHashScan(dtd->notations, notation, NULL);
    xmlHashScan(dtd->entities, lookups, doc);
    xmlHashScan(dtd->pentities, lookups, doc);
}

static void dtds(const char *root, int flags) {
    char name[4096];
    static const char *predefined[] = {"lt", "gt", "amp", "apos", "quot", "none"};
    if (chdir(root) != 0) {
        perror(root);
        exit(2);
    }
    while (fgets(name, sizeof name, stdin)) {
        xmlDocPtr doc;
        size_t i;
        name[strcspn(name, "\n")] = 0;
        printf("file\t%s\n", name);
        doc = xmlReadFile(name, NULL, flags);
        if (doc != NULL) {
            scan("int", doc->intSubset, doc);
            scan("ext", doc->extSubset, doc);
            for (i = 0; i < sizeof predefined / sizeof *predefined; i++)
                lookup("doc", xmlGetDocEntity(doc, BAD_CAST predefined[i]),
                       BAD_CAST predefined[i]);
            xmlFreeDoc(doc);
        }
        puts("end");
        fflush(stdout);
    }
}

int main(int argc, char **argv) {
    xmlSetStructuredErrorFunc(NULL, quiet);
    if (argc == 2 && !strcmp(argv[1], "chars"))
        chars();
    else if (argc == 2 && !strcmp(argv[1], "encode"))
        encode();
    else if (argc == 4 && !strcmp(argv[1], "dtd"))
        dtds(argv[2], atoi(argv[3]));
    else {
        fprintf(stderr, "usage: %s chars | encode | dtd ROOT FLAGS\n", argv[0]);
        return 2;
    }
    return 0;
}
