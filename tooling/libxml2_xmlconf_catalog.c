/*
 * Emit the XML Test Suite catalog as a bounded, path-resolved TSV using the
 * pinned libxml2 oracle. This is an external test tool, not library code.
 */
#define _POSIX_C_SOURCE 200809L

#include <libxml/parser.h>
#include <libxml/tree.h>
#include <libxml/uri.h>
#include <libxml/xpath.h>

#include <errno.h>
#include <limits.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#define MAX_CASES 10000
#define PATH_LIMIT (1024 * 1024)

typedef struct {
    char **slots;
    size_t capacity;
    size_t count;
} IdSet;

static void fail(const char *message) {
    fprintf(stderr, "xmlconf catalog: %s\n", message);
}

static int has_forbidden_tsv_byte(const char *value) {
    const unsigned char *p = (const unsigned char *) value;
    for (; *p != '\0'; p++) {
        if (*p == '\t' || *p == '\n' || *p == '\r')
            return 1;
    }
    return 0;
}

static uint64_t hash_id(const char *id) {
    uint64_t hash = UINT64_C(14695981039346656037);
    const unsigned char *p = (const unsigned char *) id;
    for (; *p != '\0'; p++) {
        hash ^= *p;
        hash *= UINT64_C(1099511628211);
    }
    return hash;
}

static int ids_grow(IdSet *ids) {
    size_t new_capacity = ids->capacity == 0 ? 64 : ids->capacity * 2;
    char **new_slots;
    size_t i;

    if (new_capacity < ids->capacity || new_capacity > MAX_CASES * 2)
        return 0;
    new_slots = calloc(new_capacity, sizeof(*new_slots));
    if (new_slots == NULL)
        return 0;
    for (i = 0; i < ids->capacity; i++) {
        char *id = ids->slots[i];
        if (id != NULL) {
            size_t slot = (size_t) (hash_id(id) & (new_capacity - 1));
            while (new_slots[slot] != NULL)
                slot = (slot + 1) & (new_capacity - 1);
            new_slots[slot] = id;
        }
    }
    free(ids->slots);
    ids->slots = new_slots;
    ids->capacity = new_capacity;
    return 1;
}

static int ids_insert(IdSet *ids, const char *id) {
    size_t slot;
    char *copy;

    if (ids->capacity == 0 || (ids->count + 1) * 2 >= ids->capacity) {
        if (!ids_grow(ids))
            return -1;
    }
    slot = (size_t) (hash_id(id) & (ids->capacity - 1));
    while (ids->slots[slot] != NULL) {
        if (strcmp(ids->slots[slot], id) == 0)
            return 0;
        slot = (slot + 1) & (ids->capacity - 1);
    }
    copy = strdup(id);
    if (copy == NULL)
        return -1;
    ids->slots[slot] = copy;
    ids->count++;
    return 1;
}

static void ids_destroy(IdSet *ids) {
    size_t i;
    for (i = 0; i < ids->capacity; i++)
        free(ids->slots[i]);
    free(ids->slots);
}

static xmlChar *property(xmlNodePtr node, const char *name, int required) {
    xmlChar *value = xmlGetProp(node, BAD_CAST name);
    if (value == NULL && required) {
        fprintf(stderr, "xmlconf catalog: TEST is missing required %s\n", name);
        return NULL;
    }
    return value;
}

static int inside_root(const char *root, const char *path) {
    size_t n = strlen(root);
    if (strncmp(root, path, n) != 0)
        return 0;
    return path[n] == '\0' || (n == 1 && root[0] == '/') || path[n] == '/';
}

/* Normalize a local absolute path without requiring the leaf to exist. */
static char *normalize_absolute(const char *path) {
    char *copy;
    char **parts;
    size_t count = 0, capacity, i, out_len;
    char *cursor;
    char *out;

    if (path == NULL || path[0] != '/')
        return NULL;
    if (strlen(path) > PATH_LIMIT)
        return NULL;
    copy = strdup(path);
    if (copy == NULL)
        return NULL;
    capacity = strlen(path) / 2 + 2;
    if (capacity > PATH_LIMIT / sizeof(*parts)) {
        free(copy);
        return NULL;
    }
    parts = calloc(capacity, sizeof(*parts));
    if (parts == NULL) {
        free(copy);
        return NULL;
    }
    cursor = copy;
    while (*cursor != '\0') {
        char *start;
        while (*cursor == '/')
            cursor++;
        if (*cursor == '\0')
            break;
        start = cursor;
        while (*cursor != '\0' && *cursor != '/')
            cursor++;
        if (*cursor == '/')
            *cursor++ = '\0';
        if (strcmp(start, ".") == 0 || start[0] == '\0')
            continue;
        if (strcmp(start, "..") == 0) {
            if (count == 0) {
                free(parts);
                free(copy);
                return NULL;
            }
            count--;
        } else {
            if (count >= capacity) {
                free(parts);
                free(copy);
                return NULL;
            }
            parts[count++] = start;
        }
    }
    out_len = 2;
    for (i = 0; i < count; i++) {
        size_t len = strlen(parts[i]);
        if (out_len > PATH_LIMIT - len - 1) {
            free(parts);
            free(copy);
            return NULL;
        }
        out_len += len + 1;
    }
    out = malloc(out_len);
    if (out == NULL) {
        free(parts);
        free(copy);
        return NULL;
    }
    out[0] = '/';
    out[1] = '\0';
    for (i = 0; i < count; i++) {
        if (i != 0)
            strcat(out, "/");
        strcat(out, parts[i]);
    }
    free(parts);
    free(copy);
    return out;
}

static char *local_path_from_uri(const xmlChar *uri) {
    xmlURIPtr parsed = NULL;
    char *decoded = NULL;
    char *result = NULL;
    const char *path;
    int decoded_len;

    parsed = xmlParseURI((const char *) uri);
    if (parsed == NULL)
        goto done;
    if (parsed->scheme != NULL && strcmp(parsed->scheme, "file") != 0)
        goto done;
    if (parsed->server != NULL && parsed->server[0] != '\0' &&
        strcmp(parsed->server, "localhost") != 0)
        goto done;
    if (parsed->query != NULL || parsed->fragment != NULL)
        goto done;
    path = parsed->path;
    if (path == NULL || path[0] != '/')
        goto done;
    decoded = xmlURIUnescapeString(path, 0, NULL);
    if (decoded == NULL)
        goto done;
    decoded_len = (int) strlen(decoded);
    if (decoded_len < 0 || (size_t) decoded_len > PATH_LIMIT)
        goto done;
    result = normalize_absolute(decoded);
done:
    xmlFree(decoded);
    if (parsed != NULL)
        xmlFreeURI(parsed);
    return result;
}

static char *resolved_test_path(xmlDocPtr doc, xmlNodePtr node,
                                const char *suite_root, const char *uri) {
    xmlChar *base = NULL;
    xmlChar *resolved = NULL;
    char *absolute = NULL;
    char *relative = NULL;
    char *real = NULL;
    struct stat st;
    size_t root_len;

    base = xmlNodeGetBase(doc, node);
    if (base == NULL)
        goto done;
    resolved = xmlBuildURI(BAD_CAST uri, base);
    if (resolved == NULL)
        goto done;
    absolute = local_path_from_uri(resolved);
    if (absolute == NULL || !inside_root(suite_root, absolute))
        goto done;

    if (lstat(absolute, &st) == 0) {
        real = realpath(absolute, NULL);
        if (real == NULL || !inside_root(suite_root, real))
            goto done;
        free(absolute);
        absolute = real;
        real = NULL;
        if (!S_ISREG(st.st_mode))
            goto done;
    } else if (errno != ENOENT) {
        goto done;
    }
    root_len = strlen(suite_root);
    if (root_len == 1 && suite_root[0] == '/')
        relative = strdup(absolute + 1);
    else if (absolute[root_len] == '/')
        relative = strdup(absolute + root_len + 1);
    else if (absolute[root_len] == '\0')
        relative = strdup(".");
done:
    xmlFree(base);
    xmlFree(resolved);
    free(absolute);
    free(real);
    return relative;
}

static int validate_value(const char *value, const char *field) {
    if (value == NULL)
        return 1;
    if (has_forbidden_tsv_byte(value)) {
        fprintf(stderr, "xmlconf catalog: %s contains a TSV control byte\n", field);
        return 0;
    }
    return 1;
}

static int emit_row(xmlDocPtr doc, xmlNodePtr node, const char *suite_root,
                    IdSet *ids, size_t *count) {
    xmlChar *id = NULL, *type = NULL, *entities = NULL, *recommendation = NULL;
    xmlChar *version = NULL, *edition = NULL, *name_space = NULL, *uri = NULL;
    xmlChar *output = NULL, *output3 = NULL, *sections = NULL;
    char *path = NULL;
    int inserted, ok = 0;

    id = property(node, "ID", 1);
    type = property(node, "TYPE", 1);
    uri = property(node, "URI", 1);
    sections = property(node, "SECTIONS", 1);
    if (id == NULL || type == NULL || uri == NULL || sections == NULL)
        goto done;
    entities = property(node, "ENTITIES", 1);
    recommendation = property(node, "RECOMMENDATION", 1);
    version = property(node, "VERSION", 0);
    edition = property(node, "EDITION", 0);
    name_space = property(node, "NAMESPACE", 1);
    output = property(node, "OUTPUT", 0);
    output3 = property(node, "OUTPUT3", 0);
    if (entities == NULL || recommendation == NULL || name_space == NULL)
        goto done;

    if (!validate_value((const char *) id, "ID") ||
        !validate_value((const char *) type, "TYPE") ||
        !validate_value((const char *) entities, "ENTITIES") ||
        !validate_value((const char *) recommendation, "RECOMMENDATION") ||
        !validate_value((const char *) version, "VERSION") ||
        !validate_value((const char *) edition, "EDITION") ||
        !validate_value((const char *) name_space, "NAMESPACE") ||
        !validate_value((const char *) uri, "URI") ||
        !validate_value((const char *) output, "OUTPUT") ||
        !validate_value((const char *) output3, "OUTPUT3") ||
        !validate_value((const char *) sections, "SECTIONS"))
        goto done;

    inserted = ids_insert(ids, (const char *) id);
    if (inserted < 0) {
        fail("out of memory while checking duplicate IDs");
        goto done;
    }
    if (inserted == 0) {
        fprintf(stderr, "xmlconf catalog: duplicate ID: %s\n", (const char *) id);
        goto done;
    }
    path = resolved_test_path(doc, node, suite_root, (const char *) uri);
    if (path == NULL) {
        fprintf(stderr, "xmlconf catalog: nonlocal, outside-root, or invalid path for ID %s\n",
                (const char *) id);
        goto done;
    }
    if (!validate_value(path, "resolved path"))
        goto done;

    if (printf("%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n",
               (const char *) id, (const char *) type, (const char *) entities,
               (const char *) recommendation,
               version == NULL ? BAD_CAST "" : version,
               edition == NULL ? BAD_CAST "" : edition,
               (const char *) name_space, (const char *) uri, path,
               output == NULL ? BAD_CAST "" : output,
               output3 == NULL ? BAD_CAST "" : output3,
               (const char *) sections) < 0) {
        fail("failed writing TSV row");
        goto done;
    }
    (*count)++;
    ok = 1;
done:
    xmlFree(id);
    xmlFree(type);
    xmlFree(entities);
    xmlFree(recommendation);
    xmlFree(version);
    xmlFree(edition);
    xmlFree(name_space);
    xmlFree(uri);
    xmlFree(output);
    xmlFree(output3);
    xmlFree(sections);
    free(path);
    return ok;
}

static int version_command(void) {
    printf("compiled=%d runtime=%s\n", LIBXML_VERSION,
           xmlParserVersion == NULL ? "unknown" : xmlParserVersion);
    return LIBXML_VERSION == 21504 && xmlParserVersion != NULL &&
           strcmp(xmlParserVersion, "21504") == 0 ? 0 : 1;
}

int main(int argc, char **argv) {
    char catalog_real[PATH_MAX];
    char root_real[PATH_MAX];
    xmlDocPtr doc = NULL;
    xmlXPathContextPtr xpath = NULL;
    xmlXPathObjectPtr nodes = NULL;
    IdSet ids = {0};
    size_t count = 0;
    int i, exit_code = 1;

    if (argc == 2 && strcmp(argv[1], "--version") == 0)
        return version_command();
    if (argc != 3) {
        fail("usage: helper CATALOG.xml ABSOLUTE_SUITE_ROOT | --version");
        return 2;
    }
    if (argv[1][0] != '/' || argv[2][0] != '/' ||
        realpath(argv[1], catalog_real) == NULL ||
        realpath(argv[2], root_real) == NULL) {
        fail("catalog and suite root must be existing absolute paths");
        return 2;
    }
    {
        struct stat root_stat;
        if (stat(root_real, &root_stat) != 0 || !S_ISDIR(root_stat.st_mode)) {
            fail("suite root must be a directory");
            return 2;
        }
    }
    if (!inside_root(root_real, catalog_real)) {
        fail("catalog must be inside suite root");
        return 2;
    }
    if (LIBXML_VERSION != 21504 || xmlParserVersion == NULL ||
        strcmp(xmlParserVersion, "21504") != 0) {
        fail("helper must be compiled and run with libxml2 2.15.4 (21504)");
        return 2;
    }

    xmlInitParser();
    doc = xmlReadFile(catalog_real, NULL,
                      XML_PARSE_NOENT | XML_PARSE_DTDATTR | XML_PARSE_NONET);
    if (doc == NULL) {
        fail("could not parse catalog");
        goto done;
    }
    if (xmlDocGetRootElement(doc) == NULL ||
        xmlStrcmp(xmlDocGetRootElement(doc)->name, BAD_CAST "TESTSUITE") != 0) {
        fail("catalog root must be TESTSUITE");
        goto done;
    }
    xpath = xmlXPathNewContext(doc);
    if (xpath == NULL) {
        fail("could not create XPath context");
        goto done;
    }
    nodes = xmlXPathEvalExpression(BAD_CAST "//TEST", xpath);
    if (nodes == NULL || nodes->type != XPATH_NODESET || nodes->nodesetval == NULL) {
        fail("could not enumerate TEST nodes");
        goto done;
    }
    if (nodes->nodesetval->nodeNr <= 0 || nodes->nodesetval->nodeNr > MAX_CASES) {
        fail("catalog TEST count is empty or exceeds the 10000-case bound");
        goto done;
    }
    if (printf("yaxml-xmlconf-v1\t%d\n", nodes->nodesetval->nodeNr) < 0) {
        fail("failed writing manifest header");
        goto done;
    }
    for (i = 0; i < nodes->nodesetval->nodeNr; i++) {
        xmlNodePtr node = nodes->nodesetval->nodeTab[i];
        if (node == NULL || node->type != XML_ELEMENT_NODE ||
            xmlStrcmp(node->name, BAD_CAST "TEST") != 0) {
            fprintf(stderr, "xmlconf catalog: XPath returned a non-TEST node at index %d\n", i);
            goto done;
        }
        if (!emit_row(doc, node, root_real, &ids, &count))
            goto done;
    }
    if (count != (size_t) nodes->nodesetval->nodeNr) {
        fail("emitted row count differs from catalog TEST count");
        goto done;
    }
    if (fflush(stdout) != 0 || ferror(stdout)) {
        fail("failed flushing manifest");
        goto done;
    }
    exit_code = 0;
done:
    ids_destroy(&ids);
    if (nodes != NULL)
        xmlXPathFreeObject(nodes);
    if (xpath != NULL)
        xmlXPathFreeContext(xpath);
    if (doc != NULL)
        xmlFreeDoc(doc);
    xmlCleanupParser();
    return exit_code;
}
