/* Fixed catalog oracle only; Cargo never compiles or links this file.
 *
 * Reads tab-separated requests from stdin, one per line, hex-encoded fields
 * ("-" is NULL, "0" the empty string):
 *   load PATH | res PUBLIC SYSTEM | uri URI
 * load replaces the catalog with xmlLoadACatalog(PATH) after clearing the
 * global file cache, and prints OK or ERR. res and uri print the hex result
 * of xmlACatalogResolve / xmlACatalogResolveURI, or "-". Relative paths
 * resolve against the working directory. */
/* The per-catalog (xmlACatalog*) API is deprecated in 2.15 but is the only
 * one that resolves against a given catalog. */
#pragma GCC diagnostic ignored "-Wdeprecated-declarations"
#include <libxml/catalog.h>
#include <libxml/xmlerror.h>
#include <libxml/xmlmemory.h>
#include <libxml/xmlversion.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static int nibble(int c) {
    return c <= '9' ? c - '0' : (c | 32) - 'a' + 10;
}

static char *unhex(const char *h) {
    if (h == NULL || !strcmp(h, "-")) return NULL;
    size_t n = strlen(h) / 2;
    char *out = malloc(n + 1);
    for (size_t i = 0; i < n; i++)
        out[i] = (char)(nibble(h[2 * i]) << 4 | nibble(h[2 * i + 1]));
    out[n] = 0;
    return out;
}

static void answer(xmlChar *s) {
    if (s == NULL) fputs("-", stdout);
    else if (*s == 0) fputs("0", stdout);
    else for (xmlChar *p = s; *p; p++) printf("%02x", *p);
    fputc('\n', stdout);
    xmlFree(s);
}

static void quiet(void *ctx, const xmlError *error) { (void)ctx; (void)error; }

int main(void) {
    char *line = NULL;
    size_t cap = 0;
    xmlCatalogPtr catalog = NULL;
    if (LIBXML_VERSION != 21504) { fputs("wrong libxml2\n", stderr); return 2; }
    xmlSetStructuredErrorFunc(NULL, quiet);
    while (getline(&line, &cap, stdin) > 0) {
        char *f[3] = {0};
        int n = 0;
        line[strcspn(line, "\n")] = 0;
        for (char *t = strtok(line, "\t"); t && n < 3; t = strtok(NULL, "\t"))
            f[n++] = t;
        char *a = n > 1 ? unhex(f[1]) : NULL, *b = n > 2 ? unhex(f[2]) : NULL;
        if (!strcmp(f[0], "load")) {
            if (catalog != NULL) xmlFreeCatalog(catalog);
            xmlCatalogCleanup();
            catalog = xmlLoadACatalog(a);
            puts(catalog == NULL ? "ERR" : "OK");
        } else if (!strcmp(f[0], "res")) {
            answer(catalog ? xmlACatalogResolve(catalog, (xmlChar *)a, (xmlChar *)b) : NULL);
        } else if (!strcmp(f[0], "uri")) {
            answer(catalog ? xmlACatalogResolveURI(catalog, (xmlChar *)a) : NULL);
        } else { fputs("bad op\n", stderr); return 2; }
        fflush(stdout);
        free(a); free(b);
    }
    free(line);
    return 0;
}
