use crate::Limits;
use crate::dtd::{DtdLimits, DtdUnit};
use crate::dtd_catalog::{
    CatalogLimits, DeclarationContext, DtdCatalog, EntityLookup, LiteralSource,
};
use crate::parser::ParserState;

use super::*;

fn context(literals: LiteralSource) -> DeclarationContext {
    DeclarationContext {
        internal_subset: true,
        literals,
    }
}

#[test]
fn a_parameter_unit_can_own_its_text_before_mutating_its_source_catalog() {
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    // Numeric CR is constructed while declaring p. It is ordinary replacement
    // text when the generated declaration of g is processed, not physical EOL.
    let declaration = DtdUnit::parse(
        "<!ENTITY % p \"<!ENTITY g '&#13;'>\">",
        DtdLimits::default(),
    )
    .unwrap();
    catalog
        .apply(&declaration, context(LiteralSource::Physical))
        .unwrap();
    let (id, length) = match catalog.lookup_parameter("p") {
        EntityLookup::Internal { id, replacement } => {
            assert_eq!(replacement, "<!ENTITY g '\r'>");
            (id, replacement.len())
        }
        actual => panic!("unexpected lookup: {actual:?}"),
    };
    let declaration_id = EntityDeclarationId::from_index(id.index());
    let mut frames = EntityFrames::new(FrameLimits::default());
    frames
        .enter_physical(
            EntityKind::Parameter,
            EntityResolution::Internal {
                kind: EntityKind::Parameter,
                declaration: declaration_id,
                replacement_len: length,
            },
            100..103,
            FrameBoundary::DtdSubset,
        )
        .unwrap();
    let mut state = ParserState::new(
        Limits {
            max_owned_bytes: length,
            ..Limits::default()
        },
        0,
    );
    let generated = {
        let replacement = catalog.replacement(id).unwrap();
        let source = frames.current(declaration_id, replacement).unwrap();
        let borrowed = DtdUnit::parse(source.remaining, DtdLimits::default()).unwrap();
        borrowed
            .try_into_owned(state.owned_budget(), source.physical_reference.start)
            .unwrap()
    };
    // This mutable call is possible because generated owns its raw text. No
    // catalog borrow or replacement pointer survives the handoff.
    catalog
        .apply(&generated, context(LiteralSource::Replacement))
        .unwrap();
    match catalog.lookup_general("g") {
        EntityLookup::Internal { replacement, .. } => assert_eq!(replacement, "\r"),
        actual => panic!("unexpected generated binding: {actual:?}"),
    }
    assert_eq!(catalog.declaration(id), Some(0));
    let source = frames
        .current(declaration_id, catalog.replacement(id).unwrap())
        .unwrap();
    assert_eq!(source.local_start, 0);
    assert_eq!(source.remaining, "<!ENTITY g '\r'>");
    let origin = frames
        .advance_current(declaration_id, catalog.replacement(id).unwrap(), length)
        .unwrap();
    assert_eq!(origin.input_range(), None);
    assert_eq!(origin.replacement_range(), Some(0..length));
    assert_eq!(origin.physical_context(), 100..103);
    frames.finish_current(FrameBoundary::DtdSubset).unwrap();
    assert_eq!(frames.depth(), 0);
    assert_eq!(state.owned_budget().remaining_owned(), 0);
}

#[test]
fn catalog_ids_keep_general_and_parameter_names_distinct_for_frames() {
    let mut catalog = DtdCatalog::new(CatalogLimits::default(), false);
    for raw in ["<!ENTITY % x 'P'>", "<!ENTITY x 'G'>"] {
        catalog
            .apply(
                &DtdUnit::parse(raw, DtdLimits::default()).unwrap(),
                context(LiteralSource::Physical),
            )
            .unwrap();
    }
    let EntityLookup::Internal { id: parameter, .. } = catalog.lookup_parameter("x") else {
        panic!("parameter binding missing");
    };
    let EntityLookup::Internal { id: general, .. } = catalog.lookup_general("x") else {
        panic!("general binding missing");
    };
    assert_ne!(general, parameter);
    assert_eq!(catalog.declaration(parameter), Some(0));
    assert_eq!(catalog.declaration(general), Some(1));
    let mut frames = EntityFrames::new(FrameLimits::default());
    let mut invocations = Vec::new();
    for (kind, id, boundary, reference, expected) in [
        (
            EntityKind::Parameter,
            parameter,
            FrameBoundary::DtdSubset,
            10..13,
            "P",
        ),
        (
            EntityKind::General,
            general,
            FrameBoundary::Content { element_depth: 1 },
            20..23,
            "G",
        ),
        (
            EntityKind::General,
            general,
            FrameBoundary::Content { element_depth: 1 },
            30..33,
            "G",
        ),
    ] {
        let replacement = catalog.replacement(id).unwrap();
        let declaration = EntityDeclarationId::from_index(id.index());
        let invocation = frames
            .enter_physical(
                kind,
                EntityResolution::Internal {
                    kind,
                    declaration,
                    replacement_len: replacement.len(),
                },
                reference.clone(),
                boundary,
            )
            .unwrap();
        invocations.push(invocation.get());
        assert_eq!(
            frames.current(declaration, replacement).unwrap().remaining,
            expected
        );
        let origin = frames.advance_current(declaration, replacement, 1).unwrap();
        assert_eq!(origin.replacement_range(), Some(0..1));
        assert_eq!(origin.physical_context(), reference);
        let finished = frames.finish_current(boundary).unwrap();
        assert_eq!(finished.declaration, declaration);
        assert_eq!(finished.invocation, invocation);
    }
    assert_eq!(invocations, [1, 2, 3]);
    assert_eq!(frames.counters().reference_attempts, 3);
    assert_eq!(frames.counters().expanded_bytes, 3);
    assert_eq!(frames.counters().work, 6);
}
