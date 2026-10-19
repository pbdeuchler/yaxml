/* Regexp and XML Schema datatype oracle; Cargo never compiles or links this.
 *
 * Reads tab-separated requests from stdin, one per line, hex-encoded fields
 * ("0" is the empty string):
 *   re P V          xmlRegexpCompile(P), then xmlRegexpExec(V)
 *   val T V         xmlSchemaValidatePredefinedType(T, V)
 *   cmp T V T2 V2   xmlSchemaCompareValues of both computed values
 *   valn, cmpn      the same in the context of the <c> element of DOCUMENT
 *   facet T K F V   facet K with value F on built-in T, checked against V
 * and prints one line per request: the integer result, or "ERR" when a
 * pattern, facet or operand value is rejected first. */
#include <libxml/parser.h>
#include <libxml/xmlregexp.h>
#include <libxml/xmlschemastypes.h>
#include <libxml/schemasInternals.h>
#include <libxml/xmlmemory.h>
#include <libxml/xmlversion.h>
#include <libxml/xmlerror.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static const char DOCUMENT[] =
    "<!DOCTYPE r [<!NOTATION gif SYSTEM 'gif'><!NOTATION p:n SYSTEM 'n'>"
    "<!ENTITY pic SYSTEM 'pic.gif' NDATA gif><!ENTITY txt 'text'>]>"
    "<r xmlns='urn:default' xmlns:p='urn:p'><c/></r>";
static xmlNodePtr node = NULL;

static void quiet(void *ctx, const xmlError *error) { (void)ctx; (void)error; }

static int nibble(int c) {
    return c <= '9' ? c - '0' : (c | 32) - 'a' + 10;
}

static char *unhex(const char *h) {
    size_t n = strcmp(h, "0") ? strlen(h) / 2 : 0;
    char *out = malloc(n + 1);
    for (size_t i = 0; i < n; i++)
        out[i] = (char)(nibble(h[2 * i]) << 4 | nibble(h[2 * i + 1]));
    out[n] = 0;
    return out;
}

static const struct { const char *name; xmlSchemaTypeType type; } FACETS[] = {
    {"length", XML_SCHEMA_FACET_LENGTH},
    {"minLength", XML_SCHEMA_FACET_MINLENGTH},
    {"maxLength", XML_SCHEMA_FACET_MAXLENGTH},
    {"pattern", XML_SCHEMA_FACET_PATTERN},
    {"enumeration", XML_SCHEMA_FACET_ENUMERATION},
    {"whiteSpace", XML_SCHEMA_FACET_WHITESPACE},
    {"minInclusive", XML_SCHEMA_FACET_MININCLUSIVE},
    {"maxInclusive", XML_SCHEMA_FACET_MAXINCLUSIVE},
    {"minExclusive", XML_SCHEMA_FACET_MINEXCLUSIVE},
    {"maxExclusive", XML_SCHEMA_FACET_MAXEXCLUSIVE},
    {"totalDigits", XML_SCHEMA_FACET_TOTALDIGITS},
    {"fractionDigits", XML_SCHEMA_FACET_FRACTIONDIGITS},
};

static xmlSchemaTypePtr type_named(const char *name) {
    return xmlSchemaGetPredefinedType((const xmlChar *)name,
        (const xmlChar *)"http://www.w3.org/2001/XMLSchema");
}

/* The value of V as T; string and anySimpleType values, which
 * xmlSchemaValidatePredefinedType leaves uncomputed, come from the variant
 * that creates them. */
static int value_of(xmlSchemaTypePtr t, const char *v, xmlSchemaValPtr *out) {
    int ret = xmlSchemaValPredefTypeNode(t, (const xmlChar *)v, out, node);
    if (ret == 0 && *out == NULL)
        ret = xmlSchemaValPredefTypeNodeNoNorm(t, (const xmlChar *)v, out, node);
    return ret;
}

static long items(const char *v) {
    long n = 0;
    for (const char *c = v; *c; c++)
        if (!strchr(" \t\r\n", *c) && (c == v || strchr(" \t\r\n", c[-1])))
            n++;
    return n;
}

static int facet(const char *tn, const char *kind, const char *fv, const char *v) {
    xmlSchemaTypePtr t = type_named(tn);
    xmlSchemaFacetPtr f = xmlSchemaNewFacet();
    xmlSchemaValPtr val = NULL;
    xmlChar *norm = NULL;
    unsigned long len = 0;
    int ret = -100;
    for (size_t i = 0; i < sizeof FACETS / sizeof FACETS[0]; i++)
        if (!strcmp(kind, FACETS[i].name))
            f->type = FACETS[i].type;
    f->value = (const xmlChar *)fv;
    if (t == NULL || xmlSchemaCheckFacet(f, t, NULL, NULL) != 0 ||
        value_of(t, v, &val) != 0) {
        puts("ERR");
        goto done;
    }
    xmlSchemaWhitespaceValueType ws = XML_SCHEMA_WHITESPACE_COLLAPSE;
    if (t->builtInType == XML_SCHEMAS_STRING)
        ws = XML_SCHEMA_WHITESPACE_PRESERVE, norm = xmlStrdup((const xmlChar *)v);
    else if (t->builtInType == XML_SCHEMAS_NORMSTRING)
        ws = XML_SCHEMA_WHITESPACE_REPLACE, norm = xmlSchemaWhiteSpaceReplace((const xmlChar *)v);
    else
        norm = xmlSchemaCollapseString((const xmlChar *)v);
    if (norm == NULL)
        norm = xmlStrdup((const xmlChar *)v);
    int list = t->builtInType == XML_SCHEMAS_NMTOKENS ||
        t->builtInType == XML_SCHEMAS_IDREFS || t->builtInType == XML_SCHEMAS_ENTITIES;
    switch (f->type) {
    case XML_SCHEMA_FACET_LENGTH:
    case XML_SCHEMA_FACET_MINLENGTH:
    case XML_SCHEMA_FACET_MAXLENGTH:
        ret = list ? xmlSchemaValidateListSimpleTypeFacet(f, norm, items(v), NULL)
                   : xmlSchemaValidateLengthFacetWhtsp(f, xmlSchemaGetValType(val),
                         norm, val, &len, ws);
        break;
    default:
        ret = xmlSchemaValidateFacetWhtsp(f, ws, xmlSchemaGetValType(val), norm, val, ws);
    }
    printf("%d\n", ret);
done:
    xmlSchemaFreeValue(val);
    xmlFree(norm);
    xmlSchemaFreeFacet(f);
    return 0;
}

int main(void) {
    char *line = NULL;
    size_t cap = 0;
    if (LIBXML_VERSION != 21504) { fputs("wrong libxml2\n", stderr); return 2; }
    xmlSetStructuredErrorFunc(NULL, quiet);
    xmlSchemaInitTypes();
    xmlDocPtr doc = xmlReadMemory(DOCUMENT, sizeof DOCUMENT - 1, "doc.xml", NULL, 0);
    xmlNodePtr context = xmlDocGetRootElement(doc)->children;
    while (getline(&line, &cap, stdin) > 0) {
        char *f[5] = {0}, *a[5] = {0};
        int n = 0;
        line[strcspn(line, "\n")] = 0;
        for (char *t = strtok(line, "\t"); t && n < 5; t = strtok(NULL, "\t"))
            f[n++] = t;
        for (int i = 1; i < n; i++)
            a[i] = unhex(f[i]);
        size_t op = strlen(f[0]);
        node = op > 1 && f[0][op - 1] == 'n' ? context : NULL;
        if (node != NULL)
            f[0][op - 1] = 0;
        if (!strcmp(f[0], "re") && n == 3) {
            xmlRegexpPtr re = xmlRegexpCompile((const xmlChar *)a[1]);
            if (re == NULL) puts("ERR");
            else printf("%d\n", xmlRegexpExec(re, (const xmlChar *)a[2]));
            xmlRegFreeRegexp(re);
        } else if (!strcmp(f[0], "val") && n == 3) {
            xmlSchemaTypePtr t = type_named(a[1]);
            xmlSchemaValPtr v = NULL;
            printf("%d\n", t ? xmlSchemaValPredefTypeNode(t, (const xmlChar *)a[2], &v, node) : -100);
            xmlSchemaFreeValue(v);
        } else if (!strcmp(f[0], "cmp") && n == 5) {
            xmlSchemaTypePtr t1 = type_named(a[1]), t2 = type_named(a[3]);
            xmlSchemaValPtr v1 = NULL, v2 = NULL;
            if (t1 == NULL || t2 == NULL || value_of(t1, a[2], &v1) || value_of(t2, a[4], &v2))
                puts("ERR");
            else
                printf("%d\n", xmlSchemaCompareValues(v1, v2));
            xmlSchemaFreeValue(v1);
            xmlSchemaFreeValue(v2);
        } else if (!strcmp(f[0], "facet") && n == 5) {
            facet(a[1], a[2], a[3], a[4]);
        } else {
            fputs("bad request\n", stderr);
            return 2;
        }
        for (int i = 1; i < n; i++)
            free(a[i]);
    }
    free(line);
    xmlFreeDoc(doc);
    return 0;
}
