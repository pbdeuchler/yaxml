use super::*;

fn render(token: &Token<'_>) -> String {
    match token {
        Token::Text { text, mode, .. } if mode.raw() => format!("C({text})"),
        Token::Text { text, .. } => format!("T({text})"),
        Token::Literal(text) => format!("L({text})"),
        Token::StartTag {
            name,
            attributes,
            end,
            ..
        } => {
            let mut out = format!("S({name}");
            for (name, value) in attributes {
                out.push(' ');
                out.push_str(name);
                if let Some(value) = value {
                    out.push('=');
                    out.push_str(value);
                }
            }
            out.push_str(match end {
                TagEnd::Open => ")",
                TagEnd::SelfClosing => "/)",
                TagEnd::Eof => "!)",
            });
            out
        }
        Token::EndTag(name) => format!("E({name})"),
        Token::Comment(text) => format!("#({text})"),
        Token::Doctype { doctype, first } => format!(
            "D({}|{}|{}{})",
            doctype.name.as_deref().unwrap_or("-"),
            doctype.public_id.as_deref().unwrap_or("-"),
            doctype.system_id.as_deref().unwrap_or("-"),
            if *first { "" } else { " late" }
        ),
    }
}

fn tokenizer(mode: Mode, end: &str) -> Tokenizer {
    let mut tokenizer = Tokenizer::new(1 << 20, 50_000, 64);
    tokenizer.has_encoding = true;
    tokenizer.enter(mode, end);
    tokenizer
}

/// Enters raw modes after start tags the way the tree builder does.
fn react(tokenizer: &mut Tokenizer, token: &Token<'_>) {
    if let Token::StartTag {
        name,
        end: TagEnd::Open,
        ..
    } = token
        && let Some(element) = super::super::tables::element(name)
    {
        tokenizer.enter(element.mode, name);
    }
}

/// All tokens of a complete input.
fn scan(tokenizer: &mut Tokenizer, input: &[u8]) -> Result<Vec<String>, Stop> {
    let mut out = Vec::new();
    let mut position = 0;
    while let Some((token, used)) = tokenizer.next(&input[position..], true)? {
        if let Some(token) = token {
            react(tokenizer, &token);
            out.push(render(&token));
        }
        position += used;
    }
    Ok(out)
}

/// The same tokens when the text arrives one byte at a time.
fn scan_incrementally(mut tokenizer: Tokenizer, input: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut position = 0;
    let mut available = 0;
    loop {
        let eof = available == input.len();
        match tokenizer.next(&input[position..available], eof) {
            Ok(Some((token, used))) => {
                if let Some(token) = token {
                    react(&mut tokenizer, &token);
                    out.push(render(&token));
                }
                position += used;
            }
            Ok(None) => return out,
            Err(Stop::More) => available += 1,
            Err(stop) => panic!("{stop:?}"),
        }
    }
}

type Case = (&'static [u8], Mode, &'static [&'static str]);

const CASES: &[Case] = &[
    (b"a&amp;b", Mode::Data, &["T(a)", "T(&)", "T(b)"]),
    (b"a<b>c", Mode::Data, &["T(a)", "S(b)", "T(c)"]),
    (
        b"&notit; &notin;",
        Mode::Data,
        &["T(\u{ac})", "T(it; )", "T(\u{2209})"],
    ),
    (
        b"&amp &ampx &AMP;",
        Mode::Data,
        &["T(&)", "T( )", "T(&)", "T(x )", "T(&)"],
    ),
    (
        b"&#x41;&#65&#128;&#0;&#xD800;",
        Mode::Data,
        &["T(A)", "T(A)", "T(\u{20ac})", "T(\u{fffd})", "T(\u{fffd})"],
    ),
    (b"&#;&#x;&zz;&", Mode::Data, &["T(&#;&#x;&zz;&)"]),
    (b"a\0b", Mode::Data, &["T(a)", "T(b)"]),
    (
        b"a\r\nb\rc",
        Mode::Data,
        &["T(a)", "T(\nb)", "T(\n)", "T(c)"],
    ),
    (
        b"a\xffb\xc3\xa9",
        Mode::Data,
        &["T(a)", "T(\u{fffd})", "T(b\u{e9})"],
    ),
    (
        b"<!--x--><!--><!---><!--a--!><!--b",
        Mode::Data,
        &["#(x)", "#()", "#()", "#(a)", "#(b)"],
    ),
    (b"<!--a-", Mode::Data, &["#(a)"]),
    (
        b"<?x y><!z></3></></",
        Mode::Data,
        &["#(?x y)", "#(z)", "#(3)", "L(</)"],
    ),
    (b"< a<", Mode::Data, &["L(<)", "T( a)", "L(<)"]),
    (
        b"<!DOCTYPE html PUBLIC 'p' \"s\"><!doctype x SYSTEM 'y'>",
        Mode::Data,
        &["D(html|p|s)", "D(x|-|y late)"],
    ),
    (
        b"<!DOCTYPE html PUBLIC x 'y'>",
        Mode::Data,
        &["D(html|-|-)"],
    ),
    (b"<!DOCTYPE>", Mode::Data, &["D(-|-|-)"]),
    (
        b"  <!--c-->  <!DOCTYPE a>  x",
        Mode::Data,
        &["#(c)", "D(a|-|-)", "T(x)"],
    ),
    (
        b"<a b c=d e='f' g=\"h\" b=x>",
        Mode::Data,
        &["S(a b c=d e=f g=h)"],
    ),
    (b"<A B=1 =c d/e f= g>", Mode::Data, &["S(a b=1 =c d e f=g)"]),
    (b"<a href=x/>", Mode::Data, &["S(a href=x/)"]),
    (b"<a/><b/c>", Mode::Data, &["S(a/)", "S(b c)"]),
    (
        b"<a b='&notit;' c=&notit; d='&amp='>",
        Mode::Data,
        &["S(a b=&notit; c=&notit; d=&amp=)"],
    ),
    (b"<a", Mode::Data, &["S(a!)"]),
    (b"<a b='x", Mode::Data, &["S(a b=x!)"]),
    (b"</a b=c></b", Mode::Data, &["E(a)"]),
    (b"<a\0b>", Mode::Data, &["S(a\u{fffd}b)"]),
    (
        b"<style>x</y>z</STYLE >w",
        Mode::Data,
        &["S(style)", "C(x</y>z)", "E(style)", "T(w)"],
    ),
    (
        b"<title>&amp;<b></title>",
        Mode::Data,
        &["S(title)", "T(&)", "T(<b>)", "E(title)"],
    ),
    (
        b"<textarea>\0</textarea>",
        Mode::Data,
        &["S(textarea)", "T(\u{fffd})", "E(textarea)"],
    ),
    (
        b"<script>&amp;\0</script>",
        Mode::Data,
        &["S(script)", "C(&amp;)", "C(\u{fffd})", "E(script)"],
    ),
    (
        b"<script><!-- <script> </script> --></script>x",
        Mode::Data,
        &[
            "S(script)",
            "C(<!-- <script> </script> -->)",
            "E(script)",
            "T(x)",
        ],
    ),
    (
        b"<script><!-- </script>x",
        Mode::Data,
        &["S(script)", "C(<!-- )", "E(script)", "T(x)"],
    ),
    (b"<script></scr", Mode::Data, &["S(script)", "C(</scr)"]),
    (
        b"<plaintext>a</plaintext>b",
        Mode::Data,
        &["S(plaintext)", "C(a</plaintext>b)"],
    ),
    (b"x</xmp>y", Mode::Rawtext, &["C(x)", "E(xmp)", "T(y)"]),
];

#[test]
fn tokens_match_libxml2_for_each_state() {
    for (input, mode, expected) in CASES {
        let tokens = scan(&mut tokenizer(*mode, "xmp"), input).unwrap();
        assert_eq!(&tokens, expected, "{:?}", String::from_utf8_lossy(input));
        let split = scan_incrementally(tokenizer(*mode, "xmp"), input);
        assert_eq!(
            &split,
            expected,
            "split {:?}",
            String::from_utf8_lossy(input)
        );
    }
}

#[test]
fn names_are_lowercased_and_truncated_at_100_bytes() {
    let long = format!("<{}>", "A".repeat(150));
    let tokens = scan(&mut tokenizer(Mode::Data, ""), long.as_bytes()).unwrap();
    assert_eq!(tokens, [format!("S({})", "a".repeat(100))]);
}

#[test]
fn incomplete_tokens_wait_for_input() {
    let cases: &[&[u8]] = &[
        b"<a", b"<a b='x", b"&am", b"&#12", b"a", b"\r", b"<!--x", b"<!DOCTY", b"<!", b"</a",
        b"\xc3", b"<!--a--",
    ];
    for input in cases {
        let result = tokenizer(Mode::Data, "").next(input, false);
        assert_eq!(
            result.err(),
            Some(Stop::More),
            "{:?}",
            String::from_utf8_lossy(input)
        );
    }
}

#[test]
fn undecided_encoding_stops_at_the_first_non_ascii_byte() {
    let mut fresh = Tokenizer::new(1 << 20, 50_000, 64);
    let (token, used) = fresh.next(b"ab\xe9", true).unwrap().unwrap();
    assert_eq!((render(&token.unwrap()), used), ("T(ab)".to_owned(), 2));
    assert_eq!(fresh.next(b"\xe9", true).err(), Some(Stop::Switch(0)));
    assert_eq!(
        fresh.next(b"<p a='x\xe9'>", true).err(),
        Some(Stop::Switch(7))
    );
    let mut prolog = Tokenizer::new(1 << 20, 50_000, 64);
    assert_eq!(
        prolog.next(b"  <!--\xe9-->", true).err(),
        Some(Stop::Switch(6))
    );
    // Names are read as UTF-8 regardless.
    let (token, _) = fresh.next(b"<p\xc3\xa9>", true).unwrap().unwrap();
    assert_eq!(render(&token.unwrap()), "S(p\u{e9})");
}

#[test]
fn ceilings_stop_scanning() {
    let mut short = Tokenizer::new(3, 3, 1);
    short.has_encoding = true;
    let too_long: &[&[u8]] = &[
        b"<!--abcd-->",
        b"<a b='abcd'>",
        b"<!x abcd>",
        b"<!DOCTYPE abcd>",
    ];
    for input in too_long {
        assert_eq!(short.next(input, true).err(), Some(Stop::TooLong));
    }
    assert!(short.next(b"<!--abc-->", true).is_ok());
    assert_eq!(short.next(b"<a b c>", true).err(), Some(Stop::Attributes));
    assert!(short.next(b"<a b>", true).is_ok());
}

/// Where libxml2's input is when it reports the first invalid byte: the
/// start of the text chunk, value, comment or name being scanned.
#[test]
fn invalid_bytes_are_reported_from_the_scan_start() {
    let cases: &[(&[u8], Option<usize>)] = &[
        (b"<p a=b>ok", None),
        (b"\xff", Some(0)),
        (b"x\xff", Some(0)),
        (b"<p>ab\xffc\xff", Some(3)),
        (b"<p a=\"x\xff\">", Some(6)),
        (b"<p a=x\xff>", Some(5)),
        (b"<p\xff>", Some(1)),
        (b"<!--a\xff-->", Some(4)),
        (b"  <!--a\xff-->", Some(6)),
    ];
    for &(input, expected) in cases {
        let mut tokenizer = tokenizer(Mode::Data, "");
        tokenizer.fragment();
        let (mut position, mut first) = (0, None);
        while let Some((token, used)) = tokenizer.next(&input[position..], true).unwrap() {
            if let Some(token) = token {
                react(&mut tokenizer, &token);
            }
            first = first.or(tokenizer.invalid.take().map(|at| position + at));
            position += used;
        }
        assert_eq!(first, expected, "{input:?}");
    }
}

#[test]
fn only_the_prolog_skips_blanks() {
    let mut tokenizer = tokenizer(Mode::Data, "");
    let (token, used) = tokenizer.next(b" \n<!--c-->", true).unwrap().unwrap();
    assert_eq!((token.is_some(), used, tokenizer.skipped), (true, 10, 2));
    tokenizer.skipped = 0;
    assert!(tokenizer.next(b"<p>", true).unwrap().is_some());
    assert!(tokenizer.next(b" x", true).unwrap().is_some());
    assert_eq!(tokenizer.skipped, 0);
    let mut fragment = super::Tokenizer::new(1 << 20, 50_000, 64);
    fragment.fragment();
    let (_, used) = fragment.next(b" <!--c-->", true).unwrap().unwrap();
    assert_eq!((used, fragment.skipped), (1, 0));
}

#[test]
fn utf8_validation_matches_htmlvalidateutf8() {
    let cases: &[(&[u8], Option<usize>)] = &[
        (b"\xc2\x80", Some(2)),
        (b"\xc1\x80", None),
        (b"\xe0\xa0\x80", Some(3)),
        (b"\xe0\x9f\x80", None),
        (b"\xed\x9f\xbf", Some(3)),
        (b"\xed\xa0\x80", None),
        (b"\xf0\x90\x80\x80", Some(4)),
        (b"\xf0\x8f\xbf\xbf", None),
        (b"\xf4\x8f\xbf\xbf", Some(4)),
        (b"\xf4\x90\x80\x80", None),
        (b"\xc3", None),
    ];
    for (bytes, expected) in cases {
        assert_eq!(utf8(bytes, 0, true), Ok(*expected), "{bytes:?}");
    }
    assert_eq!(utf8(b"\xe2\x82", 0, false), Err(Stop::More));
}
