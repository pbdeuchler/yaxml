/* Fixed construction oracle only; Cargo never compiles or links this file. */
#include <libxml/parser.h>
#include <libxml/tree.h>
#include <libxml/xmlversion.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static void fail(const char *message) {
    fprintf(stderr, "%s\n", message);
    exit(1);
}

static xmlNode *element(xmlDoc *doc, xmlNode *parent, const char *local) {
    xmlNode *node = xmlNewDocNode(doc, NULL, BAD_CAST local, NULL);
    if (node == NULL) fail("element allocation failed");
    if (parent == NULL) xmlDocSetRootElement(doc, node);
    else if (xmlAddChild(parent, node) != node) fail("element append failed");
    return node;
}

static xmlNs *binding(xmlNode *node, const char *prefix, const char *uri) {
    xmlNs *ns = xmlNewNs(node, BAD_CAST uri, BAD_CAST prefix);
    if (ns == NULL) fail("namespace allocation failed");
    return ns;
}

static void attribute(xmlNode *node, xmlNs *ns, const char *local, const char *value) {
    if (xmlNewNsProp(node, ns, BAD_CAST local, BAD_CAST value) == NULL)
        fail("attribute allocation failed");
}

static void emit(const char *label, xmlDoc *doc) {
    xmlBuffer *buffer = xmlBufferCreate();
    if (buffer == NULL) fail("buffer allocation failed");
    if (xmlNodeDump(buffer, doc, xmlDocGetRootElement(doc), 0, 0) < 0)
        fail("serialization failed");
    const xmlChar *bytes = xmlBufferContent(buffer);
    size_t length = xmlBufferLength(buffer);
    if (length > 65536) fail("oracle output ceiling");
    if (printf("%s\t", label) < 0) fail("stdout failed");
    for (size_t i = 0; i < length; ++i)
        if (printf("%02x", bytes[i]) < 0) fail("stdout failed");
    if (putchar('\n') == EOF) fail("stdout failed");
    xmlBufferFree(buffer);
    xmlFreeDoc(doc);
}

int main(int argc, char **argv) {
    if (argc == 2 && strcmp(argv[1], "--version") == 0) {
        if (printf("YAXML-CREATE-1 compiled=%d runtime=%s tree=%d output=%d\n",
                LIBXML_VERSION, xmlParserVersion,
                xmlHasFeature(XML_WITH_TREE), xmlHasFeature(XML_WITH_OUTPUT)) < 0)
            fail("stdout failed");
        return 0;
    }
    if (argc != 1) fail("unexpected construction oracle arguments");
    const char *labels[] = {
        "plain", "attributes", "prefixed-root", "default-root", "inherited-prefix",
        "default-undeclaration", "prefix-shadow", "xml-attribute", "unicode",
        "literal-text", "automatic-element", "automatic-attribute"
    };
    for (size_t i = 0; i < sizeof(labels) / sizeof(labels[0]); ++i) {
        xmlDoc *doc = xmlNewDoc(BAD_CAST "1.0");
        if (doc == NULL) fail("document allocation failed");
        xmlNode *root = element(doc, NULL, i == 8 ? "élem" : "root");
        xmlNode *child;
        xmlNs *ns;
        switch (i) {
            case 0: break;
            case 1:
                attribute(root, NULL, "id", "A&B");
                attribute(root, NULL, "quote", "\"<>");
                break;
            case 2:
                xmlSetNs(root, binding(root, "p", "urn:p"));
                break;
            case 3:
                xmlSetNs(root, binding(root, NULL, "urn:d"));
                break;
            case 4:
                ns = binding(root, "p", "urn:p");
                child = element(doc, root, "child");
                xmlSetNs(child, ns);
                attribute(child, ns, "id", "17");
                break;
            case 5:
                xmlSetNs(root, binding(root, NULL, "urn:d"));
                child = element(doc, root, "child");
                binding(child, NULL, "");
                break;
            case 6:
                xmlSetNs(root, binding(root, "p", "urn:old"));
                child = element(doc, root, "child");
                ns = binding(child, "p", "urn:new");
                xmlSetNs(child, ns);
                attribute(child, ns, "id", "17");
                break;
            case 7:
                ns = xmlSearchNs(doc, root, BAD_CAST "xml");
                if (ns == NULL) fail("implicit xml binding unavailable");
                attribute(root, ns, "lang", "en");
                break;
            case 8:
                attribute(root, NULL, "clé", "café🙂");
                break;
            case 9:
                child = xmlNewDocText(doc, BAD_CAST "A&B <x> 🙂");
                if (child == NULL || xmlAddChild(root, child) != child)
                    fail("literal text append failed");
                break;
            case 10:
                binding(root, "p", "urn:p");
                child = element(doc, root, "child");
                xmlSetNs(child, binding(child, NULL, "urn:p"));
                break;
            case 11:
                ns = binding(root, "p", "urn:p");
                child = element(doc, root, "child");
                attribute(child, ns, "id", "17");
                break;
            default: fail("unknown fixed case");
        }
        emit(labels[i], doc);
    }
    return ferror(stdout) ? 1 : 0;
}
