#![forbid(unsafe_code)]
use std::collections::HashMap;
use yaxml::{Document, Request, Resolver};

fn memory(files: &[(&str, &str)]) -> Box<dyn Resolver> {
    let files: HashMap<String, Vec<u8>> = files
        .iter()
        .map(|(k, v)| (k.to_string(), v.as_bytes().to_vec()))
        .collect();
    Box::new(move |request: Request<'_>| Ok(files.get(request.uri).cloned()))
}

#[test]
fn smoke() {
    let input = br#"<!DOCTYPE r SYSTEM "r.dtd" [<!ENTITY e SYSTEM "e.txt">]><r>&e;&x;</r>"#;
    let doc = Document::with_resolver(input, Default::default(), "", memory(&[
        ("r.dtd", "<?xml version='1.0' encoding='UTF-8'?><!ENTITY % p '<!ENTITY x \"X\">'><![ INCLUDE [ %p; ]]><![IGNORE[ <![ x ]]> ]]><!ATTLIST r id ID #IMPLIED a CDATA 'dflt'>"),
        ("e.txt", "<?xml encoding='utf-8'?>hello<b/>"),
    ])).unwrap();
    let mut out = String::new();
    for n in doc.nodes() {
        out.push_str(&format!("{:?}\n", n.kind()));
    }
    println!("{out}");
}
