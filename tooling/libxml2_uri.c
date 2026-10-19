/* Fixed URI oracle only; Cargo never compiles or links this file.
 *
 * Reads tab-separated requests from stdin, one per line, hex-encoded fields:
 *   build A B | parse A | raw A | rel A B | norm A | esc A B | unesc A | canon A
 * and prints one line per request: "ERR", or hex fields ("-" for NULL).
 * parse/raw print: save scheme user server port path query_raw fragment. */
#include <libxml/uri.h>
#include <libxml/xmlmemory.h>
#include <libxml/xmlversion.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static int nibble(int c) {
    return c <= '9' ? c - '0' : (c | 32) - 'a' + 10;
}

static char *unhex(const char *h) {
    size_t n = strlen(h) / 2;
    char *out = malloc(n + 1);
    for (size_t i = 0; i < n; i++)
        out[i] = (char)(nibble(h[2 * i]) << 4 | nibble(h[2 * i + 1]));
    out[n] = 0;
    return out;
}

static void hex(const char *s) {
    if (s == NULL) { fputs("-", stdout); return; }
    if (*s == 0) { fputs("0", stdout); return; }
    for (; *s; s++) printf("%02x", (unsigned char)*s);
}

static void field(const char *s) { fputc('\t', stdout); hex(s); }

static void result(const char *s) {
    if (s == NULL) fputs("ERR", stdout); else { fputs("OK", stdout); field(s); }
    fputc('\n', stdout);
}

static void uri_fields(xmlURI *u) {
    char port[16];
    xmlChar *saved = xmlSaveUri(u);
    fputs("OK", stdout);
    field((char *)saved);
    field(u->scheme);
    field(u->user);
    field(u->server);
    snprintf(port, sizeof port, "%d", u->port);
    field(port);
    field(u->path);
    field(u->query_raw);
    field(u->fragment);
    fputc('\n', stdout);
    xmlFree(saved);
}

int main(void) {
    char *line = NULL;
    size_t cap = 0;
    if (LIBXML_VERSION != 21504) { fputs("wrong libxml2\n", stderr); return 2; }
    while (getline(&line, &cap, stdin) > 0) {
        char *f[4] = {0};
        int n = 0;
        line[strcspn(line, "\n")] = 0;
        for (char *t = strtok(line, "\t"); t && n < 4; t = strtok(NULL, "\t"))
            f[n++] = t;
        char *a = n > 1 ? unhex(f[1]) : NULL, *b = n > 2 ? unhex(f[2]) : NULL;
        const char *op = f[0];
        xmlChar *r = NULL;
        if (!strcmp(op, "build")) { r = xmlBuildURI((xmlChar *)a, (xmlChar *)b); result((char *)r); }
        else if (!strcmp(op, "rel")) { r = xmlBuildRelativeURI((xmlChar *)a, (xmlChar *)b); result((char *)r); }
        else if (!strcmp(op, "esc")) { r = xmlURIEscapeStr((xmlChar *)a, (xmlChar *)b); result((char *)r); }
        else if (!strcmp(op, "canon")) { r = xmlCanonicPath((xmlChar *)a); result((char *)r); }
        else if (!strcmp(op, "unesc")) { r = (xmlChar *)xmlURIUnescapeString(a, 0, NULL); result((char *)r); }
        else if (!strcmp(op, "norm")) { xmlNormalizeURIPath(a); result(a); }
        else if (!strcmp(op, "parse") || !strcmp(op, "raw")) {
            xmlURI *u = xmlParseURIRaw(a, !strcmp(op, "raw"));
            if (u == NULL) result(NULL); else { uri_fields(u); xmlFreeURI(u); }
        }
        else { fputs("bad op\n", stderr); return 2; }
        xmlFree(r);
        free(a); free(b);
    }
    free(line);
    return 0;
}
