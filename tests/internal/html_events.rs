use super::*;

/// libxml2's tokenizer mode numbers (`DATA_*` in include/private/html.h).
fn mode(state: u32) -> tables::Mode {
    use tables::Mode;
    match state {
        1 => Mode::Rcdata,
        2 => Mode::Rawtext,
        3 => Mode::Plaintext,
        4 => Mode::Script,
        5 => Mode::ScriptEsc1,
        6 => Mode::ScriptEsc2,
        _ => Mode::Data,
    }
}

/// runtest.c's htmlTokenizerTest output for one input: the HTML5 tokenizer
/// in `state` after `start_tag`, with its SAX callbacks printed.
fn tokenize(input: &[u8], state: u32, start_tag: &str) -> String {
    let mut out = String::new();
    let mut characters = false;
    let handler = |event: Event<'_>| {
        let text = match event {
            Event::Text(text) | Event::Cdata(text) => Some(text),
            _ => None,
        };
        if let Some(text) = text {
            if !characters {
                out.push_str("Character\n");
                characters = true;
            }
            out.push_str(text);
            return ControlFlow::Continue(());
        }
        if characters {
            out.push('\n');
            characters = false;
        }
        match event {
            Event::Doctype(doctype) => {
                let none = |value: Option<&str>| value.unwrap_or("<none>").to_owned();
                let name = Some(doctype.name).filter(|name| !name.is_empty());
                out += &format!(
                    "DOCTYPE\n{}\n{}\n{}\n",
                    none(name),
                    none(doctype.public_id),
                    none(doctype.system_id)
                );
            }
            Event::Start { name, attributes } => {
                out += &format!("StartTag\n{name}");
                for (name, value) in attributes {
                    out += &format!(" {name}={}", value.as_deref().unwrap_or(""));
                }
                out.push('\n');
            }
            Event::End(name) => out += &format!("EndTag\n{name}\n"),
            Event::Comment(text) => out += &format!("Comment\n{text}\n"),
            _ => {}
        }
        ControlFlow::Continue(())
    };
    let mut engine = Engine::new(Options::new(HTML5), Events::new(handler), false).unwrap();
    engine.tokenizer.fragment();
    engine.tokenizer.has_encoding = true;
    engine.tokenizer.enter(mode(state), start_tag);
    engine.begin(Decoder::Utf8, Cow::Borrowed(input), 0, true);
    engine.run(true).unwrap();
    drop(engine.summary());
    if characters {
        out.push('\n');
    }
    out
}

/// Records of a test/html-tokenizer file: number, last start tag, state, input.
fn records(bytes: &[u8]) -> Vec<(usize, String, u32, Vec<u8>)> {
    let mut records = Vec::new();
    let mut rest = bytes;
    while let Some(newline) = rest.iter().position(|&c| c == b'\n') {
        let header = String::from_utf8_lossy(&rest[..newline]).into_owned();
        let fields: Vec<&str> = header.split(' ').collect();
        let [number, tag, state, length] = fields[..] else {
            break;
        };
        let (Ok(number), Ok(state), Ok(length)) =
            (number.parse(), state.parse(), length.parse::<usize>())
        else {
            break;
        };
        let start = newline + 1;
        let end = start + length;
        records.push((number, tag.to_owned(), state, rest[start..end].to_vec()));
        rest = &rest[(end + 1).min(rest.len())..];
    }
    records
}

/// libxml2 runs html5lib's tokenizer tests through `HTML_PARSE_HTML5` with
/// custom SAX callbacks (runtest.c, result/html-tokenizer); every record must
/// produce the same callbacks here.
#[test]
fn html5_tokenizer_matches_libxml2_results() {
    let Some(source) = std::env::var_os("YAXML_LIBXML2_SOURCE") else {
        eprintln!("YAXML_LIBXML2_SOURCE unset; skipping libxml2's html-tokenizer results");
        return;
    };
    let source = std::path::Path::new(&source);
    let mut paths: Vec<_> = std::fs::read_dir(source.join("test/html-tokenizer"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "test"))
        .collect();
    paths.sort();
    let (mut total, mut failures) = (0, Vec::new());
    for path in paths {
        let expected = std::fs::read(
            source
                .join("result/html-tokenizer")
                .join(path.file_name().unwrap()),
        )
        .unwrap();
        let expected = String::from_utf8_lossy(&expected).into_owned();
        let mut at = 0;
        for (index, (number, tag, state, input)) in records(&std::fs::read(&path).unwrap())
            .into_iter()
            .enumerate()
        {
            total += 1;
            let ours = format!("{index}\n{}", tokenize(&input, state, &tag));
            let next = format!("\n{}\n", index + 1);
            let end = expected[at..]
                .find(&next)
                .map_or(expected.len(), |p| at + p + 1);
            if expected[at..end] != ours {
                failures.push(format!(
                    "{} #{number} state {state}: {:?}\n  ours:   {ours:?}\n  libxml2: {:?}",
                    path.display(),
                    String::from_utf8_lossy(&input),
                    &expected[at..end]
                ));
            }
            at = end;
        }
    }
    eprintln!(
        "html-tokenizer: {}/{total} records match libxml2",
        total - failures.len()
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
