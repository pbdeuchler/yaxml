/* XML Schema component oracle; Cargo never compiles or links this.
 *
 * For each schema file argument prints "== file", then one line per global
 * element, attribute and type definition of the compiled xmlSchema (the
 * main schema and every import), anonymous types inline after " :: ", in
 * the format of tests/xsd_components.rs. Lines repeat when a schema is its
 * own import: compare them as a set. Uses xmlschemas.c's private
 * xmlSchemaItemList, xmlSchemaAttributeUse, xmlSchemaImport, xmlSchemaIDC,
 * xmlSchemaIDCSelect and xmlSchemaQNameRef layouts
 * (2.15.4). */
#include <stdio.h>
#include <string.h>
#include <libxml/xmlschemas.h>
#include <libxml/schemasInternals.h>
#include <libxml/hash.h>

typedef struct { void **items; int nbItems; int sizeItems; } List;
typedef struct {
  xmlSchemaTypeType type; void *annot; void *next; xmlSchemaAttributePtr attrDecl;
  int flags; xmlNodePtr node; int occurs; const xmlChar *defValue; void *defVal;
} Use;
typedef struct {
  int type; int flags; const xmlChar *schemaLocation, *orig, *tns; xmlDocPtr doc;
  void *relations; int located, parsed, imported, preserveDoc; void *globals, *locals;
  xmlSchemaPtr schema;
} Import;
typedef struct Select { struct Select *next; void *idc; int index; const xmlChar *xpath; void *comp; } Select;
typedef struct { xmlSchemaTypeType type; void *item; xmlSchemaTypeType itemType; const xmlChar *name, *tns; xmlNodePtr node; } Ref;
typedef struct Idc {
  xmlSchemaTypeType type; void *annot; struct Idc *next; xmlNodePtr node; const xmlChar *name, *tns;
  Select *selector, *fields; int nbFields; Ref *ref;
} Idc;

static void name(const xmlChar *ns, const xmlChar *local) {
  if (!local) { printf("#anon"); return; }
  if (ns) printf("{%s}", ns);
  printf("%s", local);
}
static void tname(xmlSchemaTypePtr t) {
  if (!t) printf("-"); else name(t->targetNamespace, t->name);
}
static void wildcard(xmlSchemaWildcardPtr w) {
  if (!w) return;
  printf(" wildcard=");
  if (w->any) printf("any");
  else if (w->negNsSet) printf("not:%s", w->negNsSet->value ? (char *)w->negNsSet->value : "");
  else for (xmlSchemaWildcardNsPtr n = w->nsSet; n; n = n->next)
    printf("%s%s", n == w->nsSet ? "" : ",", n->value ? (char *)n->value : "");
  printf(" process=%s", w->processContents == 1 ? "skip" : w->processContents == 2 ? "lax" : "strict");
}
static void attributes(List *uses, xmlSchemaWildcardPtr w) {
  for (int i = 0; uses && i < uses->nbItems; i++) {
    Use *u = uses->items[i];
    if (u->type != XML_SCHEMA_TYPE_ATTRIBUTE_USE) continue;
    printf(" use="); name(u->attrDecl->targetNamespace, u->attrDecl->name);
    printf("%s", u->occurs == XML_SCHEMAS_ATTR_USE_REQUIRED ? "!" : "?");
    if (u->defValue) printf("%s%s", u->flags & XML_SCHEMAS_ATTR_FIXED ? "==" : "=", u->defValue);
  }
  wildcard(w);
}
static void group(void *p, void *d, const xmlChar *k) {
  xmlSchemaAttributeGroupPtr g = p;
  printf("attrgroup "); name(g->targetNamespace, g->name);
  attributes(g->attrUses, g->attributeWildcard);
  printf("\n");
}
/* facetSet kinds in enum order, less whitespace and enumerations;
   patterns counted (they are the type's own). */
static void facets(xmlSchemaTypePtr t) {
  static const char *names[] = {"minInclusive", "minExclusive", "maxInclusive", "maxExclusive",
    "totalDigits", "fractionDigits", "pattern", "enumeration", "whiteSpace", "length", "maxLength", "minLength"};
  int patterns = 0;
  printf(" facets=");
  for (int kind = XML_SCHEMA_FACET_MININCLUSIVE; kind <= XML_SCHEMA_FACET_MINLENGTH; kind++)
    for (xmlSchemaFacetLinkPtr l = t->facetSet; l; l = l->next)
      if ((int) l->facet->type == kind) {
        if (kind == XML_SCHEMA_FACET_PATTERN) patterns++;
        else if (kind != XML_SCHEMA_FACET_ENUMERATION && kind != XML_SCHEMA_FACET_WHITESPACE)
          printf("%s,", names[kind - XML_SCHEMA_FACET_MININCLUSIVE]);
      }
  printf(" patterns=%d", patterns);
}
static void type(void *p, void *d, const xmlChar *k);
static void anon(xmlSchemaTypePtr t) { if (t && !t->name) { printf(" :: "); type(t, NULL, (const xmlChar *)""); } }
static void elem(void *p, void *d, const xmlChar *k) {
  xmlSchemaElementPtr e = p; int f = e->flags;
  printf("element "); name(e->targetNamespace, e->name);
  printf(" type="); tname(e->subtypes);
  printf(" nillable=%d abstract=%d", !!(f & XML_SCHEMAS_ELEM_NILLABLE), !!(f & XML_SCHEMAS_ELEM_ABSTRACT));
  if (e->value) printf(" %s=%s", f & XML_SCHEMAS_ELEM_FIXED ? "fixed" : "default", e->value);
  printf(" block=%s%s%s", f & XML_SCHEMAS_ELEM_BLOCK_EXTENSION ? "e" : "",
         f & XML_SCHEMAS_ELEM_BLOCK_RESTRICTION ? "r" : "", f & XML_SCHEMAS_ELEM_BLOCK_SUBSTITUTION ? "s" : "");
  printf(" final=%s%s", f & XML_SCHEMAS_ELEM_FINAL_EXTENSION ? "e" : "", f & XML_SCHEMAS_ELEM_FINAL_RESTRICTION ? "r" : "");
  if (e->substGroup) { printf(" head="); name(e->substGroupNs, e->substGroup); }
  printf(" subst=%d", !!(f & XML_SCHEMAS_ELEM_SUBST_GROUP_HEAD));
  for (Idc *c = e->idcs; c; c = c->next) {
    printf(" %s=", c->type == XML_SCHEMA_TYPE_IDC_KEY ? "key" : c->type == XML_SCHEMA_TYPE_IDC_UNIQUE ? "unique" : "keyref");
    name(c->tns, c->name);
    printf("[%s", c->selector->xpath);
    for (Select *s = c->fields; s; s = s->next) printf("|%s", s->xpath);
    printf("]");
    if (c->ref && c->ref->item) { printf("->"); name(((Idc *)c->ref->item)->tns, ((Idc *)c->ref->item)->name); }
  }
  anon(e->subtypes);
  printf("\n");
}
static void attr(void *p, void *d, const xmlChar *k) {
  xmlSchemaAttributePtr a = p;
  printf("attribute "); name(a->targetNamespace, a->name);
  printf(" type="); tname(a->subtypes);
  if (a->defValue) printf(" %s=%s", a->flags & XML_SCHEMAS_ATTR_FIXED ? "fixed" : "default", a->defValue);
  anon(a->subtypes);
  printf("\n");
}
static void type(void *p, void *d, const xmlChar *k) {
  xmlSchemaTypePtr t = p; int f = t->flags;
  printf("type "); name(t->targetNamespace, t->name);
  printf(" base="); tname(t->baseType);
  printf(" derivation=%s", f & XML_SCHEMAS_TYPE_DERIVATION_METHOD_EXTENSION ? "extension" : "restriction");
  printf(" final=%s%s%s%s", f & XML_SCHEMAS_TYPE_FINAL_EXTENSION ? "e" : "", f & XML_SCHEMAS_TYPE_FINAL_RESTRICTION ? "r" : "",
         f & XML_SCHEMAS_TYPE_FINAL_LIST ? "l" : "", f & XML_SCHEMAS_TYPE_FINAL_UNION ? "u" : "");
  printf(" block=%s%s", f & XML_SCHEMAS_TYPE_BLOCK_EXTENSION ? "e" : "", f & XML_SCHEMAS_TYPE_BLOCK_RESTRICTION ? "r" : "");
  if (t->type == XML_SCHEMA_TYPE_SIMPLE) {
    printf(" ws=%s", f & XML_SCHEMAS_TYPE_WHITESPACE_PRESERVE ? "preserve" : f & XML_SCHEMAS_TYPE_WHITESPACE_REPLACE ? "replace" : f & XML_SCHEMAS_TYPE_WHITESPACE_COLLAPSE ? "collapse" : "-");
    facets(t);
    printf(" simple variety=%s", f & XML_SCHEMAS_TYPE_VARIETY_LIST ? "list" : f & XML_SCHEMAS_TYPE_VARIETY_UNION ? "union" : "atomic");
    if (f & XML_SCHEMAS_TYPE_VARIETY_LIST) { printf(" item="); tname(t->subtypes); }
    if (f & XML_SCHEMAS_TYPE_VARIETY_UNION) {
      printf(" members=");
      for (xmlSchemaTypeLinkPtr l = t->memberTypes; l; l = l->next) { if (l != t->memberTypes) printf(","); tname(l->type); }
    }
  } else {
    static const char *content[] = {"unknown", "empty", "elements", "mixed", "simple", "mixed", "basic", "any"};
    printf(" complex abstract=%d content=%s", !!(f & XML_SCHEMAS_TYPE_ABSTRACT), content[t->contentType]);
    if (t->contentType == XML_SCHEMA_CONTENT_SIMPLE) { printf(" simple="); tname(t->contentTypeDef); }
    attributes(t->attrUses, t->attributeWildcard);
  }
  if (!k || *k) printf("\n");
}
static void nota(void *p, void *d, const xmlChar *k) {
  xmlSchemaNotationPtr n = p;
  printf("notation "); name(n->targetNamespace, n->name); printf("\n");
}
static void scan(xmlSchemaPtr s) {
  xmlHashScan(s->notaDecl, nota, NULL);
  xmlHashScan(s->attrgrpDecl, group, NULL);
  xmlHashScan(s->elemDecl, elem, NULL);
  xmlHashScan(s->attrDecl, attr, NULL);
  xmlHashScan(s->typeDecl, type, NULL);
}
static void import(void *p, void *d, const xmlChar *k) { if (((Import *)p)->schema) scan(((Import *)p)->schema); }

int main(int argc, char **argv) {
  for (int i = 1; i < argc; i++) {
    xmlSchemaParserCtxtPtr c = xmlSchemaNewParserCtxt(argv[i]);
    xmlSchemaSetParserStructuredErrors(c, NULL, NULL);
    xmlSchemaPtr s = xmlSchemaParse(c);
    printf("== %s\n", argv[i]);
    if (s) printf("schema tns=%s\n", s->targetNamespace ? (char *)s->targetNamespace : "-");
    if (s) { scan(s); xmlHashScan(s->schemasImports, import, NULL); xmlSchemaFree(s); }
    xmlSchemaFreeParserCtxt(c);
  }
  return 0;
}
