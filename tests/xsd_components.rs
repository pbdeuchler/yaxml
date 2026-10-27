//! Read-only schema components (`schemasInternals.h`): global element,
//! attribute and type definitions printed in the line format of
//! harness/tooling/libxml2_xsd_components.c, which walks libxml2 2.15.4's
//! compiled `xmlSchema`. With `YAXML_XSD_COMPONENTS_HELPER` and
//! `YAXML_LIBXML2_SOURCE` set, the ignored test compares every schema of
//! libxml2's `test/schemas` that both build.

use std::collections::BTreeSet;
use std::fmt::Write;

use yaxml::xsd::{
    Constraint, ContentType, Derivations, Limits, Namespaces, ProcessContents, Schema,
    SimpleVariety, TypeDefinition, TypeKind,
};

fn qualified(namespace: Option<&str>, name: Option<&str>) -> String {
    match (namespace, name) {
        (_, None) => "#anon".into(),
        (None, Some(name)) => name.into(),
        (Some(ns), Some(name)) => format!("{{{ns}}}{name}"),
    }
}

fn type_name(ty: TypeDefinition<'_>) -> String {
    qualified(ty.namespace(), ty.name())
}

fn flags(set: Derivations) -> String {
    [
        (set.extension, 'e'),
        (set.restriction, 'r'),
        (set.substitution, 's'),
        (set.list, 'l'),
        (set.union, 'u'),
    ]
    .iter()
    .filter_map(|&(on, c)| on.then_some(c))
    .collect()
}

fn value(out: &mut String, value: Option<Constraint<'_>>) {
    if let Some(value) = value {
        let kind = if value.fixed { "fixed" } else { "default" };
        write!(out, " {kind}={}", value.value).unwrap();
    }
}

/// An anonymous type, inline after its declaration.
fn anonymous(out: &mut String, ty: TypeDefinition<'_>) {
    if ty.name().is_none() {
        out.push_str(" :: ");
        definition(out, ty);
    }
}

fn definition(out: &mut String, ty: TypeDefinition<'_>) {
    let derivation = if ty.is_extension() {
        "extension"
    } else {
        "restriction"
    };
    write!(
        out,
        "type {} base={} derivation={derivation} final={} block={}",
        type_name(ty),
        type_name(ty.base()),
        flags(ty.final_()),
        flags(ty.block()),
    )
    .unwrap();
    match ty.kind() {
        TypeKind::Simple(SimpleVariety::Atomic) => out.push_str(" simple variety=atomic"),
        TypeKind::Simple(SimpleVariety::List(item)) => {
            write!(out, " simple variety=list item={}", type_name(item)).unwrap();
        }
        TypeKind::Simple(SimpleVariety::Union(members)) => {
            let members: Vec<String> = members.into_iter().map(type_name).collect();
            write!(out, " simple variety=union members={}", members.join(",")).unwrap();
        }
        TypeKind::Complex(content) => {
            let content = match content {
                ContentType::Empty => "empty".into(),
                ContentType::Elements => "elements".into(),
                ContentType::Mixed => "mixed".into(),
                ContentType::Simple(simple) => format!("simple simple={}", type_name(simple)),
            };
            write!(
                out,
                " complex abstract={} content={content}",
                ty.is_abstract() as u8
            )
            .unwrap();
            for used in ty.attribute_uses() {
                let declaration = used.declaration();
                let name = qualified(declaration.namespace(), Some(declaration.name()));
                let required = if used.is_required() { '!' } else { '?' };
                write!(out, " use={name}{required}").unwrap();
                if let Some(value) = used.value() {
                    let fixed = if value.fixed { "==" } else { "=" };
                    write!(out, "{fixed}{}", value.value).unwrap();
                }
            }
            if let Some(wildcard) = ty.attribute_wildcard() {
                let namespaces = match wildcard.namespaces {
                    Namespaces::Any => "any".into(),
                    Namespaces::Not(ns) => format!("not:{}", ns.unwrap_or_default()),
                    Namespaces::Set(set) => (set.iter().map(|ns| ns.unwrap_or_default()))
                        .collect::<Vec<_>>()
                        .join(","),
                };
                let process = match wildcard.process {
                    ProcessContents::Skip => "skip",
                    ProcessContents::Lax => "lax",
                    ProcessContents::Strict => "strict",
                };
                write!(out, " wildcard={namespaces} process={process}").unwrap();
            }
        }
    }
}

/// One line per global component, sorted (libxml2 scans hash tables).
fn components(schema: &Schema) -> BTreeSet<String> {
    let mut lines = BTreeSet::new();
    for element in schema.elements() {
        let mut out = String::new();
        let ty = element.type_definition();
        write!(
            out,
            "element {} type={} nillable={} abstract={}",
            qualified(element.namespace(), Some(element.name())),
            type_name(ty),
            element.is_nillable() as u8,
            element.is_abstract() as u8,
        )
        .unwrap();
        value(&mut out, element.value());
        write!(out, " block={}", flags(element.block())).unwrap();
        anonymous(&mut out, ty);
        lines.insert(out);
    }
    for attribute in schema.attributes() {
        let mut out = String::new();
        let ty = attribute.type_definition();
        let name = qualified(attribute.namespace(), Some(attribute.name()));
        write!(out, "attribute {name} type={}", type_name(ty)).unwrap();
        value(&mut out, attribute.value());
        anonymous(&mut out, ty);
        lines.insert(out);
    }
    for ty in schema.type_definitions() {
        let mut out = String::new();
        definition(&mut out, ty);
        lines.insert(out);
    }
    lines
}

const SCHEMA: &str = "<xs:schema xmlns:xs='http://www.w3.org/2001/XMLSchema' xmlns:t='urn:t' targetNamespace='urn:t' blockDefault='extension' finalDefault='restriction'>
<xs:element name='e' type='xs:int' default='3' nillable='true'/>
<xs:element name='f' type='xs:string' fixed='x' abstract='true' block='#all'/>
<xs:attribute name='g' fixed='1'><xs:simpleType><xs:list itemType='xs:int'/></xs:simpleType></xs:attribute>
<xs:simpleType name='u' final='list union'><xs:union memberTypes='xs:int xs:boolean'/></xs:simpleType>
<xs:complexType name='b' abstract='true' block='restriction'><xs:attribute name='x' use='required'/><xs:anyAttribute namespace='##other' processContents='skip'/></xs:complexType>
<xs:complexType name='s'><xs:simpleContent><xs:extension base='xs:decimal'><xs:attribute name='y' default='d'/></xs:extension></xs:simpleContent></xs:complexType>
<xs:complexType name='m' mixed='true'><xs:complexContent><xs:extension base='t:b'><xs:sequence><xs:element name='c'/></xs:sequence><xs:attribute ref='t:g'/></xs:extension></xs:complexContent></xs:complexType>
</xs:schema>";

/// Recorded with the helper.
const COMPONENTS: [&str; 7] = [
    "attribute {urn:t}g type=#anon fixed=1 :: type #anon base={http://www.w3.org/2001/XMLSchema}anySimpleType derivation=restriction final= block= simple variety=list item={http://www.w3.org/2001/XMLSchema}int",
    "element {urn:t}e type={http://www.w3.org/2001/XMLSchema}int nillable=1 abstract=0 default=3 block=e",
    "element {urn:t}f type={http://www.w3.org/2001/XMLSchema}string nillable=0 abstract=1 fixed=x block=ers",
    "type {urn:t}b base={http://www.w3.org/2001/XMLSchema}anyType derivation=restriction final=r block=r complex abstract=1 content=empty use=x! wildcard=not:urn:t process=skip",
    "type {urn:t}m base={urn:t}b derivation=extension final=r block=e complex abstract=0 content=mixed use={urn:t}g? use=x! wildcard=not:urn:t process=skip",
    "type {urn:t}s base={http://www.w3.org/2001/XMLSchema}decimal derivation=extension final=r block=e complex abstract=0 content=simple simple={http://www.w3.org/2001/XMLSchema}decimal use=y?",
    "type {urn:t}u base={http://www.w3.org/2001/XMLSchema}anySimpleType derivation=restriction final=lu block= simple variety=union members={http://www.w3.org/2001/XMLSchema}int,{http://www.w3.org/2001/XMLSchema}boolean",
];

#[test]
fn global_components_follow_libxml2() {
    let schema = Schema::parse(SCHEMA.as_bytes(), "a.xsd").unwrap();
    let got: Vec<String> = components(&schema).into_iter().collect();
    assert_eq!(got, COMPONENTS);
}

#[test]
fn lookups_find_globals_and_built_ins_only() {
    let schema = Schema::parse(SCHEMA.as_bytes(), "a.xsd").unwrap();
    let xs = Some("http://www.w3.org/2001/XMLSchema");
    let t = Some("urn:t");
    let cases: [(&str, Option<&str>, &str, bool); 9] = [
        ("element", t, "e", true),
        ("element", None, "e", false),
        ("element", t, "c", false),
        ("attribute", t, "g", true),
        ("attribute", t, "x", false),
        ("type", t, "m", true),
        ("type", xs, "int", true),
        ("type", xs, "anyType", true),
        ("type", t, "nope", false),
    ];
    for (kind, namespace, name, found) in cases {
        let got = match kind {
            "element" => schema
                .element(namespace, name)
                .map(|e| (e.namespace(), e.name())),
            "attribute" => schema
                .attribute(namespace, name)
                .map(|a| (a.namespace(), a.name())),
            _ => (schema.type_definition(namespace, name))
                .map(|t| (t.namespace(), t.name().unwrap())),
        };
        assert_eq!(got, found.then_some((namespace, name)), "{kind} {name}");
    }
    let int = schema.type_definition(xs, "int").unwrap();
    let any = schema.type_definition(xs, "anyType").unwrap();
    assert!(int.is_builtin() && !schema.type_definition(t, "m").unwrap().is_builtin());
    assert_eq!(any.base().name(), Some("anyType"));
    assert!(int.attribute_uses().next().is_none() && int.attribute_wildcard().is_none());
    assert!(schema.type_definitions().all(|ty| !ty.is_builtin()));
    let empty = Schema::parse(
        b"<xs:schema xmlns:xs='http://www.w3.org/2001/XMLSchema'/>",
        "e.xsd",
    );
    assert!(components(&empty.unwrap()).is_empty());
}

/// Restrictions of a union, whose `memberTypes` libxml2 leaves empty.
const RESTRICTED_UNIONS: [&str; 2] = ["type {http://FOO}fooType.B ", "type {http://foo}emType "];

/// libxml2's wildcard namespace list in the view's sorted order.
fn sorted_wildcard(line: &str) -> String {
    let Some((head, rest)) = line.split_once(" wildcard=") else {
        return line.to_owned();
    };
    let (set, tail) = rest.split_once(' ').unwrap();
    let mut items: Vec<&str> = set.split(',').collect();
    if set != "any" && !set.starts_with("not:") {
        items.sort();
    }
    format!("{head} wildcard={} {tail}", items.join(","))
}

fn build(path: &std::path::Path) -> Option<Schema> {
    let text = std::fs::read(path).unwrap();
    let uri = format!("file://{}", path.display());
    let resolver = yaxml::FileResolver::new(path.parent().unwrap()).unwrap();
    Schema::with_resolver(&text, &uri, Some(Box::new(resolver)), Limits::default()).ok()
}

#[test]
#[ignore = "requires YAXML_XSD_COMPONENTS_HELPER (harness/tooling/libxml2_xsd_components.c) and YAXML_LIBXML2_SOURCE"]
fn components_match_libxml2_on_its_schemas() {
    let helper = std::env::var("YAXML_XSD_COMPONENTS_HELPER").unwrap();
    let source = std::env::var("YAXML_LIBXML2_SOURCE").unwrap();
    let directory = std::path::Path::new(&source)
        .join("test/schemas")
        .canonicalize()
        .unwrap();
    let mut paths: Vec<_> = (std::fs::read_dir(&directory).unwrap())
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "xsd"))
        .collect();
    paths.sort();
    let (mut compared, mut failures) = (0, Vec::new());
    for path in paths {
        let output = std::process::Command::new(&helper)
            .arg(&path)
            .output()
            .unwrap();
        let expected: BTreeSet<String> = (String::from_utf8(output.stdout).unwrap().lines())
            .skip(1)
            .map(sorted_wildcard)
            .filter(|line| !line.ends_with("variety=union members="))
            .collect();
        let Some(schema) = build(&path) else { continue };
        if expected.is_empty() {
            continue;
        }
        compared += 1;
        let got: BTreeSet<String> = (components(&schema).into_iter())
            .filter(|line| !RESTRICTED_UNIONS.iter().any(|name| line.starts_with(name)))
            .collect();
        if got != expected {
            let name = path.file_name().unwrap().to_string_lossy();
            let missing: Vec<_> = expected.difference(&got).collect();
            let extra: Vec<_> = got.difference(&expected).collect();
            failures.push(format!(
                "{name}\n  libxml2 {missing:?}\n  yaxml   {extra:?}"
            ));
        }
    }
    assert!(compared > 100, "compared {compared}");
    assert!(
        failures.is_empty(),
        "{} of {compared}:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
