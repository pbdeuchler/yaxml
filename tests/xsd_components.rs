//! Read-only schema components (`schemasInternals.h`): global element,
//! attribute and type definitions printed in the line format of
//! harness/tooling/libxml2_xsd_components.c, which walks libxml2 2.15.4's
//! compiled `xmlSchema`. With `YAXML_XSD_COMPONENTS_HELPER` and
//! `YAXML_LIBXML2_SOURCE` set, the ignored test compares every schema of
//! libxml2's `test/schemas` that both build.

use std::collections::BTreeSet;
use std::fmt::Write;

use yaxml::OwnedNode;
use yaxml::schema_types::Facet;
use yaxml::xsd::{
    AttributeUse, Compositor, Constraint, ContentType, Derivations, IdcKind, Limits, ModelGroup,
    Namespaces, Particle, ProcessContents, Schema, SimpleVariety, Term, TypeDefinition, TypeKind,
    Wildcard,
};

fn qualified(namespace: Option<&str>, name: Option<&str>) -> String {
    match (namespace, name) {
        (_, None) => "#anon".into(),
        (None, Some(name)) => name.into(),
        (Some(ns), Some(name)) => format!("{{{ns}}}{name}"),
    }
}

/// ` note=[text]..`: each annotation's content, whitespace runs collapsed.
fn note<'a>(out: &mut String, notes: impl Iterator<Item = OwnedNode<'a>>) {
    for (i, annotation) in notes.enumerate() {
        let text = annotation.content().unwrap_or_default();
        let words: Vec<&str> = text.split_ascii_whitespace().collect();
        let gap = if i == 0 { " note=" } else { "" };
        write!(out, "{gap}[{}]", words.join(" ")).unwrap();
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

/// libxml2's facet kind order (`XML_SCHEMA_FACET_*`).
const ORDER: [&str; 9] = [
    "minInclusive",
    "minExclusive",
    "maxInclusive",
    "maxExclusive",
    "totalDigits",
    "fractionDigits",
    "length",
    "maxLength",
    "minLength",
];

fn attributes<'a>(
    out: &mut String,
    uses: impl Iterator<Item = AttributeUse<'a>>,
    wildcard: Option<Wildcard<'a>>,
) {
    for used in uses {
        let declaration = used.declaration();
        let name = qualified(declaration.namespace(), Some(declaration.name()));
        let required = if used.is_required() { '!' } else { '?' };
        write!(out, " use={name}{required}").unwrap();
        if let Some(value) = used.value() {
            let fixed = if value.fixed { "==" } else { "=" };
            write!(out, "{fixed}{}", value.value).unwrap();
        }
        note(out, used.annotations());
    }
    if let Some(wildcard) = wildcard {
        write_wildcard(out, wildcard);
    }
}

fn write_wildcard(out: &mut String, wildcard: Wildcard<'_>) {
    let namespaces = match &wildcard.namespaces {
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
    note(out, wildcard.annotations());
}

fn definition(out: &mut String, ty: TypeDefinition<'_>) {
    let derivation = if ty.is_extension() {
        "extension"
    } else {
        "restriction"
    };
    write!(out, "type {}", type_name(ty)).unwrap();
    note(out, ty.annotations());
    write!(
        out,
        " base={} derivation={derivation} final={} block={}",
        type_name(ty.base()),
        flags(ty.final_()),
        flags(ty.block()),
    )
    .unwrap();
    if let TypeKind::Simple(_) = ty.kind() {
        let whitespace = ty.whitespace();
        let whitespace = whitespace.map_or("-".into(), |ws| format!("{ws:?}").to_lowercase());
        let mut facets: Vec<&Facet> = (ty.facets().iter())
            .filter(|facet| !matches!(facet, Facet::WhiteSpace(_)))
            .collect();
        facets.sort_by_key(|facet| ORDER.iter().position(|&name| name == facet.name()));
        let facets: String = facets
            .iter()
            .map(|facet| format!("{},", facet.name()))
            .collect();
        let patterns = ty.patterns().len();
        write!(out, " ws={whitespace} facets={facets} patterns={patterns}").unwrap();
    }
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
            let model = matches!(content, ContentType::Elements | ContentType::Mixed);
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
            if let Some(content) = ty.particle().filter(|_| model) {
                out.push_str(" particle=");
                particle(out, content);
            }
            attributes(out, ty.attribute_uses(), ty.attribute_wildcard());
        }
    }
}

/// `min..max:term`, groups as `seq(..)`, `choice(..)`, `all(..)`.
fn particle(out: &mut String, particle: Particle<'_>) {
    let max = particle
        .max_occurs()
        .map_or("*".into(), |max| max.to_string());
    write!(out, "{}..{max}", particle.min_occurs()).unwrap();
    note(out, particle.annotations());
    out.push(':');
    match particle.term() {
        None => out.push('-'),
        Some(Term::Element(e)) => out.push_str(&qualified(e.namespace(), Some(e.name()))),
        Some(Term::Wildcard(wildcard)) => {
            out.push_str("any(");
            write_wildcard(out, wildcard);
            out.push(')');
        }
        Some(Term::Group(group)) => model(out, Some(group)),
    }
}

fn model(out: &mut String, group: Option<ModelGroup<'_>>) {
    let Some(group) = group else {
        return out.push('-');
    };
    let compositor = match group.compositor() {
        Compositor::Sequence => "seq",
        Compositor::Choice => "choice",
        Compositor::All => "all",
    };
    out.push_str(compositor);
    note(out, group.annotations());
    out.push('(');
    for (i, child) in group.particles().enumerate() {
        if i > 0 {
            out.push(',');
        }
        particle(out, child);
    }
    out.push(')');
}

/// One line per global component, sorted (libxml2 scans hash tables).
fn components(schema: &Schema) -> BTreeSet<String> {
    let mut out = format!("schema tns={}", schema.target_namespace().unwrap_or("-"));
    note(&mut out, schema.annotation().into_iter());
    let mut lines = BTreeSet::from([out]);
    for element in schema.elements() {
        let mut out = String::new();
        let ty = element.type_definition();
        let name = qualified(element.namespace(), Some(element.name()));
        write!(out, "element {name}").unwrap();
        note(&mut out, element.annotations());
        write!(
            out,
            " type={} nillable={} abstract={}",
            type_name(ty),
            element.is_nillable() as u8,
            element.is_abstract() as u8,
        )
        .unwrap();
        value(&mut out, element.value());
        write!(out, " block={}", flags(element.block())).unwrap();
        write!(out, " final={}", flags(element.final_())).unwrap();
        if let Some(head) = element.substitution_group() {
            write!(
                out,
                " head={}",
                qualified(head.namespace(), Some(head.name()))
            )
            .unwrap();
        }
        let head = element.substitutes().next().is_some();
        write!(out, " subst={}", head as u8).unwrap();
        for idc in element.identity_constraints() {
            let kind = match idc.kind() {
                IdcKind::Key => "key",
                IdcKind::Unique => "unique",
                IdcKind::KeyRef => "keyref",
            };
            let name = qualified(idc.namespace(), Some(idc.name()));
            write!(out, " {kind}={name}[{}", idc.selector()).unwrap();
            idc.fields()
                .for_each(|field| write!(out, "|{field}").unwrap());
            out.push(']');
            note(&mut out, idc.annotations());
            if let Some(key) = idc.refer() {
                write!(out, "->{}", qualified(key.namespace(), Some(key.name()))).unwrap();
            }
        }
        anonymous(&mut out, ty);
        lines.insert(out);
    }
    for attribute in schema.attributes() {
        let mut out = String::new();
        let ty = attribute.type_definition();
        let name = qualified(attribute.namespace(), Some(attribute.name()));
        write!(out, "attribute {name}").unwrap();
        note(&mut out, attribute.annotations());
        write!(out, " type={}", type_name(ty)).unwrap();
        value(&mut out, attribute.value());
        anonymous(&mut out, ty);
        lines.insert(out);
    }
    for ty in schema.type_definitions() {
        let mut out = String::new();
        definition(&mut out, ty);
        lines.insert(out);
    }
    for group in schema.attribute_groups() {
        let mut out = String::new();
        let name = qualified(group.namespace(), Some(group.name()));
        write!(out, "attrgroup {name}").unwrap();
        note(&mut out, group.annotations());
        attributes(&mut out, group.attribute_uses(), group.attribute_wildcard());
        lines.insert(out);
    }
    for group in schema.model_groups() {
        let name = qualified(group.namespace(), Some(group.name()));
        let mut out = format!("group {name}");
        note(&mut out, group.annotations());
        out.push_str(" model=");
        model(&mut out, group.model_group());
        lines.insert(out);
    }
    for (namespace, name) in schema.notations() {
        lines.insert(format!("notation {}", qualified(namespace, Some(name))));
    }
    lines
}

const SCHEMA: &str = "<xs:schema xmlns:xs='http://www.w3.org/2001/XMLSchema' xmlns:t='urn:t' targetNamespace='urn:t' blockDefault='extension' finalDefault='restriction'>
<xs:annotation><xs:documentation>Top
  level</xs:documentation></xs:annotation>
<xs:attributeGroup name='ag'><xs:annotation><xs:documentation>ag</xs:documentation></xs:annotation><xs:attribute name='z' type='xs:int' default='1'><xs:annotation><xs:documentation>z use</xs:documentation></xs:annotation></xs:attribute><xs:attributeGroup ref='t:ah'><xs:annotation><xs:documentation>ah ref</xs:documentation></xs:annotation></xs:attributeGroup><xs:anyAttribute namespace='##local urn:t'><xs:annotation><xs:documentation>ag any</xs:documentation></xs:annotation></xs:anyAttribute></xs:attributeGroup>
<xs:attributeGroup name='ah'><xs:attribute ref='t:g' use='required'/><xs:anyAttribute namespace='##any' processContents='lax'/></xs:attributeGroup>
<xs:element name='h' type='xs:decimal' final='extension'><xs:annotation><xs:documentation>h</xs:documentation></xs:annotation></xs:element>
<xs:annotation><xs:documentation>second top</xs:documentation></xs:annotation>
<xs:element name='i' type='xs:int' substitutionGroup='t:h'/>
<xs:element name='j' type='xs:int' substitutionGroup='t:i' abstract='true'/>
<xs:notation name='n' public='p' system='s'/>
<xs:notation name='a' public='q'/>
<xs:element name='e' type='xs:int' default='3' nillable='true'/>
<xs:group name='g1'><xs:annotation><xs:documentation>g1</xs:documentation></xs:annotation><xs:choice><xs:annotation><xs:documentation>choice</xs:documentation></xs:annotation><xs:element name='a'><xs:annotation><xs:documentation>local a</xs:documentation></xs:annotation></xs:element><xs:any namespace='##other' minOccurs='0'><xs:annotation><xs:documentation>any</xs:documentation></xs:annotation></xs:any></xs:choice></xs:group>
<xs:group name='g2'><xs:sequence><xs:group ref='t:g1' maxOccurs='3'><xs:annotation><xs:documentation>g1 ref</xs:documentation></xs:annotation></xs:group><xs:element ref='t:e'><xs:annotation><xs:documentation>e ref</xs:documentation></xs:annotation></xs:element></xs:sequence></xs:group>
<xs:complexType name='c1'><xs:group ref='t:g2'/></xs:complexType>
<xs:complexType name='c2'><xs:annotation><xs:documentation>c2</xs:documentation></xs:annotation><xs:complexContent><xs:annotation><xs:documentation>cc</xs:documentation></xs:annotation><xs:extension base='t:c1'><xs:annotation><xs:documentation>ext</xs:documentation></xs:annotation><xs:sequence minOccurs='0'><xs:element name='z' maxOccurs='unbounded'/></xs:sequence></xs:extension></xs:complexContent></xs:complexType>
<xs:complexType name='o1' mixed='true'><xs:sequence><xs:element name='q' minOccurs='0' maxOccurs='0'/><xs:sequence/><xs:choice minOccurs='0'/></xs:sequence></xs:complexType>
<xs:complexType name='a1'><xs:all><xs:element name='x'/><xs:element name='y' minOccurs='0'/></xs:all></xs:complexType>
<xs:element name='k'><xs:complexType><xs:sequence><xs:element name='i' maxOccurs='unbounded'/></xs:sequence></xs:complexType>
<xs:key name='pk'><xs:annotation><xs:documentation>pk</xs:documentation></xs:annotation><xs:selector xpath='t:i'><xs:annotation><xs:documentation>sel</xs:documentation></xs:annotation></xs:selector><xs:field xpath='@id'><xs:annotation><xs:documentation>field</xs:documentation></xs:annotation></xs:field><xs:field xpath='.'/></xs:key>
<xs:keyref name='fk' refer='t:pk'><xs:selector xpath='.//t:i'/><xs:field xpath='@ref'/><xs:field xpath='@r2'/></xs:keyref>
<xs:unique name='u'><xs:selector xpath='t:i | t:j'/><xs:field xpath='@n'/></xs:unique></xs:element>
<xs:element name='f' type='xs:string' fixed='x' abstract='true' block='#all'/>
<xs:attribute name='g' fixed='1'><xs:annotation><xs:documentation>g</xs:documentation></xs:annotation><xs:simpleType><xs:annotation><xs:documentation>anon</xs:documentation></xs:annotation><xs:list itemType='xs:int'><xs:annotation><xs:documentation>list</xs:documentation></xs:annotation></xs:list></xs:simpleType></xs:attribute>
<xs:simpleType name='r'><xs:restriction base='t:d'><xs:annotation><xs:documentation>r restriction</xs:documentation></xs:annotation><xs:maxInclusive value='9'/><xs:pattern value='[0-9]+'/><xs:pattern value='x'/></xs:restriction></xs:simpleType>
<xs:simpleType name='d' final=''><xs:restriction base='xs:decimal'><xs:totalDigits value='3'/><xs:minExclusive value='0'/><xs:pattern value='.'/><xs:whiteSpace value='collapse' fixed='true'/></xs:restriction></xs:simpleType>
<xs:simpleType name='u' final='list union'><xs:union memberTypes='xs:int xs:boolean'/></xs:simpleType>
<xs:complexType name='b' abstract='true' block='restriction'><xs:attribute name='x' use='required'><xs:annotation><xs:documentation>x use</xs:documentation></xs:annotation></xs:attribute><xs:anyAttribute namespace='##other' processContents='skip'/></xs:complexType>
<xs:complexType name='s'><xs:simpleContent><xs:extension base='xs:decimal'><xs:attribute name='y' default='d'/></xs:extension></xs:simpleContent></xs:complexType>
<xs:complexType name='m' mixed='true'><xs:complexContent><xs:extension base='t:b'><xs:sequence><xs:element name='c'/></xs:sequence><xs:attribute ref='t:g'/></xs:extension></xs:complexContent></xs:complexType>
</xs:schema>";

/// Recorded with the helper.
const COMPONENTS: [&str; 24] = [
    "attrgroup {urn:t}ag note=[ag] use=z? note=[z use] use={urn:t}g! wildcard=,urn:t process=strict note=[ag any]",
    "attrgroup {urn:t}ah use={urn:t}g! wildcard=any process=lax",
    "attribute {urn:t}g note=[g] type=#anon fixed=1 :: type #anon note=[anon][list] base={http://www.w3.org/2001/XMLSchema}anySimpleType derivation=restriction final= block= ws=collapse facets= patterns=0 simple variety=list item={http://www.w3.org/2001/XMLSchema}int",
    "element {urn:t}e type={http://www.w3.org/2001/XMLSchema}int nillable=1 abstract=0 default=3 block=e final=r subst=0",
    "element {urn:t}f type={http://www.w3.org/2001/XMLSchema}string nillable=0 abstract=1 fixed=x block=ers final=r subst=0",
    "element {urn:t}h note=[h] type={http://www.w3.org/2001/XMLSchema}decimal nillable=0 abstract=0 block=e final=e subst=1",
    "element {urn:t}i type={http://www.w3.org/2001/XMLSchema}int nillable=0 abstract=0 block=e final=r head={urn:t}h subst=0",
    "element {urn:t}j type={http://www.w3.org/2001/XMLSchema}int nillable=0 abstract=1 block=e final=r head={urn:t}i subst=0",
    "element {urn:t}k type=#anon nillable=0 abstract=0 block=e final=r subst=0 key={urn:t}pk[t:i|@id|.] note=[pk][sel][field] keyref={urn:t}fk[.//t:i|@ref|@r2]->{urn:t}pk unique={urn:t}u[t:i | t:j|@n] :: type #anon base={http://www.w3.org/2001/XMLSchema}anyType derivation=restriction final=r block=e complex abstract=0 content=elements particle=1..1:seq(1..*:i)",
    "group {urn:t}g1 note=[g1] model=choice note=[choice](1..1:a,0..1 note=[any]:any( wildcard=not:urn:t process=strict))",
    "group {urn:t}g2 model=seq(1..3 note=[g1 ref]:choice note=[choice](1..1:a,0..1 note=[any]:any( wildcard=not:urn:t process=strict)),1..1 note=[e ref]:{urn:t}e)",
    "notation {urn:t}a",
    "notation {urn:t}n",
    "schema tns=urn:t note=[Top level]",
    "type {urn:t}a1 base={http://www.w3.org/2001/XMLSchema}anyType derivation=restriction final=r block=e complex abstract=0 content=elements particle=1..1:all(1..1:x,0..1:y)",
    "type {urn:t}b base={http://www.w3.org/2001/XMLSchema}anyType derivation=restriction final=r block=r complex abstract=1 content=empty use=x! note=[x use] wildcard=not:urn:t process=skip",
    "type {urn:t}c1 base={http://www.w3.org/2001/XMLSchema}anyType derivation=restriction final=r block=e complex abstract=0 content=elements particle=1..1:seq(1..3 note=[g1 ref]:choice note=[choice](1..1:a,0..1 note=[any]:any( wildcard=not:urn:t process=strict)),1..1 note=[e ref]:{urn:t}e)",
    "type {urn:t}c2 note=[c2][cc][ext] base={urn:t}c1 derivation=extension final=r block=e complex abstract=0 content=elements particle=1..1:seq(1..1:seq(1..3 note=[g1 ref]:choice note=[choice](1..1:a,0..1 note=[any]:any( wildcard=not:urn:t process=strict)),1..1 note=[e ref]:{urn:t}e),0..1:seq(1..*:z))",
    "type {urn:t}d base={http://www.w3.org/2001/XMLSchema}decimal derivation=restriction final= block= ws=collapse facets=minExclusive,totalDigits, patterns=1 simple variety=atomic",
    "type {urn:t}m base={urn:t}b derivation=extension final=r block=e complex abstract=0 content=mixed particle=1..1:seq(1..1:c) use={urn:t}g? use=x! note=[x use] wildcard=not:urn:t process=skip",
    "type {urn:t}o1 base={http://www.w3.org/2001/XMLSchema}anyType derivation=restriction final=r block=e complex abstract=0 content=mixed particle=1..1:seq(0..0:q,1..1:seq(),0..1:choice())",
    "type {urn:t}r note=[r restriction] base={urn:t}d derivation=restriction final=r block= ws=collapse facets=minExclusive,maxInclusive,totalDigits, patterns=2 simple variety=atomic",
    "type {urn:t}s base={http://www.w3.org/2001/XMLSchema}decimal derivation=extension final=r block=e complex abstract=0 content=simple simple={http://www.w3.org/2001/XMLSchema}decimal use=y?",
    "type {urn:t}u base={http://www.w3.org/2001/XMLSchema}anySimpleType derivation=restriction final=lu block= ws=- facets= patterns=0 simple variety=union members={http://www.w3.org/2001/XMLSchema}int,{http://www.w3.org/2001/XMLSchema}boolean",
];

#[test]
fn global_components_follow_libxml2() {
    let schema = Schema::parse(SCHEMA.as_bytes(), "a.xsd").unwrap();
    let got: Vec<String> = components(&schema).into_iter().collect();
    assert_eq!(got, COMPONENTS);
}

/// (annotations, nodes in the first one after libxml2's cleanup).
fn summary<'a>(notes: impl Iterator<Item = OwnedNode<'a>>) -> (usize, usize) {
    let notes: Vec<_> = notes.collect();
    (
        notes.len(),
        notes.first().map_or(0, |n| n.descendants().count()),
    )
}

#[test]
fn annotations_belong_to_libxml2_owners() {
    let source = "<xs:schema xmlns:xs='http://www.w3.org/2001/XMLSchema' targetNamespace='urn:t'>
<xs:annotation><xs:documentation xml:space='preserve'> <b> </b><!--c--><?p?></xs:documentation></xs:annotation>
<xs:complexType name='c'><xs:sequence><xs:element name='a'><xs:annotation><xs:documentation>a</xs:documentation></xs:annotation></xs:element></xs:sequence>
<xs:attribute name='x'><xs:annotation><xs:documentation>x</xs:documentation></xs:annotation></xs:attribute></xs:complexType>
<xs:element name='e'><xs:annotation><xs:documentation> <b>e</b> <!--c--></xs:documentation></xs:annotation></xs:element>
</xs:schema>";
    let schema = Schema::parse(source.as_bytes(), "a.xsd").unwrap();
    let t = Some("urn:t");
    let c = schema.type_definition(t, "c").unwrap();
    let content = c.particle().unwrap();
    let Some(Term::Group(sequence)) = content.term() else {
        panic!("sequence")
    };
    let local = sequence.particles().next().unwrap();
    let Some(Term::Element(a)) = local.term() else {
        panic!("element")
    };
    let x = c.attribute_uses().next().unwrap();
    let int = schema.type_definition(Some("http://www.w3.org/2001/XMLSchema"), "int");
    let cases: [(&str, (usize, usize)); 10] = [
        // Blank text dropped even under xml:space='preserve' (libxml2 asks
        // xmlNodeGetSpacePreserve of the text node: -1); comment, PI too.
        ("schema", summary(schema.annotation().into_iter())),
        // Blank text and the comment dropped.
        (
            "global element",
            summary(schema.element(t, "e").unwrap().annotations()),
        ),
        ("type without one", summary(c.annotations())),
        ("content particle", summary(content.annotations())),
        ("sequence", summary(sequence.annotations())),
        ("local element particle", summary(local.annotations())),
        ("local element declaration", summary(a.annotations())),
        ("attribute use", summary(x.annotations())),
        (
            "local attribute declaration",
            summary(x.declaration().annotations()),
        ),
        ("built-in type", summary(int.unwrap().annotations())),
    ];
    let expected = [
        ("schema", (1, 3)),
        ("global element", (1, 4)),
        ("type without one", (0, 0)),
        ("content particle", (0, 0)),
        ("sequence", (0, 0)),
        ("local element particle", (0, 0)),
        ("local element declaration", (1, 3)),
        ("attribute use", (1, 3)),
        ("local attribute declaration", (0, 0)),
        ("built-in type", (0, 0)),
    ];
    assert_eq!(cases, expected);
    let bare = Schema::parse(
        b"<xs:schema xmlns:xs='http://www.w3.org/2001/XMLSchema'/>",
        "b.xsd",
    );
    assert!(bare.unwrap().annotation().is_none());
}

#[test]
fn lookups_find_globals_and_built_ins_only() {
    let schema = Schema::parse(SCHEMA.as_bytes(), "a.xsd").unwrap();
    let xs = Some("http://www.w3.org/2001/XMLSchema");
    let t = Some("urn:t");
    let cases: [(&str, Option<&str>, &str, bool); 17] = [
        ("element", t, "e", true),
        ("element", None, "e", false),
        ("element", t, "c", false),
        ("attribute", t, "g", true),
        ("attribute", t, "x", false),
        ("type", t, "m", true),
        ("type", xs, "int", true),
        ("type", xs, "anyType", true),
        ("type", t, "nope", false),
        ("attribute group", t, "ag", true),
        ("attribute group", None, "ag", false),
        ("identity constraint", t, "fk", true),
        ("identity constraint", None, "fk", false),
        ("identity constraint", t, "k", false),
        ("model group", t, "g1", true),
        ("model group", None, "g1", false),
        ("model group", t, "c1", false),
    ];
    for (kind, namespace, name, found) in cases {
        let got = match kind {
            "element" => schema
                .element(namespace, name)
                .map(|e| (e.namespace(), e.name())),
            "attribute" => schema
                .attribute(namespace, name)
                .map(|a| (a.namespace(), a.name())),
            "model group" => {
                (schema.model_group(namespace, name)).map(|g| (g.namespace(), g.name()))
            }
            "identity constraint" => {
                (schema.identity_constraint(namespace, name)).map(|c| (c.namespace(), c.name()))
            }
            "attribute group" => {
                (schema.attribute_group(namespace, name)).map(|g| (g.namespace(), g.name()))
            }
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
    let m = schema.type_definition(t, "m").unwrap();
    assert!(m.whitespace().is_none() && m.facets().is_empty() && m.patterns().is_empty());
    let r = schema.type_definition(t, "r").unwrap();
    assert_eq!(r.patterns()[1].as_str(), "x");
    assert_eq!(schema.target_namespace(), t);
    let [h, i, j] = ["h", "i", "j"].map(|name| schema.element(t, name).unwrap());
    let members: Vec<_> = h.substitutes().map(|e| e.name()).collect();
    assert_eq!(members, ["i"], "abstract j substitutes for no head");
    assert_eq!(j.substitution_group().unwrap().name(), "i");
    assert!(h.substitution_group().is_none() && i.substitutes().next().is_none());
    let empty = Schema::parse(
        b"<xs:schema xmlns:xs='http://www.w3.org/2001/XMLSchema'/>",
        "e.xsd",
    );
    let lines = components(&empty.unwrap());
    assert_eq!(lines, BTreeSet::from(["schema tns=-".to_owned()]));
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
