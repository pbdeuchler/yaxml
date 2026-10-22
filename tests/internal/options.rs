use super::*;

#[test]
fn check_accepts_supported_flags_and_rejects_the_rest() {
    let cases: &[(u32, bool, u32, bool)] = &[
        // (flags, tree surface, surface exclusions, accepted)
        (0, false, 0, true),
        (
            NOENT | DTDLOAD | DTDATTR | NO_XXE | NOCDATA | NSCLEAN,
            false,
            0,
            true,
        ),
        (
            NOERROR | NOWARNING | PEDANTIC | SAX1 | NONET | NODICT | COMPACT,
            false,
            0,
            true,
        ),
        (
            NOXINCNODE | NOBASEFIX | HUGE | IGNORE_ENC | BIG_LINES,
            false,
            0,
            true,
        ),
        (NO_SYS_CATALOG | SKIP_IDS, false, 0, true),
        (DTDVALID | NOBLANKS, true, 0, true),
        (XINCLUDE, true, 0, cfg!(feature = "xinclude")),
        (DTDVALID, false, 0, false),
        (NOBLANKS, false, 0, true),
        (NOBLANKS, false, NOBLANKS, false),
        (XINCLUDE, false, 0, false),
        (XINCLUDE, true, XINCLUDE, false),
        (RECOVER, true, 0, true),
        (RECOVER, false, 0, true),
        (OLD10, true, 0, true),
        (OLDSAX, true, 0, false),
        (UNZIP, true, 0, false),
        (CATALOG_PI, true, 0, cfg!(feature = "catalog")),
        (CATALOG_PI, false, 0, cfg!(feature = "catalog")),
        (1 << 28, true, 0, false),
        (1 << 31, false, 0, false),
    ];
    for &(flags, tree, unsupported, accepted) in cases {
        let result = ParseOptions::new(flags).check(tree, unsupported);
        match accepted {
            true => assert_eq!(result, Ok(Flags(flags)), "{flags:#x}"),
            false => assert_eq!(
                result.map_err(|error| error.kind),
                Err(ErrorKind::UnsupportedOption),
                "{flags:#x}"
            ),
        }
    }
}

#[test]
fn loading_follows_libxml2() {
    use ResourceKind::{DtdSubset, GeneralEntity, ParameterEntity, XInclude};
    // (flags, subset, parameter entity, general entity)
    let cases: &[(u32, bool, bool, bool)] = &[
        (0, false, false, false),
        (DTDLOAD, true, true, false),
        (DTDATTR, true, true, false),
        (NOENT, false, true, true),
        (DTDVALID, true, true, true),
        (DTDLOAD | NOENT, true, true, true),
        (DTDLOAD | NOENT | NO_XXE, false, false, false),
    ];
    for &(flags, subset, parameter, general) in cases {
        let flags = Flags(flags);
        assert_eq!(flags.loads(DtdSubset), subset, "{flags:?}");
        assert_eq!(flags.loads(ParameterEntity), parameter, "{flags:?}");
        assert_eq!(flags.loads(GeneralEntity), general, "{flags:?}");
        assert!(flags.loads(XInclude));
    }
}

#[test]
fn huge_raises_only_libxml2_limits() {
    let normal = ParseOptions::default();
    assert_eq!(
        normal.effective_limits().0,
        Limits::default(),
        "no flag, no change"
    );
    let (limits, decode) = ParseOptions::new(HUGE).effective_limits();
    assert_eq!(limits.max_token_bytes, 1_000_000_000);
    assert_eq!(limits.max_depth, 2048);
    assert_eq!(limits.max_entity_depth, 40);
    assert_eq!(decode.max_decoded_token_bytes, 1_000_000_000);
    assert_eq!(limits.max_input_bytes, Limits::default().max_input_bytes);
    // A caller's larger ceiling survives.
    let wide = Limits {
        max_depth: 5000,
        ..Limits::default()
    };
    let options = ParseOptions::new(HUGE).with_limits(wide);
    assert_eq!(options.effective_limits().0.max_depth, 5000);
}

#[test]
fn options_debug_hides_the_resolver() {
    let resolver = |_: crate::Request<'_>| Ok(None);
    let options = ParseOptions::new(NOENT).with_resolver("base.xml", Box::new(resolver));
    let text = format!("{options:?}");
    assert!(text.contains("0x2") && text.contains("base.xml") && text.contains(".."));
    assert!(format!("{:?}", ParseOptions::default()).contains("resolver: None"));
}
