use yaxml::options::*;
#[test]
fn dbg() {
    let all =
        NOENT | DTDATTR | NOCDATA | NSCLEAN | RECOVER | NOBLANKS | DTDVALID | IGNORE_ENC | SKIP_IDS;
    for flags in [all, RECOVER] {
        for input in [
            &b"<a><?xml?>>"[..],
            b"<?xml version=\"1.0\"?>\n<a><?xml?></a>\n",
            b"<doc>1 < 2 but not</doc>",
        ] {
            let d = yaxml::Document::parse_with(input, yaxml::ParseOptions::new(flags)).unwrap();
            let s = yaxml::save::to_vec(&d, &Default::default()).unwrap();
            let kinds: Vec<_> = d.errors().iter().map(|e| e.kind).collect();
            eprintln!(
                "{flags:x} {:?} -> {:?} {kinds:?}",
                String::from_utf8_lossy(input),
                String::from_utf8(s).unwrap()
            );
        }
    }
}
