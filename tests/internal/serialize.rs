use super::*;

#[test]
fn views_borrow_normalized_names_and_attributes_and_reuse_metadata() {
    let document = Document::parse("<p:r xmlns:p='urn:&#114;' a='x&#9;y'><c/></p:r>").unwrap();
    let mut attributes = Vec::new();
    let root = TreeNode::Borrowed(document.root_element());
    let name = root.name().unwrap();
    assert_eq!(name.namespace(), Some("urn:r"));
    assert_eq!(
        name.namespace().unwrap().as_ptr(),
        document
            .root_element()
            .name()
            .unwrap()
            .namespace()
            .unwrap()
            .as_ptr()
    );
    root.attributes(&mut attributes, 2, 7).unwrap();
    assert_eq!(attributes.len(), 2);
    assert!(matches!(attributes[1].value, Cow::Borrowed("x\ty")));
    let allocation = attributes.as_ptr();
    root.attributes(&mut attributes, 2, 8).unwrap();
    assert_eq!(attributes.as_ptr(), allocation);
    assert_eq!(
        root.attributes(&mut attributes, 1, 9).unwrap_err(),
        Error::new(ErrorKind::LimitExceeded(Limit::Attributes), 9)
    );
    assert!(attributes.is_empty());
    root.first_child()
        .unwrap()
        .attributes(&mut attributes, 0, 10)
        .unwrap();
    assert!(attributes.is_empty());
    assert!(root.leaf().is_none());
    assert!(TreeNode::Borrowed(document.root()).name().is_none());
    assert!(TreeNode::Borrowed(document.root()).leaf().is_none());

    let owned = OwnedDocument::parse("<r a='v'/>").unwrap();
    let node = TreeNode::Owned(owned.root_element());
    assert!(node.leaf().is_none());
    assert!(TreeNode::Owned(owned.root()).leaf().is_none());
    node.attributes(&mut attributes, 1, 0).unwrap();
    assert_eq!(attributes[0].value, "v");
    assert!(matches!(attributes[0].value, Cow::Borrowed(_)));
    assert_eq!(
        node.attributes(&mut attributes, 0, 11).unwrap_err(),
        Error::new(ErrorKind::LimitExceeded(Limit::Attributes), 11)
    );
}

#[test]
fn both_storage_forms_visit_all_kinds_and_close_ancestors_once() {
    let input = "<?xml version='1.0' encoding='utf-8' standalone='yes'?><?before?><!--p--><r xmlns='u' a='&amp;'><a><b>text</b><c/></a><![CDATA[]]><!--c--><?pi data?><e></e></r><!--after--><?end?>";
    let expected = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><?before?><!--p--><r xmlns=\"u\" a=\"&amp;\"><a><b>text</b><c/></a><![CDATA[]]><!--c--><?pi data?><e/></r><!--after--><?end?>";
    let document = Document::parse(input).unwrap();
    let owned = OwnedDocument::parse(input).unwrap();
    let mut borrowed_writer = Writer::new(Vec::new());
    borrowed_writer.write_document(&document).unwrap();
    assert_eq!(borrowed_writer.position(), expected.len());
    borrowed_writer.finish().unwrap();
    assert_eq!(borrowed_writer.into_inner(), expected.as_bytes());
    let mut owned_writer = Writer::new(Vec::new());
    owned_writer.write_owned_document(&owned).unwrap();
    owned_writer.finish().unwrap();
    assert_eq!(owned_writer.into_inner(), expected.as_bytes());
}

#[test]
fn attribute_limit_precedes_growth_and_any_output() {
    let document = Document::parse("<r a='v'/>").unwrap();
    let mut writer = Writer::with_limits(
        Vec::new(),
        crate::WriterLimits {
            max_attributes: 0,
            ..Default::default()
        },
    );
    assert!(matches!(
        writer.write_document(&document),
        Err(WriteError::Xml(Error {
            kind: ErrorKind::LimitExceeded(Limit::Attributes),
            offset: 0
        }))
    ));
    assert_eq!(writer.position(), 0);
    assert!(matches!(
        writer.finish(),
        Err(WriteError::Xml(Error {
            kind: ErrorKind::InvalidState,
            ..
        }))
    ));
    assert!(writer.into_inner().is_empty());
}
