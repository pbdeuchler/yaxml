use super::*;

#[test]
fn decode_normalizes_supported_resources_and_rejects_malformed_ones() {
    let utf16: Vec<u8> = [0xff, 0xfe]
        .into_iter()
        .chain(
            "<?xml encoding='UTF-16'?>\u{e9}<a/>"
                .encode_utf16()
                .flat_map(u16::to_le_bytes),
        )
        .collect();
    let cases: [(&[u8], Result<&str, ErrorKind>); 16] = [
        (b"plain <a/>", Ok("plain <a/>")),
        (b"", Ok("")),
        (b"\xef\xbb\xbfbom", Ok("bom")),
        (b"<?xml version='1.0' encoding='UTF-8'?>text", Ok("text")),
        (b"<?xml encoding='utf-8' ?>text", Ok("text")),
        (b"a\r\nb\rc\n", Ok("a\nb\nc\n")),
        (&utf16, Ok("\u{e9}<a/>")),
        (b"<?xml encoding='ISO-8859-1'?>\xe9", Ok("\u{e9}")),
        (b"<?xmlfoo?>", Ok("<?xmlfoo?>")),
        (b"\xff", Err(ErrorKind::InvalidUtf8)),
        (b"\x01", Err(ErrorKind::InvalidCharacter)),
        (
            b"<?xml version='1.0'?>x",
            Err(ErrorKind::InvalidDeclaration),
        ),
        (
            b"<?xml encoding='UTF-8' standalone='yes'?>",
            Err(ErrorKind::InvalidDeclaration),
        ),
        (
            b"<?xml encoding='UTF-8'",
            Err(ErrorKind::InvalidDeclaration),
        ),
        (
            b"<?xml encoding='bogus'?>",
            Err(ErrorKind::UnsupportedEncoding),
        ),
        (
            b"<?xml encoding='US-ASCII'?>\x80",
            Err(ErrorKind::UnmappedByte),
        ),
    ];
    for (input, expected) in cases {
        assert_eq!(decode(input).as_deref(), expected.as_deref(), "{input:?}");
    }
    assert_eq!(decode(&[0xff, 0xfe, b'a']), Err(ErrorKind::InvalidUtf16));
}

fn loader(limits: Limits, responses: Vec<io::Result<Option<Vec<u8>>>>) -> Loader {
    let mut responses = responses.into_iter();
    let resolver = move |_: Request<'_>| responses.next().expect("scripted response");
    Loader::new(
        "dir/doc.xml",
        Box::new(resolver),
        &limits,
        DtdLimits::default(),
    )
}

#[test]
fn loader_counts_requests_and_bytes_across_loads() {
    let limits = Limits {
        max_external_loads: 2,
        max_external_bytes: 5,
        ..Limits::default()
    };
    let mut ok = loader(limits, vec![Ok(Some(b"abc".to_vec())), Ok(None)]);
    assert_eq!(ok.uri("e.ent"), "dir/e.ent");
    assert_eq!(
        ok.load(ResourceKind::GeneralEntity, "u", None),
        Ok(Some("abc".into()))
    );
    assert_eq!(ok.load(ResourceKind::GeneralEntity, "u", None), Ok(None));
    assert_eq!(
        ok.load(ResourceKind::GeneralEntity, "u", None),
        Err(ErrorKind::LimitExceeded(Limit::ExternalLoads))
    );
    let mut large = loader(
        limits,
        vec![Ok(Some(b"abc".to_vec())), Ok(Some(b"def".to_vec()))],
    );
    assert!(large.load(ResourceKind::DtdSubset, "u", None).is_ok());
    assert_eq!(
        large.load(ResourceKind::DtdSubset, "u", None),
        Err(ErrorKind::LimitExceeded(Limit::ExternalBytes))
    );
    let mut failing = loader(limits, vec![Err(io::Error::other("denied"))]);
    assert_eq!(
        failing.load(ResourceKind::ParameterEntity, "u", None),
        Err(ErrorKind::ExternalResource)
    );
    let mut malformed = loader(limits, vec![Ok(Some(vec![0xff]))]);
    assert_eq!(
        malformed.load(ResourceKind::ParameterEntity, "u", None),
        Err(ErrorKind::InvalidUtf8)
    );
}
