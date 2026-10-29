/* Fixed xmlTextReader oracle only; Cargo never compiles or links this file.
 *
 * Usage: textreader-helper FILE [FLAGS]
 * Reads FILE with xmlReaderForFile(FILE, NULL, FLAGS | NONET) (FLAGS is a
 * decimal XML_PARSE_* mask, default NOENT | DTDATTR) and prints one line per
 * node (see tests/textreader_oracle.rs for the format): cursor properties
 * (with Standalone, IsDefault, IsNamespaceDecl, QuoteChar, XmlVersion and
 * Encoding), every attribute through
 * MoveToNextAttribute and the nodes of its value through ReadAttributeValue,
 * indexed and named attribute lookups, then ReadString/ReadInnerXml/
 * ReadOuterXml except on DocumentType and EndElement nodes. A first START line
 * has XmlVersion and Encoding before any read; a final END line has
 * Standalone, Normalization, ByteConsumed, XmlVersion and Encoding after the
 * last node.
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
    printf(" %d %d", xmlTextReaderStandalone(r), xmlTextReaderIsDefault(r));
    printf(" %d %c", xmlTextReaderIsNamespaceDecl(r), xmlTextReaderQuoteChar(r));
    str(xmlTextReaderConstXmlVersion(r));
    str(xmlTextReaderConstEncoding(r));
}

int main(int argc, char **argv) {
    if (argc != 2 && argc != 3) return 2;
    int flags = argc == 3 ? atoi(argv[2]) : XML_PARSE_NOENT | XML_PARSE_DTDATTR;
    xmlTextReaderPtr r = xmlReaderForFile(argv[1], NULL,
        flags | XML_PARSE_NONET | XML_PARSE_NOERROR | XML_PARSE_NOWARNING);
    if (r == NULL) return 1;
    int ret;
    fputs("START", stdout);
    str(xmlTextReaderConstXmlVersion(r));
    str(xmlTextReaderConstEncoding(r));
    putchar('\n');
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
            int walked = 0;
            while (xmlTextReaderReadAttributeValue(r) == 1) {
                printf("  ~v");
                cursor(r);
                putchar('\n');
                walked++;
            }
            if (walked) {
                /* From a value there is no next attribute. */
                printf("  ~next %d\n", xmlTextReaderMoveToNextAttribute(r));
                xmlTextReaderMoveToAttribute(r, name);
            }
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
    else {
        printf("END %d %d %ld", xmlTextReaderStandalone(r), xmlTextReaderNormalization(r),
               xmlTextReaderByteConsumed(r));
        str(xmlTextReaderConstXmlVersion(r));
        str(xmlTextReaderConstEncoding(r));
        putchar('\n');
    }
    xmlFreeTextReader(r);
    return 0;
}
