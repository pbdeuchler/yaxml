//! SAX2 callbacks (feature `sax`), pinned against libxml2 2.15.4's default
//! SAX2 handlers wrapped to log each call (`xmlSAXVersion(2)`,
//! `xmlNewSAXParserCtxt`, `xmlCtxtReadFile`), in the helper's line format.
//! Error lines name the `ErrorKind` (libxml2: domain, code, level).

use std::fmt::Write;
use std::ops::ControlFlow;

use yaxml::valid::{self, AttributeDefault};
use yaxml::{Declaration, Doctype, Element, Error, ErrorKind, Name, ParseOptions, options, sax};

const XMLNS: &str = "http://www.w3.org/2000/xmlns/";

#[derive(Default)]
struct Log {
    lines: String,
    /// Stop at the first callback whose line starts with this.
    stop: Option<&'static str>,
    /// Prefix each line with the locator's `@line:column`.
    places: bool,
    place: String,
}

impl Log {
    fn line(&mut self, line: String) -> ControlFlow<()> {
        writeln!(self.lines, "{}{line}", self.place).unwrap();
        match self.stop {
            Some(stop) if line.starts_with(stop) => ControlFlow::Break(()),
            _ => ControlFlow::Continue(()),
        }
    }
}

fn s(text: Option<&str>) -> &str {
    text.unwrap_or("-")
}

impl sax::Handler for Log {
    fn locate(&mut self, locator: &sax::Locator<'_>) {
        if self.places {
            self.place = format!("@{}:{} ", locator.line(), locator.column());
        }
    }
    fn start_document(&mut self, declaration: Option<&Declaration<'_>>) -> ControlFlow<()> {
        let version = declaration.map(|declaration| declaration.version);
        self.line(format!("startDocument {}", s(version)))
    }
    fn internal_subset(&mut self, d: &Doctype<'_>) -> ControlFlow<()> {
        self.line(format!(
            "internalSubset {} {} {}",
            d.name,
            s(d.public_id),
            s(d.system_id)
        ))
    }
    fn external_subset(&mut self, d: &Doctype<'_>) -> ControlFlow<()> {
        self.line(format!(
            "externalSubset {} {} {}",
            d.name,
            s(d.public_id),
            s(d.system_id)
        ))
    }
    fn entity_decl(&mut self, e: valid::Entity<'_>) -> ControlFlow<()> {
        let (kind, public, system) = (e.kind as u8, s(e.public_id), s(e.system_id));
        self.line(format!(
            "entityDecl {} {kind} {public} {system} {}",
            e.name,
            s(e.content)
        ))
    }
    fn unparsed_entity_decl(&mut self, e: valid::Entity<'_>) -> ControlFlow<()> {
        let (public, system, notation) = (s(e.public_id), s(e.system_id), s(e.notation));
        self.line(format!(
            "unparsedEntityDecl {} {public} {system} {notation}",
            e.name
        ))
    }
    fn notation_decl(&mut self, n: valid::Notation<'_>) -> ControlFlow<()> {
        let (public, system) = (s(n.public_id), s(n.system_id));
        self.line(format!("notationDecl {} {public} {system}", n.name))
    }
    fn element_decl(&mut self, e: valid::Element<'_>) -> ControlFlow<()> {
        self.line(format!("elementDecl {} {}", e.name, e.kind as u8))
    }
    fn attribute_decl(&mut self, a: valid::Attribute<'_>) -> ControlFlow<()> {
        let (default, value) = match a.default {
            AttributeDefault::Value(value) => (1, Some(value)),
            AttributeDefault::Required => (2, None),
            AttributeDefault::Implied => (3, None),
            AttributeDefault::Fixed(value) => (4, Some(value)),
        };
        let kind = a.kind as u8;
        self.line(format!(
            "attributeDecl {} {} {kind} {default} {}",
            a.element,
            a.name,
            s(value)
        ))
    }
    fn start_element(&mut self, element: &Element<'_>) -> ControlFlow<()> {
        let name = &element.name;
        let mut line = format!(
            "startElementNs {} {} {}",
            name.local(),
            s(name.prefix()),
            s(name.namespace())
        );
        let (declarations, attributes): (Vec<_>, Vec<_>) = (element.attributes.iter())
            .partition(|attribute| attribute.name.namespace() == Some(XMLNS));
        for declaration in declarations {
            let prefix = (declaration.name.prefix()).map(|_| declaration.name.local());
            write!(line, " xmlns:{}={}", s(prefix), declaration.value).unwrap();
        }
        for attribute in attributes {
            let name = &attribute.name;
            let colon = name.prefix().map_or("", |_| ":");
            let (prefix, local, uri) = (s(name.prefix()), name.local(), s(name.namespace()));
            write!(line, " {prefix}{colon}{local}{{{uri}}}={}", attribute.value).unwrap();
        }
        self.line(line)
    }
    fn end_element(&mut self, name: &Name<'_>) -> ControlFlow<()> {
        let (prefix, uri) = (s(name.prefix()), s(name.namespace()));
        self.line(format!("endElementNs {} {prefix} {uri}", name.local()))
    }
    fn characters(&mut self, text: &str) -> ControlFlow<()> {
        self.line(format!("characters {text}|"))
    }
    fn cdata_block(&mut self, text: &str) -> ControlFlow<()> {
        self.line(format!("cdataBlock {text}"))
    }
    fn comment(&mut self, text: &str) -> ControlFlow<()> {
        self.line(format!("comment {text}"))
    }
    fn processing_instruction(&mut self, target: &str, data: &str) -> ControlFlow<()> {
        self.line(format!("processingInstruction {target} {data}"))
    }
    fn reference(&mut self, name: &str) -> ControlFlow<()> {
        self.line(format!("reference {name}"))
    }
    fn warning(&mut self, error: &Error) {
        let _ = self.line(format!("warning {:?}", error.kind));
    }
    fn error(&mut self, error: &Error) {
        let _ = self.line(format!("error {:?}", error.kind));
    }
    fn fatal_error(&mut self, error: &Error) {
        let _ = self.line(format!("fatal {:?}", error.kind));
    }
    fn end_document(&mut self) {
        let _ = self.line("endDocument".into());
    }
}

const DTD: &str = "<!DOCTYPE r [
<!ENTITY a \"va\">
<!NOTATION n SYSTEM \"n\">
<!ENTITY u SYSTEM \"u.bin\" NDATA n>
<!ELEMENT r (#PCDATA|e)*>
<!ATTLIST r x CDATA \"d\" y (p|q) #IMPLIED>
<!ENTITY % pe \"x\">
<?pi d?>
<!-- c -->
]>
<r xmlns:p=\"urn:p\" p:z=\"1\">t&a;<![CDATA[c]]><p:e/><?q r?><!--k--></r>";

/// Without `NOENT` libxml2 also reports `characters va|` (and the elements)
/// of an entity's first reference before `reference`, and declarations in
/// source order with the DTD's PI and comment. DTD defaults come after the
/// specified attributes with or without `DTDATTR` (`nb_defaulted`).
const DTD_EVENTS: &str = "startDocument -
internalSubset r - -
entityDecl a 1 - - va
unparsedEntityDecl u - u.bin n
entityDecl pe 4 - - x
notationDecl n - n
elementDecl r 3
attributeDecl r x 1 1 d
attributeDecl r y 9 3 -
externalSubset r - -
startElementNs r - - xmlns:p=urn:p p:z{urn:p}=1 -x{-}=d
characters t|
reference a
cdataBlock c
startElementNs e p urn:p
endElementNs e p urn:p
processingInstruction q r
comment k
endElementNs r - -
endDocument
";

#[test]
fn callbacks_follow_libxml2() {
    let references = "<!DOCTYPE r [<!ENTITY a 'v<b/>w'><!ENTITY e SYSTEM 'e.xml'>\
                      <!ATTLIST r x CDATA 'd'>]><r>&a;&e;&a;</r>";
    let cases: [(&str, &str, u32, &str); 10] = [
        ("declarations", DTD, 0, DTD_EVENTS),
        // libxml2 reports `b` only for the first reference; later ones as
        // their text alone (`characters v|`, `characters w|`).
        (
            "noent",
            references,
            options::NOENT,
            "startDocument -\ninternalSubset r - -\nentityDecl a 1 - - v<b/>w\n\
             entityDecl e 2 - e.xml -\nattributeDecl r x 1 1 d\nexternalSubset r - -\n\
             startElementNs r - - -x{-}=d\ncharacters v|\nstartElementNs b - -\nendElementNs b - -\n\
             characters w|\ncharacters v|\nstartElementNs b - -\nendElementNs b - -\n\
             characters w|\nendElementNs r - -\nendDocument\n",
        ),
        // libxml2 marks `x` `(default)` (`nb_defaulted`).
        (
            "dtdattr",
            references,
            options::DTDATTR,
            "startDocument -\ninternalSubset r - -\nentityDecl a 1 - - v<b/>w\n\
             entityDecl e 2 - e.xml -\nattributeDecl r x 1 1 d\nexternalSubset r - -\n\
             startElementNs r - - -x{-}=d\nreference a\nreference e\nreference a\n\
             endElementNs r - -\nendDocument\n",
        ),
        (
            "undeclared",
            "<!DOCTYPE r SYSTEM 'x.dtd'><r>&u;</r>",
            0,
            "startDocument -\ninternalSubset r - x.dtd\nexternalSubset r - x.dtd\n\
             startElementNs r - -\nwarning Warning(UndeclaredEntity { error: false })\n\
             reference u\nendElementNs r - -\nendDocument\n",
        ),
        (
            "namespaces",
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <r xmlns=\"urn:d\" xmlns:q=\"urn:q\"><q:x a=\"1\" q:b=\"2\"/></r>",
            0,
            "startDocument 1.0\nstartElementNs r - urn:d xmlns:-=urn:d xmlns:q=urn:q\n\
             startElementNs x q urn:q -a{-}=1 q:b{urn:q}=2\nendElementNs x q urn:q\n\
             endElementNs r - urn:d\nendDocument\n",
        ),
        (
            "fatal",
            "<r><a>t</b></r>",
            0,
            "startDocument -\nstartElementNs r - -\nstartElementNs a - -\ncharacters t|\n\
             fatal MismatchedEndTag\nendDocument\n",
        ),
        (
            "recovered",
            "<r><a>t</b></r>",
            options::RECOVER,
            "startDocument -\nstartElementNs r - -\nstartElementNs a - -\ncharacters t|\n\
             error MismatchedEndTag\nendElementNs a - -\nendElementNs r - -\nendDocument\n",
        ),
        (
            "namespace error",
            "<r><p:x/></r>",
            0,
            "startDocument -\nstartElementNs r - -\nerror Namespace\nstartElementNs x p -\n\
             endElementNs x p -\nendElementNs r - -\nendDocument\n",
        ),
        // "Document is empty": no document callbacks; any other input starts one.
        ("empty", "", 0, "fatal MissingRoot\n"),
        (
            "blank",
            " ",
            0,
            "startDocument -\nfatal MissingRoot\nendDocument\n",
        ),
    ];
    for (name, input, flags, expected) in cases {
        let mut log = Log::default();
        let result = sax::parse(input.as_bytes(), ParseOptions::new(flags), &mut log);
        assert_eq!(log.lines, expected, "{name}");
        let fatal = expected.contains("fatal ");
        assert_eq!(result.is_err(), fatal, "{name}");
    }
}

#[test]
fn external_subset_declarations_come_before_the_root() {
    let dtd = |_: yaxml::Request<'_>| {
        Ok(Some(
            b"<!ENTITY b 'e'><!ELEMENT r EMPTY><!ATTLIST r k CDATA 'v'>".to_vec(),
        ))
    };
    let options = ParseOptions::new(options::DTDLOAD).with_resolver("", Box::new(dtd));
    let input = "<!DOCTYPE r SYSTEM 'x.dtd' [<!ENTITY a 'i'>]><r/>";
    let mut log = Log::default();
    sax::parse(input.as_bytes(), options, &mut log).unwrap();
    // libxml2 passes `k='v'` as defaulted.
    let expected = "startDocument -\ninternalSubset r - x.dtd\nentityDecl a 1 - - i\n\
                    externalSubset r - x.dtd\nentityDecl b 1 - - e\nelementDecl r 1\n\
                    attributeDecl r k 1 1 v\nstartElementNs r - - -k{-}=v\nendElementNs r - -\n\
                    endDocument\n";
    assert_eq!(log.lines, expected);
}

#[test]
fn attribute_values_follow_libxml2() {
    // Without `NOENT` libxml2 keeps references and passes an `&` from a
    // reference as `&#38;` (namespace declarations excepted); defaults come
    // after the specified attributes, normalized.
    let kept = "<!DOCTYPE r [<!ENTITY e 'E'><!ATTLIST r x CDATA 'a&e;b'>]><r y='&e;&amp;'/>";
    let cases: [(&str, &str, u32, &str); 9] = [
        (
            "escaped",
            "<r x='a&amp;b&#38;c&lt;d'/>",
            0,
            "r - - -x{-}=a&#38;b&#38;c<d",
        ),
        (
            "substituted",
            "<r x='a&amp;b&#38;c&lt;d'/>",
            options::NOENT,
            "r - - -x{-}=a&b&c<d",
        ),
        ("kept", kept, 0, "r - - -y{-}=&e;&#38; -x{-}=a&e;b"),
        (
            "kept noent",
            kept,
            options::NOENT,
            "r - - -y{-}=E& -x{-}=aEb",
        ),
        (
            "xmlns",
            "<r xmlns:p='a&amp;b' p:x='1&amp;'/>",
            0,
            "r - - xmlns:p=a&b p:x{a&b}=1&#38;",
        ),
        (
            "specified wins",
            "<!DOCTYPE r [<!ATTLIST r x CDATA 'd'>]><r x='s'/>",
            0,
            "r - - -x{-}=s",
        ),
        (
            "child nmtokens",
            "<!DOCTYPE r [<!ATTLIST c t NMTOKENS '  a   b '>]><r><c/></r>",
            0,
            "c - - -t{-}=a b",
        ),
        (
            "defaulted xmlns",
            "<!DOCTYPE r [<!ATTLIST r xmlns:p CDATA 'urn:p' p:a CDATA '1'>]><r/>",
            0,
            "r - - xmlns:p=urn:p p:a{urn:p}=1",
        ),
        (
            "char ref default",
            "<!DOCTYPE r [<!ATTLIST r x CDATA 'a&#38;b'>]><r/>",
            0,
            "r - - -x{-}=a&#38;b",
        ),
    ];
    for (name, input, flags, expected) in cases {
        let mut log = Log::default();
        sax::parse(input.as_bytes(), ParseOptions::new(flags), &mut log).unwrap();
        let line = format!("startElementNs {expected}\n");
        assert!(log.lines.contains(&line), "{name}: {}", log.lines);
    }
}

#[test]
fn a_callback_stops_the_parse() {
    let cases = [
        ("startDocument", "startDocument -\n"),
        ("internalSubset", "startDocument -\ninternalSubset r - -\n"),
        (
            "entityDecl",
            "startDocument -\ninternalSubset r - -\nentityDecl a 1 - - va\n",
        ),
        (
            "characters",
            &DTD_EVENTS[..DTD_EVENTS.find("characters t|\n").unwrap() + 14],
        ),
        (
            "reference",
            &DTD_EVENTS[..DTD_EVENTS.find("cdataBlock").unwrap()],
        ),
    ];
    for (stop, expected) in cases {
        let mut log = Log {
            stop: Some(stop),
            ..Log::default()
        };
        assert_eq!(
            sax::parse(DTD.as_bytes(), ParseOptions::default(), &mut log),
            Ok(())
        );
        assert_eq!(log.lines, expected, "{stop}");
    }
}

#[test]
fn input_and_option_errors_come_before_any_callback() {
    let cases: [(&[u8], u32, ErrorKind); 2] = [
        (b"\xff\xfe<\0r", 0, ErrorKind::InvalidUtf16),
        (b"<r/>", options::OLDSAX, ErrorKind::UnsupportedOption),
    ];
    for (input, flags, kind) in cases {
        let mut log = Log::default();
        let error = sax::parse(input, ParseOptions::new(flags), &mut log).unwrap_err();
        assert_eq!((error.kind, log.lines.as_str()), (kind, ""));
    }
    // A UTF-16 document is decoded first.
    let utf16: Vec<u8> = "\u{feff}<r>é</r>"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let mut log = Log::default();
    sax::parse(&utf16, ParseOptions::default(), &mut log).unwrap();
    assert!(log.lines.contains("characters é|"), "{}", log.lines);
}

/// libxml2's `areBlanks` needs `ctxt->node`, which only a tree builder sets:
/// under SAX, `NOBLANKS` reports blanks as `characters`, declared or not.
#[test]
fn noblanks_changes_no_callback() {
    for input in [
        "<r>\n <a/>\n</r>",
        "<!DOCTYPE r [<!ELEMENT r (a)*><!ELEMENT a EMPTY>]><r>\n <a/>\n</r>",
    ] {
        let (mut plain, mut noblanks) = (Log::default(), Log::default());
        sax::parse(input.as_bytes(), ParseOptions::default(), &mut plain).unwrap();
        let options = ParseOptions::new(options::NOBLANKS);
        sax::parse(input.as_bytes(), options, &mut noblanks).unwrap();
        assert!(plain.lines.contains("characters \n |"), "{}", plain.lines);
        assert_eq!(noblanks.lines, plain.lines, "{input}");
    }
}

#[test]
fn the_locator_is_libxml2s_parsing_point() {
    // Recorded from xmlSAX2GetLineNumber/ColumnNumber in each callback.
    let cases = [
        (
            "\u{feff}<r>é\u{e9}x</r>",
            "@1:1 startDocument -\n@1:3 startElementNs r - -\n@1:7 characters ééx|\n\
             @1:11 endElementNs r - -\n@1:11 endDocument\n",
        ),
        (
            "<r\n a=\"x\ny\"\n>t</r>",
            "@1:1 startDocument -\n@4:1 startElementNs r - - -a{-}=x y\n@4:3 characters t|\n\
             @4:7 endElementNs r - -\n@4:7 endDocument\n",
        ),
        (
            "<?xml version=\"1.0\"?><!--c-->\n<r/>  \n<!--d-->\n",
            "@1:22 startDocument 1.0\n@1:30 comment c\n@2:3 startElementNs r - -\n\
             @2:5 endElementNs r - -\n@3:9 comment d\n@4:1 endDocument\n",
        ),
        (
            "<?xml version=\"1.0\"?>\n<r>\n  t <![CDATA[x]]><?p d?>\n<é b=\"2\"/></r>",
            "@2:1 startDocument 1.0\n@2:3 startElementNs r - -\n@3:5 characters \n  t |\n\
             @3:18 cdataBlock x\n@3:25 processingInstruction p d\n@4:1 characters \n|\n\
             @4:9 startElementNs é - - -b{-}=2\n@4:11 endElementNs é - -\n\
             @4:15 endElementNs r - -\n@4:15 endDocument\n",
        ),
        (
            "<r><a></a></r",
            "@1:1 startDocument -\n@1:3 startElementNs r - -\n@1:6 startElementNs a - -\n\
             @1:11 endElementNs a - -\n@1:14 fatal UnexpectedEof\n@1:14 endDocument\n",
        ),
        (
            "<r><a></b></r>",
            "@1:1 startDocument -\n@1:3 startElementNs r - -\n@1:6 startElementNs a - -\n\
             @1:15 fatal MismatchedEndTag\n@1:15 endDocument\n",
        ),
        // Outside the root element libxml2 stops at the error.
        (
            "<r/>x  ",
            "@1:1 startDocument -\n@1:3 startElementNs r - -\n@1:5 endElementNs r - -\n\
             @1:5 fatal TextOutsideRoot\n@1:5 endDocument\n",
        ),
        (
            "x<r/>",
            "@1:1 startDocument -\n@1:1 fatal TextOutsideRoot\n@1:1 endDocument\n",
        ),
    ];
    for (input, expected) in cases {
        let mut log = Log {
            places: true,
            ..Log::default()
        };
        let _ = sax::parse(input.as_bytes(), ParseOptions::default(), &mut log);
        assert_eq!(log.lines, expected, "{input}");
    }
}

#[test]
fn the_locator_names_the_base() {
    #[derive(Default)]
    struct Base(Vec<Option<String>>);
    impl sax::Handler for Base {
        fn locate(&mut self, locator: &sax::Locator<'_>) {
            self.0.push(locator.system_id().map(str::to_owned));
        }
    }
    for base in [None, Some("doc.xml")] {
        let mut handler = Base::default();
        let options = ParseOptions::default().base(base.map(str::to_owned));
        sax::parse(b"<r/>", options, &mut handler).unwrap();
        // Start, the start tag's `/>`, after it.
        assert_eq!(handler.0, vec![base.map(str::to_owned); 3], "{base:?}");
    }
}

#[test]
fn the_locator_window_is_libxml2s() {
    // Recorded from xmlCtxtGetInputWindow(ctxt, 0, ..) in each callback.
    struct Windows {
        size: usize,
        window: String,
        lines: String,
    }
    impl sax::Handler for Windows {
        fn locate(&mut self, locator: &sax::Locator<'_>) {
            let (content, column) = locator.window(self.size);
            self.window = format!("{content}|{column}");
        }
        fn start_element(&mut self, element: &Element<'_>) -> ControlFlow<()> {
            let _ = writeln!(self.lines, "{}|{}", element.name.local(), self.window);
            ControlFlow::Continue(())
        }
        fn characters(&mut self, _: &str) -> ControlFlow<()> {
            let _ = writeln!(self.lines, "#text|{}", self.window);
            ControlFlow::Continue(())
        }
    }
    let nested = "<r>\n  <a b=\"1\">tx</a>\n<é>z</é></r>";
    let cases = [
        (
            nested,
            80,
            "r|<r>|2\n#text|  <a b=\"1\">tx</a>|2\na|  <a b=\"1\">tx</a>|10\n\
             #text|  <a b=\"1\">tx</a>|13\n#text|<é>z</é></r>|0\né|<é>z</é></r>|3\n\
             #text|<é>z</é></r>|5\n",
        ),
        // A small window ends before the position (or a character it splits).
        (
            nested,
            4,
            "r|<r>|2\n#text|  <a|2\na|=\"1\"|3\n#text|\">tx|3\n#text|<é>|0\né|<é>|3\n\
             #text|é>z|3\n",
        ),
        // Line ends before the position are skipped back over.
        ("<r>\n\n</r>", 80, "r|<r>|2\n#text|</r>|0\n"),
    ];
    for (input, size, expected) in cases {
        let mut windows = Windows {
            size,
            window: String::new(),
            lines: String::new(),
        };
        sax::parse(input.as_bytes(), ParseOptions::default(), &mut windows).unwrap();
        assert_eq!(windows.lines, expected, "{input} {size}");
    }
}
