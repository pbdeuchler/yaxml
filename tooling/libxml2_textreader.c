/* Fixed xmlTextReader oracle only; Cargo never compiles or links this file.
 *
 * Usage: textreader-helper FILE
 * Reads FILE with xmlReaderForFile(FILE, NULL, NOENT | DTDATTR | NONET) and
 * prints one line per node (see tests/textreader_oracle.rs for the format):
 * cursor properties, every attribute through MoveToNextAttribute, indexed and
 * named attribute lookups, then ReadString/ReadInnerXml/ReadOuterXml except on
 * DocumentType and EndElement nodes.
 * Strings are C-escaped inside quotes; NULL is "~". A failed read prints ERROR. */
#include <libxml/xmlreader.h>
#include <stdio.h>
#include <stdlib.h>

static void str(const xmlChar *s) {
    if (s == NULL) { fputs(" ~", stdout); return; }
    fputs(" \"", stdout);
    for (; *s; s++) {
        switch (*s) {
        case '\n': fputs("\\n", stdout); break;
        case '\r': fputs("\\r", stdout); break;
        case '\t': fputs("\\t", stdout); break;
        case '"': fputs("\\\"", stdout); break;
        case '\\': fputs("\\\\", stdout); break;
        default: fputc(*s, stdout);
        }
    }
    fputc('"', stdout);
}

static void owned(xmlChar *s) {
    str(s);
    xmlFree(s);
}

static void cursor(xmlTextReaderPtr r) {
    printf("%d %d", xmlTextReaderDepth(r), xmlTextReaderNodeType(r));
    str(xmlTextReaderConstName(r));
    str(xmlTextReaderConstLocalName(r));
    str(xmlTextReaderConstPrefix(r));
    str(xmlTextReaderConstNamespaceUri(r));
    printf(" %d %d", xmlTextReaderIsEmptyElement(r), xmlTextReaderHasValue(r));
    str(xmlTextReaderConstValue(r));
    printf(" %d %d", xmlTextReaderAttributeCount(r), xmlTextReaderHasAttributes(r));
}

int main(int argc, char **argv) {
    if (argc != 2) return 2;
    xmlTextReaderPtr r = xmlReaderForFile(argv[1], NULL,
        XML_PARSE_NOENT | XML_PARSE_DTDATTR | XML_PARSE_NONET | XML_PARSE_NOERROR |
        XML_PARSE_NOWARNING);
    if (r == NULL) return 1;
    int ret;
    while ((ret = xmlTextReaderRead(r)) == 1) {
        cursor(r);
        owned(xmlTextReaderBaseUri(r));
        owned(xmlTextReaderXmlLang(r));
        owned(xmlTextReaderLookupNamespace(r, NULL));
        owned(xmlTextReaderLookupNamespace(r, BAD_CAST "p"));
        owned(xmlTextReaderLookupNamespace(r, BAD_CAST "xml"));
        putchar('\n');
        int count = xmlTextReaderAttributeCount(r);
        int type = xmlTextReaderNodeType(r);
        if (type == XML_READER_TYPE_DOCUMENT_TYPE || type == XML_READER_TYPE_END_ELEMENT)
            continue;
        for (int i = 0; i <= count; i++) {
            printf("  #%d", i);
            owned(xmlTextReaderGetAttributeNo(r, i));
            putchar('\n');
        }
        while (xmlTextReaderMoveToNextAttribute(r) == 1) {
            printf("  @");
            cursor(r);
            xmlChar *name = xmlTextReaderName(r);
            xmlChar *local = xmlTextReaderLocalName(r);
            xmlChar *uri = xmlTextReaderNamespaceUri(r);
            /* Lookups by name are made from the element, as libxml2 requires. */
            xmlTextReaderMoveToElement(r);
            owned(xmlTextReaderGetAttribute(r, name));
            owned(xmlTextReaderGetAttributeNs(r, local, uri));
            putchar('\n');
            xmlTextReaderMoveToAttribute(r, name);
            xmlFree(name);
            xmlFree(local);
            xmlFree(uri);
        }
        printf("  content %d", xmlTextReaderMoveToElement(r));
        owned(xmlTextReaderReadString(r));
        owned(xmlTextReaderReadInnerXml(r));
        owned(xmlTextReaderReadOuterXml(r));
        putchar('\n');
    }
    if (ret != 0) puts("ERROR");
    xmlFreeTextReader(r);
    return 0;
}
