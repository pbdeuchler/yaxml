/* Metadata oracle for the checksum-pinned W3C XML conformance catalog. */
#define _POSIX_C_SOURCE 200809L

#include <libxml/hash.h>
#include <libxml/parser.h>
#include <libxml/tree.h>
#include <libxml/uri.h>
#include <libxml/xpath.h>

#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>

#define MAX_TESTS 10000
#define MAX_FIELD 1024

static void error(const char *message) {
    fprintf(stderr, "xmlconf catalog: %s\n", message);
}

static int safe_field(const xmlChar *value, const char *name) {
    const unsigned char *p;
    if (value == NULL)
        return 1;
    if (strlen((const char *) value) > MAX_FIELD) {
        fprintf(stderr, "xmlconf catalog: %s exceeds %d bytes\n", name, MAX_FIELD);
        return 0;
    }
    for (p = value; *p != '\0'; p++) {
        if (*p == '\t' || *p == '\n' || *p == '\r') {
            fprintf(stderr, "xmlconf catalog: %s contains a TSV control byte\n", name);
            return 0;
        }
    }
    return 1;
}

static int valid_type(const xmlChar *type) {
    return xmlStrEqual(type, BAD_CAST "valid") ||
           xmlStrEqual(type, BAD_CAST "invalid") ||
           xmlStrEqual(type, BAD_CAST "not-wf") ||
           xmlStrEqual(type, BAD_CAST "error");
}

/* xmlBuildURI performs URI dot-segment normalization. No file is opened here. */
static char *relative_case_path(xmlDocPtr doc, xmlNodePtr node,
                                const char *suite_root, const xmlChar *uri) {
    xmlChar *base = xmlNodeGetBase(doc, node);
    xmlChar *resolved = NULL;
    xmlURIPtr parsed = NULL;
    const char *path;
    char *relative = NULL;
    size_t root_len;

    if (base == NULL)
        goto done;
    resolved = xmlBuildURI(uri, base);
    if (resolved == NULL)
        goto done;
    parsed = xmlParseURI((const char *) resolved);
    if (parsed == NULL ||
        (parsed->scheme != NULL && strcmp(parsed->scheme, "file") != 0) ||
        (parsed->server != NULL && parsed->server[0] != '\0' &&
         strcmp(parsed->server, "localhost") != 0) ||
        parsed->query != NULL || parsed->fragment != NULL)
        goto done;
    path = parsed->path;
    if (path == NULL || path[0] != '/' || strchr(path, '%') != NULL)
        goto done;

    root_len = strlen(suite_root);
    if (strncmp(path, suite_root, root_len) != 0 ||
        (path[root_len] != '\0' && path[root_len] != '/'))
        goto done;
    if (root_len == 1 && suite_root[0] == '/')
        relative = strdup(path + 1);
    else if (path[root_len] == '/')
        relative = strdup(path + root_len + 1);
    else
        relative = strdup(".");
done:
    if (parsed != NULL)
        xmlFreeURI(parsed);
    xmlFree(resolved);
    xmlFree(base);
    return relative;
}

static int emit_test(xmlDocPtr doc, xmlNodePtr node, const char *suite_root,
                     xmlHashTablePtr ids, size_t *type_counts, size_t *emitted) {
    static const char *const names[] = {
        "ID", "TYPE", "ENTITIES", "RECOMMENDATION", "VERSION", "EDITION",
        "NAMESPACE", "URI", "OUTPUT", "OUTPUT3", "SECTIONS"
    };
    xmlChar *f[11] = {0};
    char *path = NULL;
    int ok = 0;
    size_t i, type_index;

    for (i = 0; i < 11; i++) {
        f[i] = xmlGetProp(node, BAD_CAST names[i]);
        if (f[i] == NULL && (i == 0 || i == 1 || i == 2 || i == 3 ||
                             i == 6 || i == 7 || i == 10)) {
            fprintf(stderr, "xmlconf catalog: TEST is missing required %s\n", names[i]);
            goto done;
        }
        if (!safe_field(f[i], names[i]))
            goto done;
    }
    if (!valid_type(f[1])) {
        fprintf(stderr, "xmlconf catalog: unknown TYPE for ID %s\n", f[0]);
        goto done;
    }
    if (xmlHashLookup(ids, f[0]) != NULL) {
        fprintf(stderr, "xmlconf catalog: duplicate ID: %s\n", f[0]);
        goto done;
    }
    if (xmlHashAddEntry(ids, f[0], node) != 0) {
        error("could not record catalog ID");
        goto done;
    }
    if (xmlStrEqual(f[1], BAD_CAST "valid"))
        type_index = 0;
    else if (xmlStrEqual(f[1], BAD_CAST "invalid"))
        type_index = 1;
    else if (xmlStrEqual(f[1], BAD_CAST "not-wf"))
        type_index = 2;
    else
        type_index = 3;

    path = relative_case_path(doc, node, suite_root, f[7]);
    if (path == NULL) {
        fprintf(stderr, "xmlconf catalog: nonlocal or outside-root URI for ID %s\n", f[0]);
        goto done;
    }
    if (printf("%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n",
               f[0], f[1], f[2], f[3], f[4] == NULL ? BAD_CAST "" : f[4],
               f[5] == NULL ? BAD_CAST "" : f[5], f[6], f[7], path,
               f[8] == NULL ? BAD_CAST "" : f[8],
               f[9] == NULL ? BAD_CAST "" : f[9], f[10]) < 0) {
        error("failed writing TSV row");
        goto done;
    }
    type_counts[type_index]++;
    (*emitted)++;
    ok = 1;
done:
    free(path);
    for (i = 0; i < 11; i++)
        xmlFree(f[i]);
    return ok;
}

static int version_command(void) {
    printf("compiled=%d runtime=%s\n", LIBXML_VERSION,
           xmlParserVersion == NULL ? "unknown" : xmlParserVersion);
    return LIBXML_VERSION == 21504 && xmlParserVersion != NULL &&
           strcmp(xmlParserVersion, "21504") == 0 ? 0 : 1;
}

int main(int argc, char **argv) {
    char catalog_path[PATH_MAX], root_path[PATH_MAX];
    struct stat root_stat, catalog_stat;
    xmlDocPtr doc = NULL;
    xmlXPathContextPtr context = NULL;
    xmlXPathObjectPtr tests = NULL;
    xmlHashTablePtr ids = NULL;
    size_t type_counts[4] = {0, 0, 0, 0};
    size_t emitted = 0;
    int i, result = 1;

    if (argc == 2 && strcmp(argv[1], "--version") == 0)
        return version_command();
    if (argc != 3 || argv[1][0] != '/' || argv[2][0] != '/' ||
        realpath(argv[1], catalog_path) == NULL || realpath(argv[2], root_path) == NULL) {
        error("usage: helper ABSOLUTE_CATALOG.xml ABSOLUTE_SUITE_ROOT | --version");
        return 2;
    }
    if (stat(root_path, &root_stat) != 0 || !S_ISDIR(root_stat.st_mode) ||
        stat(catalog_path, &catalog_stat) != 0 || !S_ISREG(catalog_stat.st_mode)) {
        error("suite root must be a directory and catalog must be a regular file");
        return 2;
    }
    if (!(root_path[0] == '/' && root_path[1] == '\0') &&
        (strncmp(catalog_path, root_path, strlen(root_path)) != 0 ||
        (catalog_path[strlen(root_path)] != '\0' &&
         catalog_path[strlen(root_path)] != '/'))) {
        error("catalog must be inside suite root");
        return 2;
    }
    if (LIBXML_VERSION != 21504 || xmlParserVersion == NULL ||
        strcmp(xmlParserVersion, "21504") != 0) {
        error("helper must be compiled and run with libxml2 2.15.4 (21504)");
        return 2;
    }

    xmlInitParser();
    doc = xmlReadFile(catalog_path, NULL,
                      XML_PARSE_NOENT | XML_PARSE_DTDATTR | XML_PARSE_NONET);
    if (doc == NULL) {
        error("could not parse catalog");
        goto done;
    }
    if (xmlDocGetRootElement(doc) == NULL ||
        !xmlStrEqual(xmlDocGetRootElement(doc)->name, BAD_CAST "TESTSUITE")) {
        error("catalog root must be TESTSUITE");
        goto done;
    }
    context = xmlXPathNewContext(doc);
    if (context == NULL) {
        error("could not create XPath context");
        goto done;
    }
    tests = xmlXPathEvalExpression(BAD_CAST "//TEST", context);
    if (tests == NULL || tests->type != XPATH_NODESET || tests->nodesetval == NULL ||
        tests->nodesetval->nodeNr <= 0 || tests->nodesetval->nodeNr > MAX_TESTS) {
        error("catalog TEST count is empty or exceeds 10000");
        goto done;
    }
    ids = xmlHashCreate(MAX_TESTS);
    if (ids == NULL) {
        error("could not allocate ID set");
        goto done;
    }
    if (printf("yaxml-xmlconf-v1\t%d\n", tests->nodesetval->nodeNr) < 0) {
        error("failed writing TSV header");
        goto done;
    }
    for (i = 0; i < tests->nodesetval->nodeNr; i++) {
        xmlNodePtr node = tests->nodesetval->nodeTab[i];
        if (node == NULL || node->type != XML_ELEMENT_NODE ||
            !xmlStrEqual(node->name, BAD_CAST "TEST") ||
            !emit_test(doc, node, root_path, ids, type_counts, &emitted))
            goto done;
    }
    if (emitted != (size_t) tests->nodesetval->nodeNr ||
        type_counts[0] + type_counts[1] + type_counts[2] + type_counts[3] != emitted) {
        error("emitted or type count differs from catalog TEST count");
        goto done;
    }
    if (fflush(stdout) != 0 || ferror(stdout)) {
        error("failed flushing manifest");
        goto done;
    }
    result = 0;
done:
    if (ids != NULL)
        xmlHashFree(ids, NULL);
    if (tests != NULL)
        xmlXPathFreeObject(tests);
    if (context != NULL)
        xmlXPathFreeContext(context);
    if (doc != NULL)
        xmlFreeDoc(doc);
    xmlCleanupParser();
    return result;
}
