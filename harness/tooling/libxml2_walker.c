/* xmlReaderWalker dump for tests/walker.rs: `libxml2_walker FILE OPTIONS MODE`
 * parses FILE with OPTIONS and walks it with xmlTextReaderRead (MODE 0),
 * xmlTextReaderNext (1), xmlTextReaderNextSibling (2), or Read with Next on
 * the elements a, d and y (3), printing every accessor per node. Regenerate:
 * cd tests/fixtures/walker && for each NAME.xml and MODE:
 *   libxml2_walker NAME.xml 0 MODE > NAME.MODE.txt */
#include <stdio.h>
#include <string.h>
#include <stdlib.h>
#include <libxml/parser.h>
#include <libxml/xmlreader.h>
#define S(x) ((x) ? (const char *)(x) : "-")
static void line(xmlTextReaderPtr r) {
    xmlChar *v = xmlTextReaderValue(r);
    printf("%d %d %s %s %s %s e%d v%d a%d h%d",
        xmlTextReaderDepth(r), xmlTextReaderNodeType(r), S(xmlTextReaderConstName(r)),
        S(xmlTextReaderConstLocalName(r)), S(xmlTextReaderConstPrefix(r)),
        S(xmlTextReaderConstNamespaceUri(r)), xmlTextReaderIsEmptyElement(r),
        xmlTextReaderHasValue(r), xmlTextReaderAttributeCount(r), xmlTextReaderHasAttributes(r));
    printf(" [%s]", S(v)); xmlFree(v);
}
int main(int argc, char **argv) {
    int options = argc > 2 ? atoi(argv[2]) : 0;
    xmlDocPtr doc = xmlReadFile(argv[1], NULL, options);
    if (!doc) return 1;
    int mode = argc > 3 ? atoi(argv[3]) : 0;
    xmlTextReaderPtr r = xmlReaderWalker(doc);
    printf("state %d ver %s enc %s sa %d\n", xmlTextReaderReadState(r), S(xmlTextReaderConstXmlVersion(r)), S(xmlTextReaderConstEncoding(r)), xmlTextReaderStandalone(r));
    int ret;
    printf("before %d %d %s %d\n", xmlTextReaderDepth(r), xmlTextReaderNodeType(r), S(xmlTextReaderConstName(r)), xmlTextReaderMoveToFirstAttribute(r));
    for (;;) {
        const char *nm = (const char *)xmlTextReaderConstName(r);
        int skip = mode == 3 && nm && xmlTextReaderNodeType(r) == 1 && (!strcmp(nm, "a") || !strcmp(nm, "d") || !strcmp(nm, "y"));
        ret = mode == 1 || skip ? xmlTextReaderNext(r) : mode == 2 ? xmlTextReaderNextSibling(r) : xmlTextReaderRead(r);
        if (ret != 1) break;
        line(r);
        xmlChar *s = xmlTextReaderReadString(r), *in = xmlTextReaderReadInnerXml(r), *out = xmlTextReaderReadOuterXml(r);
        xmlChar *base = xmlTextReaderBaseUri(r);
        printf(" lang=%s base=%s ns=%s string=%s\n  inner=%s\n  outer=%s\n", S(xmlTextReaderConstXmlLang(r)), S(base),
            S(xmlTextReaderLookupNamespace(r, NULL)), S(s), S(in), S(out));
        xmlFree(s); xmlFree(in); xmlFree(out); xmlFree(base);
        {
            const char *q[] = {"p:k", "xmlns", "xmlns:p", "k", "a", "b", "xml:lang"};
            printf("  get");
            for (int i = 0; i < 7; i++) { xmlChar *g = xmlTextReaderGetAttribute(r, BAD_CAST q[i]); printf(" %s", S(g)); xmlFree(g); }
            xmlChar *g1 = xmlTextReaderGetAttributeNs(r, BAD_CAST "k", BAD_CAST "urn:p"), *g2 = xmlTextReaderGetAttributeNs(r, BAD_CAST "p", BAD_CAST "http://www.w3.org/2000/xmlns/"), *g3 = xmlTextReaderGetAttributeNo(r, 0), *g4 = xmlTextReaderGetAttributeNo(r, 2), *g5 = xmlTextReaderGetAttributeNo(r, 9);
            printf(" ns %s %s no %s %s %s", S(g1), S(g2), S(g3), S(g4), S(g5));
            xmlFree(g1); xmlFree(g2); xmlFree(g3); xmlFree(g4); xmlFree(g5);
            int m1 = xmlTextReaderMoveToAttribute(r, BAD_CAST "xml:lang"); printf(" mv %d %s", m1, S(xmlTextReaderConstName(r)));
            xmlChar *inattr = xmlTextReaderGetAttribute(r, BAD_CAST "a"); printf(" in %s", S(inattr)); xmlFree(inattr);
            xmlTextReaderMoveToElement(r);
            int m2 = xmlTextReaderMoveToAttributeNs(r, BAD_CAST "k", BAD_CAST "urn:p"); printf(" %d %s", m2, S(xmlTextReaderConstName(r)));
            xmlTextReaderMoveToElement(r);
            int m3 = xmlTextReaderMoveToAttributeNo(r, 1); printf(" %d %s", m3, S(xmlTextReaderConstName(r)));
            int m4 = xmlTextReaderMoveToAttributeNo(r, 9); printf(" %d %s", m4, S(xmlTextReaderConstName(r)));
            int m5 = xmlTextReaderMoveToFirstAttribute(r); printf(" %d %s", m5, S(xmlTextReaderConstName(r)));
            int m6 = xmlTextReaderMoveToElement(r), m7 = xmlTextReaderMoveToElement(r);
            printf(" el %d %d look %s %s\n", m6, m7, S(xmlTextReaderLookupNamespace(r, BAD_CAST "p")), S(xmlTextReaderLookupNamespace(r, BAD_CAST "xml")));
        }
        while (xmlTextReaderMoveToNextAttribute(r) == 1) {
            printf("  @"); line(r); printf(" decl%d\n", xmlTextReaderIsNamespaceDecl(r));
        }
        xmlTextReaderMoveToElement(r);
    }
    printf("end %d state %d type %d\n", ret, xmlTextReaderReadState(r), xmlTextReaderNodeType(r));
    xmlFreeTextReader(r); xmlFreeDoc(doc);
    return 0;
}
