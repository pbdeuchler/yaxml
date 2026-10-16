use super::*;

#[derive(Clone, Copy)]
struct LocalBinding<'a> {
    prefix: Option<&'a str>,
    uri: &'a str,
}

struct GeneratedBinding {
    prefix: Option<String>,
    uri: String,
}

struct StagedName {
    qualified: Range<usize>,
    local: Range<usize>,
    prefix_length: Option<NonZeroUsize>,
    namespace: Option<Range<usize>>,
}

struct StagedAttribute {
    name: StagedName,
    value: Range<usize>,
    is_declaration: bool,
}

struct StagedElement {
    strings: String,
    name: StagedName,
    attributes: Vec<StagedAttribute>,
}

struct Work {
    used: usize,
    limit: usize,
}

impl Work {
    fn charge(&mut self, amount: usize) -> Result<(), EditError> {
        let next = self
            .used
            .checked_add(amount)
            .ok_or_else(namespace_work_error)?;
        if next > self.limit {
            return Err(namespace_work_error());
        }
        self.used = next;
        Ok(())
    }

    fn compare(&mut self, left: &str, right: &str) -> Result<bool, EditError> {
        self.charge(
            left.len()
                .checked_add(right.len())
                .ok_or_else(namespace_work_error)?,
        )?;
        Ok(left == right)
    }
}

fn allocation_error() -> EditError {
    EditError::new(EditErrorKind::Allocation, None)
}

fn namespace_work_error() -> EditError {
    EditError::new(EditErrorKind::LimitExceeded(EditLimit::NamespaceWork), None)
}

fn location(component: EditComponent, offset: Option<usize>) -> EditLocation {
    EditLocation::ElementName {
        component,
        byte_offset: offset,
    }
}

fn item_location(
    index: usize,
    namespace: bool,
    component: Option<EditComponent>,
    offset: Option<usize>,
) -> EditLocation {
    if namespace {
        EditLocation::NamespaceItem {
            index,
            component,
            byte_offset: offset,
        }
    } else {
        EditLocation::AttributeItem {
            index,
            component,
            byte_offset: offset,
        }
    }
}

fn located(kind: EditErrorKind, component: EditComponent, offset: Option<usize>) -> EditError {
    EditError::located(kind, location(component, offset))
}

fn located_item(
    kind: EditErrorKind,
    index: usize,
    namespace: bool,
    component: Option<EditComponent>,
    offset: Option<usize>,
) -> EditError {
    EditError::located(kind, item_location(index, namespace, component, offset))
}

fn limit_error(limit: EditLimit, component: EditComponent, offset: usize) -> EditError {
    located(EditErrorKind::LimitExceeded(limit), component, Some(offset))
}

fn limit_item_error(
    limit: EditLimit,
    index: usize,
    namespace: bool,
    component: EditComponent,
    offset: usize,
) -> EditError {
    located_item(
        EditErrorKind::LimitExceeded(limit),
        index,
        namespace,
        Some(component),
        Some(offset),
    )
}

fn check_length(
    value: &str,
    component: EditComponent,
    item: Option<(usize, bool)>,
    limit: usize,
    supplied: &mut usize,
    limits: EditLimits,
) -> Result<(), EditError> {
    if value.len() > limit {
        return Err(match item {
            Some((index, namespace)) => {
                limit_item_error(EditLimit::NameBytes, index, namespace, component, limit)
            }
            None => limit_error(EditLimit::NameBytes, component, limit),
        });
    }
    let next = supplied
        .checked_add(value.len())
        .ok_or_else(|| match item {
            Some((index, namespace)) => limit_item_error(
                EditLimit::SuppliedBytes,
                index,
                namespace,
                component,
                limits.max_supplied_bytes.saturating_sub(*supplied),
            ),
            None => limit_error(
                EditLimit::SuppliedBytes,
                component,
                limits.max_supplied_bytes.saturating_sub(*supplied),
            ),
        })?;
    if next > limits.max_supplied_bytes {
        return Err(match item {
            Some((index, namespace)) => limit_item_error(
                EditLimit::SuppliedBytes,
                index,
                namespace,
                component,
                limits.max_supplied_bytes - *supplied,
            ),
            None => limit_error(
                EditLimit::SuppliedBytes,
                component,
                limits.max_supplied_bytes - *supplied,
            ),
        });
    }
    *supplied = next;
    Ok(())
}

fn check_value_length(
    value: &str,
    index: usize,
    supplied: &mut usize,
    limits: EditLimits,
) -> Result<(), EditError> {
    if value.len() > limits.max_value_bytes {
        return Err(limit_item_error(
            EditLimit::ValueBytes,
            index,
            false,
            EditComponent::Value,
            limits.max_value_bytes,
        ));
    }
    let next = supplied.checked_add(value.len()).ok_or_else(|| {
        limit_item_error(
            EditLimit::SuppliedBytes,
            index,
            false,
            EditComponent::Value,
            limits.max_supplied_bytes.saturating_sub(*supplied),
        )
    })?;
    if next > limits.max_supplied_bytes {
        return Err(limit_item_error(
            EditLimit::SuppliedBytes,
            index,
            false,
            EditComponent::Value,
            limits.max_supplied_bytes - *supplied,
        ));
    }
    *supplied = next;
    Ok(())
}

fn check_spec_lengths(spec: &ElementSpec<'_>, limits: EditLimits) -> Result<usize, EditError> {
    let mut total = 0;
    let mut name = |part: &str, component| {
        check_length(
            part,
            component,
            None,
            limits.max_name_bytes,
            &mut total,
            limits,
        )
    };
    name(spec.name.local, EditComponent::LocalName)?;
    if let Some(uri) = spec.name.namespace {
        name(uri, EditComponent::NamespaceUri)?;
    }
    if let PrefixPolicy::Exact(Some(prefix)) = spec.name.prefix {
        name(prefix, EditComponent::Prefix)?;
    }
    for (index, item) in spec.items.iter().enumerate() {
        match item {
            ElementItem::Namespace(declaration) => {
                if let Some(prefix) = declaration.prefix {
                    check_length(
                        prefix,
                        EditComponent::Prefix,
                        Some((index, true)),
                        limits.max_name_bytes,
                        &mut total,
                        limits,
                    )?;
                }
                check_length(
                    declaration.uri,
                    EditComponent::NamespaceUri,
                    Some((index, true)),
                    limits.max_name_bytes,
                    &mut total,
                    limits,
                )?;
            }
            ElementItem::Attribute(attribute) => {
                check_length(
                    attribute.name.local,
                    EditComponent::LocalName,
                    Some((index, false)),
                    limits.max_name_bytes,
                    &mut total,
                    limits,
                )?;
                if let Some(uri) = attribute.name.namespace {
                    check_length(
                        uri,
                        EditComponent::NamespaceUri,
                        Some((index, false)),
                        limits.max_name_bytes,
                        &mut total,
                        limits,
                    )?;
                }
                if let PrefixPolicy::Exact(Some(prefix)) = attribute.name.prefix {
                    check_length(
                        prefix,
                        EditComponent::Prefix,
                        Some((index, false)),
                        limits.max_name_bytes,
                        &mut total,
                        limits,
                    )?;
                }
                check_value_length(attribute.value, index, &mut total, limits)?;
            }
        }
    }
    Ok(total)
}

fn validate_part(
    value: &str,
    component: EditComponent,
    item: Option<(usize, bool)>,
    is_name: bool,
) -> Result<(), EditError> {
    if let Some((offset, _)) = value
        .char_indices()
        .find(|(_, c)| !crate::escape::xml_char(*c))
    {
        return Err(match item {
            Some((index, namespace)) => located_item(
                EditErrorKind::InvalidCharacter,
                index,
                namespace,
                Some(component),
                Some(offset),
            ),
            None => located(EditErrorKind::InvalidCharacter, component, Some(offset)),
        });
    }
    if is_name {
        crate::parser::validate_name(value, 0, false).map_err(|error| match item {
            Some((index, namespace)) => located_item(
                EditErrorKind::InvalidName,
                index,
                namespace,
                Some(component),
                Some(error.offset),
            ),
            None => located(EditErrorKind::InvalidName, component, Some(error.offset)),
        })?;
    }
    Ok(())
}

fn validate_spec(spec: &ElementSpec<'_>) -> Result<(), EditError> {
    validate_part(spec.name.local, EditComponent::LocalName, None, true)?;
    if let Some(uri) = spec.name.namespace {
        validate_part(uri, EditComponent::NamespaceUri, None, false)?;
    }
    if let PrefixPolicy::Exact(Some(prefix)) = spec.name.prefix {
        validate_part(prefix, EditComponent::Prefix, None, true)?;
    }
    for (index, item) in spec.items.iter().enumerate() {
        match item {
            ElementItem::Namespace(declaration) => {
                if let Some(prefix) = declaration.prefix {
                    validate_part(prefix, EditComponent::Prefix, Some((index, true)), true)?;
                }
                validate_part(
                    declaration.uri,
                    EditComponent::NamespaceUri,
                    Some((index, true)),
                    false,
                )?;
            }
            ElementItem::Attribute(attribute) => {
                validate_part(
                    attribute.name.local,
                    EditComponent::LocalName,
                    Some((index, false)),
                    true,
                )?;
                if let Some(uri) = attribute.name.namespace {
                    validate_part(
                        uri,
                        EditComponent::NamespaceUri,
                        Some((index, false)),
                        false,
                    )?;
                }
                if let PrefixPolicy::Exact(Some(prefix)) = attribute.name.prefix {
                    validate_part(prefix, EditComponent::Prefix, Some((index, false)), true)?;
                }
                if let Some((offset, _)) = attribute
                    .value
                    .char_indices()
                    .find(|(_, c)| !crate::escape::xml_char(*c))
                {
                    return Err(located_item(
                        EditErrorKind::InvalidCharacter,
                        index,
                        false,
                        Some(EditComponent::Value),
                        Some(offset),
                    ));
                }
            }
        }
    }
    validate_reserved_declarations(spec)
}

fn validate_reserved_declarations(spec: &ElementSpec<'_>) -> Result<(), EditError> {
    for (index, item) in spec.items.iter().enumerate() {
        let ElementItem::Namespace(declaration) = item else {
            continue;
        };
        let invalid_prefix = declaration.prefix == Some("xmlns");
        let invalid_uri = declaration.uri == XMLNS
            || (declaration.prefix == Some("xml") && declaration.uri != XML)
            || (declaration.prefix != Some("xml") && declaration.uri == XML)
            || (declaration.prefix.is_some() && declaration.uri.is_empty());
        if invalid_prefix {
            return Err(located_item(
                EditErrorKind::Namespace,
                index,
                true,
                Some(EditComponent::Prefix),
                None,
            ));
        }
        if invalid_uri {
            return Err(located_item(
                EditErrorKind::Namespace,
                index,
                true,
                Some(EditComponent::NamespaceUri),
                None,
            ));
        }
    }
    Ok(())
}

fn reserve_vec<T>(items: &mut Vec<T>, amount: usize) -> Result<(), EditError> {
    items
        .try_reserve_exact(amount)
        .map_err(|_| allocation_error())
}

fn push_string(target: &mut String, value: &str) -> Result<(), EditError> {
    target
        .try_reserve_exact(value.len())
        .map_err(|_| allocation_error())?;
    target.push_str(value);
    Ok(())
}

fn clone_string(value: &str) -> Result<String, EditError> {
    let mut result = String::new();
    push_string(&mut result, value)?;
    Ok(result)
}

fn declaration_prefix<'a>(name: &Name<'a>) -> Option<Option<&'a str>> {
    if name.namespace.as_deref() != Some(XMLNS) {
        return None;
    }
    if name.prefix == Some("xmlns") {
        Some(Some(name.local))
    } else if name.qualified == "xmlns" {
        Some(None)
    } else {
        None
    }
}

fn lookup<'a>(
    document: &'a OwnedDocument,
    parent: Option<usize>,
    prefix: Option<&'a str>,
    local: &'a [LocalBinding<'a>],
    generated: &'a [GeneratedBinding],
    work: &mut Work,
) -> Result<Option<&'a str>, EditError> {
    for binding in local {
        work.charge(1)?;
        let candidate = binding.prefix.unwrap_or("");
        let wanted = prefix.unwrap_or("");
        if work.compare(candidate, wanted)? {
            return Ok(Some(binding.uri));
        }
    }
    for binding in generated {
        work.charge(1)?;
        let candidate = binding.prefix.as_deref().unwrap_or("");
        let wanted = prefix.unwrap_or("");
        if work.compare(candidate, wanted)? {
            return Ok(Some(&binding.uri));
        }
    }
    let mut current = parent;
    while let Some(index) = current {
        work.charge(1)?;
        let record = document.nodes.get(index).ok_or_else(allocation_error)?;
        let Content::Element { attributes, .. } = &record.content else {
            current = record.parent.get().map(|node| node.0);
            continue;
        };
        for attribute_index in attributes.clone() {
            work.charge(1)?;
            let attribute = &document.attributes[attribute_index];
            let name = attribute.name.view(&document.pool.0);
            let Some(candidate_prefix) = declaration_prefix(&name) else {
                continue;
            };
            work.charge(1)?;
            if work.compare(candidate_prefix.unwrap_or(""), prefix.unwrap_or(""))? {
                let uri = &document.pool.0[attribute.value.clone()];
                work.charge(uri.len())?;
                return Ok(Some(uri));
            }
        }
        current = record.parent.get().map(|node| node.0);
    }
    Ok(None)
}

fn lookup_explicit<'a>(
    local: &'a [LocalBinding<'a>],
    prefix: Option<&str>,
    work: &mut Work,
) -> Result<Option<&'a str>, EditError> {
    for binding in local {
        work.charge(1)?;
        if work.compare(binding.prefix.unwrap_or(""), prefix.unwrap_or(""))? {
            work.charge(binding.uri.len())?;
            return Ok(Some(binding.uri));
        }
    }
    Ok(None)
}

fn lookup_generated<'a>(
    generated: &'a [GeneratedBinding],
    prefix: Option<&str>,
    work: &mut Work,
) -> Result<Option<&'a str>, EditError> {
    for binding in generated {
        work.charge(1)?;
        if work.compare(
            binding.prefix.as_deref().unwrap_or(""),
            prefix.unwrap_or(""),
        )? {
            work.charge(binding.uri.len())?;
            return Ok(Some(&binding.uri));
        }
    }
    Ok(None)
}

fn ensure_binding(
    document: &OwnedDocument,
    parent: Option<usize>,
    local: &[LocalBinding<'_>],
    generated: &mut Vec<GeneratedBinding>,
    prefix: Option<&str>,
    uri: &str,
    exact: bool,
    work: &mut Work,
    error_location: EditLocation,
    supplied_items: usize,
    max_attributes: usize,
) -> Result<(), EditError> {
    if let Some(current) = lookup_explicit(local, prefix, work)? {
        if work.compare(current, uri)? {
            return Ok(());
        }
        return Err(EditError::located(EditErrorKind::Namespace, error_location));
    }
    if let Some(current) = lookup_generated(generated, prefix, work)? {
        if work.compare(current, uri)? {
            return Ok(());
        }
        return Err(EditError::located(EditErrorKind::Namespace, error_location));
    }
    if let Some(current) = lookup(document, parent, prefix, local, generated, work)? {
        if work.compare(current, uri)? {
            return Ok(());
        }
        if !exact {
            return Err(EditError::located(EditErrorKind::Namespace, error_location));
        }
    }
    let planned_count = supplied_items
        .checked_add(generated.len())
        .and_then(|count| count.checked_add(1))
        .ok_or_else(|| limit_global(EditLimit::Attributes))?;
    if planned_count > max_attributes {
        return Err(limit_global(EditLimit::Attributes));
    }
    generated.push(GeneratedBinding {
        prefix: prefix.map(clone_string).transpose()?,
        uri: clone_string(uri)?,
    });
    Ok(())
}

fn declaration_exists(
    local: &[LocalBinding<'_>],
    prefix: Option<&str>,
    work: &mut Work,
) -> Result<bool, EditError> {
    for binding in local {
        work.charge(1)?;
        if work.compare(binding.prefix.unwrap_or(""), prefix.unwrap_or(""))? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn exact_prefix_reserved(
    spec: &ElementSpec<'_>,
    candidate: &str,
    work: &mut Work,
) -> Result<bool, EditError> {
    let mut check = |policy: PrefixPolicy<'_>| -> Result<bool, EditError> {
        let PrefixPolicy::Exact(Some(prefix)) = policy else {
            return Ok(false);
        };
        work.charge(1)?;
        work.compare(prefix, candidate)
    };
    if check(spec.name.prefix)? {
        return Ok(true);
    }
    for item in spec.items {
        if let ElementItem::Attribute(attribute) = item {
            if check(attribute.name.prefix)? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn generated_prefix_available(
    document: &OwnedDocument,
    parent: Option<usize>,
    local: &[LocalBinding<'_>],
    generated: &[GeneratedBinding],
    prefix: &str,
    spec: &ElementSpec<'_>,
    work: &mut Work,
) -> Result<bool, EditError> {
    if exact_prefix_reserved(spec, prefix, work)?
        || declaration_exists(local, Some(prefix), work)?
        || lookup_generated(generated, Some(prefix), work)?.is_some()
    {
        return Ok(false);
    }
    Ok(lookup(document, parent, Some(prefix), local, generated, work)?.is_none())
}

fn next_prefix(
    document: &OwnedDocument,
    parent: Option<usize>,
    local: &[LocalBinding<'_>],
    generated: &[GeneratedBinding],
    spec: &ElementSpec<'_>,
    limits: EditLimits,
    work: &mut Work,
    error_location: EditLocation,
) -> Result<String, EditError> {
    let mut number = 1usize;
    let mut candidate = String::new();
    candidate
        .try_reserve_exact(22)
        .map_err(|_| allocation_error())?;
    loop {
        candidate.clear();
        candidate.push_str("ns");
        use std::fmt::Write as _;
        write!(&mut candidate, "{number}").map_err(|_| allocation_error())?;
        if candidate.len() > limits.max_name_bytes {
            return Err(EditError::located(
                EditErrorKind::LimitExceeded(EditLimit::NameBytes),
                error_location,
            ));
        }
        work.charge(1)?;
        if generated_prefix_available(document, parent, local, generated, &candidate, spec, work)? {
            work.charge(candidate.len())?;
            return Ok(candidate);
        }
        number = number
            .checked_add(1)
            .ok_or_else(|| EditError::new(EditErrorKind::IdentifierExhausted, None))?;
    }
}

fn find_matching_prefix<'a>(
    document: &'a OwnedDocument,
    parent: Option<usize>,
    local: &'a [LocalBinding<'a>],
    generated: &'a [GeneratedBinding],
    uri: &str,
    include_default: bool,
    work: &mut Work,
    limits: EditLimits,
    error_location: EditLocation,
) -> Result<Option<String>, EditError> {
    for binding in local {
        work.charge(1)?;
        if binding.prefix.is_none() && !include_default {
            continue;
        }
        if work.compare(binding.uri, uri)? {
            return Ok(binding
                .prefix
                .map(|prefix| selected_prefix(prefix, limits, work, error_location))
                .transpose()?);
        }
    }
    for binding in generated {
        work.charge(1)?;
        if binding.prefix.is_none() && !include_default {
            continue;
        }
        if work.compare(&binding.uri, uri)? {
            return Ok(binding
                .prefix
                .as_deref()
                .map(|prefix| selected_prefix(prefix, limits, work, error_location))
                .transpose()?);
        }
    }
    let mut current = parent;
    while let Some(index) = current {
        work.charge(1)?;
        let record = &document.nodes[index];
        if let Content::Element { attributes, .. } = &record.content {
            for attribute_index in attributes.clone() {
                work.charge(1)?;
                let attribute = &document.attributes[attribute_index];
                let name = attribute.name.view(&document.pool.0);
                let Some(prefix) = declaration_prefix(&name) else {
                    continue;
                };
                if prefix.is_none() && !include_default {
                    continue;
                }
                if let Some(effective) = lookup(document, parent, prefix, local, generated, work)? {
                    if work.compare(effective, uri)? {
                        return Ok(prefix
                            .map(|value| selected_prefix(value, limits, work, error_location))
                            .transpose()?);
                    }
                }
            }
        }
        current = record.parent.get().map(|node| node.0);
    }
    Ok(None)
}

fn selected_prefix(
    prefix: &str,
    limits: EditLimits,
    work: &mut Work,
    error_location: EditLocation,
) -> Result<String, EditError> {
    if prefix.len() > limits.max_name_bytes {
        let location = match error_location {
            EditLocation::ElementName { .. } => EditLocation::ElementName {
                component: EditComponent::Prefix,
                byte_offset: Some(limits.max_name_bytes),
            },
            EditLocation::NamespaceItem { index, .. } => EditLocation::NamespaceItem {
                index,
                component: Some(EditComponent::Prefix),
                byte_offset: Some(limits.max_name_bytes),
            },
            EditLocation::AttributeItem { index, .. } => EditLocation::AttributeItem {
                index,
                component: Some(EditComponent::Prefix),
                byte_offset: Some(limits.max_name_bytes),
            },
        };
        return Err(EditError::located(
            EditErrorKind::LimitExceeded(EditLimit::NameBytes),
            location,
        ));
    }
    work.charge(prefix.len())?;
    clone_string(prefix)
}

fn namespace_error(loc: EditLocation) -> EditError {
    EditError::located(EditErrorKind::Namespace, loc)
}

fn put_string(scratch: &mut String, value: &str) -> Range<usize> {
    let start = scratch.len();
    scratch.push_str(value);
    start..scratch.len()
}

fn ensure_scratch(scratch: &mut String, additional: usize) -> Result<(), EditError> {
    scratch
        .try_reserve_exact(additional)
        .map_err(|_| allocation_error())
}

fn make_name(
    scratch: &mut String,
    local_name: &str,
    namespace: Option<&str>,
    prefix: Option<&str>,
) -> Result<StagedName, EditError> {
    let required = prefix
        .map_or(0, |prefix| {
            prefix.len().checked_add(1).unwrap_or(usize::MAX)
        })
        .checked_add(local_name.len())
        .and_then(|bytes| bytes.checked_add(namespace.map_or(0, str::len)))
        .ok_or_else(allocation_error)?;
    ensure_scratch(scratch, required)?;
    let start = scratch.len();
    if let Some(prefix) = prefix {
        scratch.push_str(prefix);
        scratch.push(':');
    }
    let local_start = scratch.len();
    scratch.push_str(local_name);
    let qualified = start..scratch.len();
    let prefix_length = prefix.and_then(|prefix| NonZeroUsize::new(prefix.len()));
    let local = local_start..scratch.len();
    let namespace = namespace.map(|namespace| put_string(scratch, namespace));
    Ok(StagedName {
        qualified,
        local,
        prefix_length,
        namespace,
    })
}

fn make_declaration(
    scratch: &mut String,
    prefix: Option<&str>,
    uri: &str,
) -> Result<StagedAttribute, EditError> {
    let local = prefix.unwrap_or("xmlns");
    let required = (if prefix.is_some() { 6usize } else { 5usize })
        .checked_add(prefix.map_or(0, str::len))
        .and_then(|bytes| bytes.checked_add(XMLNS.len()))
        .and_then(|bytes| bytes.checked_add(uri.len()))
        .ok_or_else(allocation_error)?;
    ensure_scratch(scratch, required)?;
    let start = scratch.len();
    if let Some(prefix) = prefix {
        scratch.push_str("xmlns:");
        scratch.push_str(prefix);
    } else {
        scratch.push_str("xmlns");
    }
    let qualified = start..scratch.len();
    let prefix_length = prefix.and_then(|_| NonZeroUsize::new(5));
    let local = put_string(scratch, local);
    let namespace = Some(put_string(scratch, XMLNS));
    let value = put_string(scratch, uri);
    Ok(StagedAttribute {
        name: StagedName {
            qualified,
            local,
            prefix_length,
            namespace,
        },
        value,
        is_declaration: true,
    })
}

fn name_location(
    index: Option<usize>,
    is_namespace: bool,
    component: EditComponent,
) -> EditLocation {
    match index {
        Some(index) => item_location(index, is_namespace, Some(component), None),
        None => location(component, None),
    }
}

fn resolve_name(
    document: &OwnedDocument,
    parent: Option<usize>,
    spec: &ElementSpec<'_>,
    name: NameSpec<'_>,
    is_element: bool,
    item_index: Option<usize>,
    local: &[LocalBinding<'_>],
    generated: &mut Vec<GeneratedBinding>,
    work: &mut Work,
    limits: EditLimits,
    scratch: &mut String,
) -> Result<StagedName, EditError> {
    let component_location = name_location(item_index, false, EditComponent::Prefix);
    let namespace_location = name_location(item_index, false, EditComponent::NamespaceUri);
    let namespace = name.namespace;
    work.charge(XMLNS.len())?;
    let empty_namespace = match namespace {
        Some(uri) => work.compare(uri, "")?,
        None => false,
    };
    let xmlns_namespace = match namespace {
        Some(uri) => work.compare(uri, XMLNS)?,
        None => false,
    };
    if empty_namespace || xmlns_namespace {
        return Err(namespace_error(namespace_location));
    }
    let xml_namespace = match namespace {
        Some(uri) => work.compare(uri, XML)?,
        None => false,
    };
    let exact_xml_prefix = match name.prefix {
        PrefixPolicy::Exact(Some(prefix)) => work.compare(prefix, "xml")?,
        _ => false,
    };
    if matches!(name.prefix, PrefixPolicy::Exact(_)) && xml_namespace && !exact_xml_prefix {
        return Err(namespace_error(namespace_location));
    }
    let explicit_default = declaration_exists(local, None, work)?;
    let prefix = match name.prefix {
        PrefixPolicy::Exact(prefix) => {
            let empty_prefix = match prefix {
                Some(prefix) => work.compare(prefix, "")?,
                None => false,
            };
            let xmlns_prefix = match prefix {
                Some(prefix) => work.compare(prefix, "xmlns")?,
                None => false,
            };
            if empty_prefix || xmlns_prefix {
                return Err(namespace_error(component_location));
            }
            match (namespace, prefix) {
                (None, None) => {
                    if is_element {
                        if let Some(current) =
                            lookup(document, parent, None, local, generated, work)?
                        {
                            if !work.compare(current, "")? {
                                if explicit_default {
                                    return Err(namespace_error(namespace_location));
                                }
                                ensure_binding(
                                    document,
                                    parent,
                                    local,
                                    generated,
                                    None,
                                    "",
                                    true,
                                    work,
                                    namespace_location,
                                    spec.items.len(),
                                    limits.max_attributes,
                                )?;
                            }
                        } else if explicit_default {
                            // An explicit empty declaration already resolves to no namespace.
                        }
                    }
                    None
                }
                (None, Some(_)) => return Err(namespace_error(component_location)),
                (Some(_uri), Some("xml")) if xml_namespace => Some(clone_string("xml")?),
                (Some(_uri), Some("xml")) => return Err(namespace_error(namespace_location)),
                (Some(uri), Some(prefix)) => {
                    ensure_binding(
                        document,
                        parent,
                        local,
                        generated,
                        Some(prefix),
                        uri,
                        true,
                        work,
                        namespace_location,
                        spec.items.len(),
                        limits.max_attributes,
                    )?;
                    Some(clone_string(prefix)?)
                }
                (Some(uri), None) if is_element => {
                    ensure_binding(
                        document,
                        parent,
                        local,
                        generated,
                        None,
                        uri,
                        true,
                        work,
                        namespace_location,
                        spec.items.len(),
                        limits.max_attributes,
                    )?;
                    None
                }
                (Some(_), None) => return Err(namespace_error(component_location)),
            }
        }
        PrefixPolicy::Automatic => match namespace {
            None => {
                if is_element {
                    if let Some(current) = lookup(document, parent, None, local, generated, work)? {
                        if !work.compare(current, "")? {
                            if explicit_default {
                                return Err(namespace_error(namespace_location));
                            }
                            ensure_binding(
                                document,
                                parent,
                                local,
                                generated,
                                None,
                                "",
                                true,
                                work,
                                namespace_location,
                                spec.items.len(),
                                limits.max_attributes,
                            )?;
                        }
                    } else if explicit_default {
                        // An explicit empty declaration already resolves to no namespace.
                    }
                }
                None
            }
            Some(_uri) if xml_namespace => Some(clone_string("xml")?),
            Some(uri) if is_element => {
                let default_binding = lookup(document, parent, None, local, generated, work)?;
                if let Some(bound) = default_binding {
                    if work.compare(bound, uri)? {
                        return make_name(scratch, name.local, namespace, None);
                    }
                }
                if !explicit_default {
                    ensure_binding(
                        document,
                        parent,
                        local,
                        generated,
                        None,
                        uri,
                        true,
                        work,
                        namespace_location,
                        spec.items.len(),
                        limits.max_attributes,
                    )?;
                    None
                } else if let Some(prefix) = find_matching_prefix(
                    document,
                    parent,
                    local,
                    generated,
                    uri,
                    false,
                    work,
                    limits,
                    component_location,
                )? {
                    Some(prefix)
                } else {
                    let prefix = next_prefix(
                        document,
                        parent,
                        local,
                        generated,
                        spec,
                        limits,
                        work,
                        component_location,
                    )?;
                    ensure_binding(
                        document,
                        parent,
                        local,
                        generated,
                        Some(&prefix),
                        uri,
                        false,
                        work,
                        namespace_location,
                        spec.items.len(),
                        limits.max_attributes,
                    )?;
                    Some(prefix)
                }
            }
            Some(uri) => {
                if let Some(prefix) = find_matching_prefix(
                    document,
                    parent,
                    local,
                    generated,
                    uri,
                    false,
                    work,
                    limits,
                    component_location,
                )? {
                    Some(prefix)
                } else {
                    let prefix = next_prefix(
                        document,
                        parent,
                        local,
                        generated,
                        spec,
                        limits,
                        work,
                        component_location,
                    )?;
                    ensure_binding(
                        document,
                        parent,
                        local,
                        generated,
                        Some(&prefix),
                        uri,
                        false,
                        work,
                        namespace_location,
                        spec.items.len(),
                        limits.max_attributes,
                    )?;
                    Some(prefix)
                }
            }
        },
    };
    if let PrefixPolicy::Exact(prefix) = name.prefix {
        if let Some(prefix) = prefix {
            work.charge(prefix.len())?;
        }
    }
    let unprefixed_xmlns_attribute = if !is_element && namespace.is_none() && prefix.is_none() {
        work.compare(name.local, "xmlns")?
    } else {
        false
    };
    if unprefixed_xmlns_attribute {
        return Err(EditError::located(
            EditErrorKind::Namespace,
            name_location(item_index, false, EditComponent::LocalName),
        ));
    }
    make_name(scratch, name.local, namespace, prefix.as_deref())
}

fn stage(
    document: &OwnedDocument,
    parent: Option<usize>,
    spec: &ElementSpec<'_>,
    limits: EditLimits,
) -> Result<StagedElement, EditError> {
    let supplied_bytes = check_spec_lengths(spec, limits)?;
    validate_spec(spec)?;
    let generated_bound = spec
        .items
        .iter()
        .filter(|item| matches!(item, ElementItem::Attribute(_)))
        .count()
        .checked_add(1)
        .ok_or_else(allocation_error)?;
    let output_capacity = spec
        .items
        .len()
        .checked_add(generated_bound)
        .ok_or_else(allocation_error)?;
    let mut local: Vec<LocalBinding<'_>> = Vec::new();
    let mut generated = Vec::new();
    let mut attributes = Vec::new();
    reserve_vec(&mut local, spec.items.len())?;
    reserve_vec(&mut generated, generated_bound)?;
    reserve_vec(&mut attributes, output_capacity)?;
    let declaration_overhead = spec
        .items
        .len()
        .checked_add(generated_bound)
        .and_then(|count| count.checked_mul(6 + XMLNS.len()))
        .ok_or_else(allocation_error)?;
    let string_capacity = supplied_bytes
        .checked_mul(2)
        .and_then(|bytes| bytes.checked_add(generated_bound.checked_mul(22 + 6 + XMLNS.len())?))
        .and_then(|bytes| bytes.checked_add(declaration_overhead))
        .and_then(|bytes| bytes.checked_add(128))
        .ok_or_else(allocation_error)?;
    let mut strings = String::new();
    strings
        .try_reserve_exact(string_capacity)
        .map_err(|_| allocation_error())?;

    let mut work = Work {
        used: 0,
        limit: limits.max_namespace_work,
    };
    for (index, item) in spec.items.iter().enumerate() {
        if let ElementItem::Namespace(declaration) = item {
            let binding = LocalBinding {
                prefix: declaration.prefix,
                uri: declaration.uri,
            };
            for previous in &local {
                work.charge(1)?;
                if work.compare(previous.prefix.unwrap_or(""), binding.prefix.unwrap_or(""))? {
                    return Err(located_item(
                        EditErrorKind::DuplicateAttribute,
                        index,
                        true,
                        None,
                        None,
                    ));
                }
            }
            local.push(binding);
        }
    }
    let resolved_element = resolve_name(
        document,
        parent,
        spec,
        spec.name,
        true,
        None,
        &local,
        &mut generated,
        &mut work,
        limits,
        &mut strings,
    )?;
    for (index, item) in spec.items.iter().enumerate() {
        match item {
            ElementItem::Namespace(declaration) => {
                attributes.push(make_declaration(
                    &mut strings,
                    declaration.prefix,
                    declaration.uri,
                )?);
            }
            ElementItem::Attribute(attribute) => {
                let name = resolve_name(
                    document,
                    parent,
                    spec,
                    attribute.name,
                    false,
                    Some(index),
                    &local,
                    &mut generated,
                    &mut work,
                    limits,
                    &mut strings,
                )?;
                for previous in &attributes {
                    if previous.is_declaration {
                        continue;
                    }
                    work.charge(1)?;
                    let namespace_matches = match (&previous.name.namespace, &name.namespace) {
                        (None, None) => true,
                        (Some(left), Some(right)) => {
                            work.compare(&strings[left.clone()], &strings[right.clone()])?
                        }
                        _ => false,
                    };
                    if namespace_matches {
                        if work.compare(
                            &strings[previous.name.local.clone()],
                            &strings[name.local.clone()],
                        )? {
                            return Err(located_item(
                                EditErrorKind::DuplicateAttribute,
                                index,
                                false,
                                None,
                                None,
                            ));
                        }
                    }
                }
                let value = put_string(&mut strings, attribute.value);
                attributes.push(StagedAttribute {
                    name,
                    value,
                    is_declaration: false,
                });
            }
        }
    }
    let final_count = spec
        .items
        .len()
        .checked_add(generated.len())
        .ok_or_else(allocation_error)?;
    if final_count > limits.max_attributes {
        return Err(EditError::new(
            EditErrorKind::LimitExceeded(EditLimit::Attributes),
            None,
        ));
    }
    for declaration in &generated {
        attributes.push(make_declaration(
            &mut strings,
            declaration.prefix.as_deref(),
            &declaration.uri,
        )?);
    }
    debug_assert!(strings.len() <= strings.capacity());
    Ok(StagedElement {
        strings,
        name: resolved_element,
        attributes,
    })
}

fn checked_payload_size(staged: &StagedElement) -> Result<usize, EditError> {
    let mut size = staged.name.qualified.len();
    if let Some(namespace) = &staged.name.namespace {
        size = size.checked_add(namespace.len()).ok_or_else(|| {
            EditError::new(EditErrorKind::LimitExceeded(EditLimit::OwnedBytes), None)
        })?;
    }
    for attribute in &staged.attributes {
        size = size
            .checked_add(attribute.name.qualified.len())
            .ok_or_else(|| {
                EditError::new(EditErrorKind::LimitExceeded(EditLimit::OwnedBytes), None)
            })?;
        if let Some(namespace) = &attribute.name.namespace {
            size = size.checked_add(namespace.len()).ok_or_else(|| {
                EditError::new(EditErrorKind::LimitExceeded(EditLimit::OwnedBytes), None)
            })?;
        }
        size = size.checked_add(attribute.value.len()).ok_or_else(|| {
            EditError::new(EditErrorKind::LimitExceeded(EditLimit::OwnedBytes), None)
        })?;
    }
    Ok(size)
}

fn store(pool: &mut String, value: &str) -> Range<usize> {
    let start = pool.len();
    pool.push_str(value);
    start..pool.len()
}

fn store_name(pool: &mut String, scratch: &str, name: &StagedName) -> StoredName {
    let qualified_text = &scratch[name.qualified.clone()];
    let qualified = store(pool, qualified_text);
    let namespace = name
        .namespace
        .as_ref()
        .map(|namespace| store(pool, &scratch[namespace.clone()]));
    StoredName {
        qualified,
        prefix_length: name.prefix_length,
        namespace,
    }
}

fn store_attributes(
    pool: &mut String,
    scratch: &str,
    staged: &[StagedAttribute],
    output: &mut Vec<StoredAttribute>,
) {
    for attribute in staged {
        output.push(StoredAttribute {
            name: store_name(pool, scratch, &attribute.name),
            value: store(pool, &scratch[attribute.value.clone()]),
        });
    }
}

fn depth_bounded(
    document: &OwnedDocument,
    parent: usize,
    maximum: usize,
) -> Result<usize, EditError> {
    let mut current = Some(NodeId(parent));
    let mut result = 0usize;
    while let Some(id) = current {
        let record = document
            .nodes
            .get(id.0)
            .ok_or_else(|| EditError::new(EditErrorKind::InvalidHandle, None))?;
        if matches!(record.content, Content::Element { .. }) {
            result = result.checked_add(1).ok_or_else(|| {
                EditError::new(EditErrorKind::LimitExceeded(EditLimit::Depth), None)
            })?;
            if result >= maximum {
                return Ok(result);
            }
        }
        current = record.parent.get();
    }
    Ok(result)
}

fn limit_global(limit: EditLimit) -> EditError {
    EditError::new(EditErrorKind::LimitExceeded(limit), None)
}

pub(super) fn create(
    root: ElementSpec<'_>,
    limits: EditLimits,
) -> Result<OwnedDocument, EditError> {
    if limits.max_depth < 1 {
        return Err(limit_global(EditLimit::Depth));
    }
    if root.items.len() > limits.max_attributes {
        return Err(limit_global(EditLimit::Attributes));
    }

    // The staging document has no published identity or nodes. It exists only
    // to share the same bounded namespace planner as append_element.
    let mut staging_document = OwnedDocument {
        identity: NonZeroUsize::MIN,
        nodes: Vec::new(),
        attributes: Vec::new(),
        pool: Pool::default(),
        root_element: NodeId(0),
        declaration: None,
        edit_owned_bytes: 0,
        edit_created_nodes: 0,
        edit_created_attributes: 0,
        slots_are_preorder: true,
    };
    let staged = stage(&staging_document, None, &root, limits)?;
    let created_attributes = staged.attributes.len();
    if created_attributes > limits.max_created_attributes {
        return Err(limit_global(EditLimit::CreatedAttributes));
    }
    if limits.max_created_nodes < 1 {
        return Err(limit_global(EditLimit::CreatedNodes));
    }
    let additional = checked_payload_size(&staged)?;
    let capacity = staging_document
        .pool
        .edit_capacity(additional, 0, limits.max_owned_bytes)?;

    staging_document
        .nodes
        .try_reserve_exact(2)
        .map_err(|_| allocation_error())?;
    staging_document
        .attributes
        .try_reserve_exact(created_attributes)
        .map_err(|_| allocation_error())?;
    staging_document
        .pool
        .reserve_edit(capacity, &mut staging_document.edit_owned_bytes)?;

    let document_node = OwnedRecord::root(0);
    let root_id = NodeId(1);
    let root_link = Link::new(root_id).ok_or_else(allocation_error)?;
    let document_link = Link::new(NodeId(0)).ok_or_else(allocation_error)?;
    let root_name = store_name(&mut staging_document.pool.0, &staged.strings, &staged.name);
    let start = staging_document.attributes.len();
    let expected_end = start
        .checked_add(created_attributes)
        .ok_or_else(allocation_error)?;
    store_attributes(
        &mut staging_document.pool.0,
        &staged.strings,
        &staged.attributes,
        &mut staging_document.attributes,
    );
    debug_assert_eq!(staging_document.attributes.len(), expected_end);
    staging_document.nodes.push(document_node);
    staging_document.nodes.push(OwnedRecord {
        id: root_id,
        content: Content::Element {
            name: root_name,
            attributes: start..expected_end,
        },
        source: None,
        parent: document_link,
        first_child: Link::NONE,
        last_child: Link::NONE,
        previous_sibling: Link::NONE,
        next_sibling: Link::NONE,
        subtree_size: 1,
    });
    staging_document.nodes[0].first_child = root_link;
    staging_document.nodes[0].last_child = root_link;
    staging_document.nodes[0].subtree_size = 2;
    staging_document.root_element = root_id;
    staging_document.identity = document_identity(&LAST_DOCUMENT_ID)
        .map_err(|_| EditError::new(EditErrorKind::IdentifierExhausted, None))?;
    staging_document.edit_created_nodes = 1;
    staging_document.edit_created_attributes = created_attributes;
    Ok(staging_document)
}

pub(super) fn append(
    editor: &mut Editor<'_>,
    parent_id: OwnedNodeId,
    element: ElementSpec<'_>,
) -> Result<OwnedNodeId, EditError> {
    let parent = editor.resolve(parent_id)?;
    if !matches!(
        editor.document.nodes[parent].content,
        Content::Element { .. }
    ) {
        return Err(EditError::new(EditErrorKind::WrongNodeKind, None));
    }
    let prospective_depth = depth_bounded(editor.document, parent, editor.limits.max_depth)?
        .checked_add(1)
        .ok_or_else(|| limit_global(EditLimit::Depth))?;
    if prospective_depth > editor.limits.max_depth {
        return Err(limit_global(EditLimit::Depth));
    }
    if element.items.len() > editor.limits.max_attributes {
        return Err(limit_global(EditLimit::Attributes));
    }
    let staged = stage(editor.document, Some(parent), &element, editor.limits)?;
    let created_nodes = editor
        .document
        .edit_created_nodes
        .checked_add(1)
        .ok_or_else(|| limit_global(EditLimit::CreatedNodes))?;
    if created_nodes > editor.limits.max_created_nodes {
        return Err(limit_global(EditLimit::CreatedNodes));
    }
    let created_attributes = editor
        .document
        .edit_created_attributes
        .checked_add(staged.attributes.len())
        .ok_or_else(|| limit_global(EditLimit::CreatedAttributes))?;
    if created_attributes > editor.limits.max_created_attributes {
        return Err(limit_global(EditLimit::CreatedAttributes));
    }
    let additional = checked_payload_size(&staged)?;
    let capacity = editor.document.pool.edit_capacity(
        additional,
        editor.document.edit_owned_bytes,
        editor.limits.max_owned_bytes,
    )?;

    let node_id = NodeId(editor.document.nodes.len());
    let node_link = Link::new(node_id).ok_or_else(allocation_error)?;
    let parent_link = Link::new(NodeId(parent)).ok_or_else(allocation_error)?;
    #[cfg(test)]
    if editor.document.nodes.len() == editor.document.nodes.capacity() {
        inject_edit_reservation_failure(EditReservation::Node)?;
    }
    editor
        .document
        .nodes
        .try_reserve(1)
        .map_err(|_| allocation_error())?;
    editor
        .document
        .attributes
        .try_reserve(staged.attributes.len())
        .map_err(|_| allocation_error())?;
    editor
        .document
        .pool
        .reserve_edit(capacity, &mut editor.document.edit_owned_bytes)?;

    let name = store_name(&mut editor.document.pool.0, &staged.strings, &staged.name);
    let start = editor.document.attributes.len();
    let end = start
        .checked_add(staged.attributes.len())
        .ok_or_else(allocation_error)?;
    store_attributes(
        &mut editor.document.pool.0,
        &staged.strings,
        &staged.attributes,
        &mut editor.document.attributes,
    );
    let previous_sibling = editor.document.nodes[parent].last_child;
    editor.document.slots_are_preorder = false;
    editor.document.nodes.push(OwnedRecord {
        id: node_id,
        content: Content::Element {
            name,
            attributes: start..end,
        },
        source: None,
        parent: parent_link,
        first_child: Link::NONE,
        last_child: Link::NONE,
        previous_sibling,
        next_sibling: Link::NONE,
        subtree_size: 1,
    });
    if let Some(previous) = previous_sibling.get() {
        editor.document.nodes[previous.0].next_sibling = node_link;
    } else {
        editor.document.nodes[parent].first_child = node_link;
    }
    editor.document.nodes[parent].last_child = node_link;
    let mut ancestor = Some(NodeId(parent));
    while let Some(id) = ancestor {
        let record = &mut editor.document.nodes[id.0];
        record.subtree_size += 1;
        ancestor = record.parent.get();
    }
    editor.document.edit_created_nodes = created_nodes;
    editor.document.edit_created_attributes = created_attributes;
    Ok(OwnedNodeId {
        document: editor.document.identity,
        slot: node_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Writer;

    fn root_spec() -> ElementSpec<'static> {
        ElementSpec {
            name: NameSpec {
                local: "catalog",
                namespace: None,
                prefix: PrefixPolicy::Exact(None),
            },
            items: &[],
        }
    }

    #[test]
    fn create_and_append_publish_attributes_links_and_complete_xml() {
        let mut document = OwnedDocument::create(root_spec(), EditLimits::default()).unwrap();
        let root = document.root_element().id();
        let mut editor = document.edit(EditLimits::default()).unwrap();
        let child = editor
            .append_element(
                root,
                ElementSpec {
                    name: NameSpec {
                        local: "item",
                        namespace: Some("urn:shop"),
                        prefix: PrefixPolicy::Exact(Some("s")),
                    },
                    items: &[
                        ElementItem::Namespace(NamespaceSpec {
                            prefix: Some("s"),
                            uri: "urn:shop",
                        }),
                        ElementItem::Attribute(AttributeSpec {
                            name: NameSpec {
                                local: "id",
                                namespace: None,
                                prefix: PrefixPolicy::Exact(None),
                            },
                            value: "A-17",
                        }),
                    ],
                },
            )
            .unwrap();
        let view = editor.document().node(child).unwrap();
        assert_eq!(view.name().unwrap().qualified, "s:item");
        assert_eq!(
            view.attributes()
                .map(|a| (a.name.qualified, a.value.into_owned()))
                .collect::<Vec<_>>(),
            [
                ("xmlns:s", "urn:shop".to_owned()),
                ("id", "A-17".to_owned())
            ]
        );
        assert_eq!(editor.document().nodes().count(), 3);
        assert_eq!(
            editor.document().root_element().first_child().unwrap().id(),
            child
        );
        let mut writer = Writer::new(Vec::new());
        writer.write_owned_document(editor.document()).unwrap();
        assert_eq!(
            writer.into_inner(),
            b"<catalog><s:item xmlns:s=\"urn:shop\" id=\"A-17\"/></catalog>"
        );
    }

    #[test]
    fn automatic_resolution_uses_default_binding_and_reserves_later_exact_prefix() {
        let mut document = OwnedDocument::parse("<r xmlns='urn:u' xmlns:p='urn:u'/>").unwrap();
        let root = document.root_element().id();
        let mut editor = document.edit(EditLimits::default()).unwrap();
        let child = editor
            .append_element(
                root,
                ElementSpec {
                    name: NameSpec {
                        local: "child",
                        namespace: Some("urn:u"),
                        prefix: PrefixPolicy::Automatic,
                    },
                    items: &[],
                },
            )
            .unwrap();
        assert_eq!(
            editor
                .document()
                .node(child)
                .unwrap()
                .name()
                .unwrap()
                .qualified,
            "child"
        );
        let other = editor
            .append_element(
                root,
                ElementSpec {
                    name: NameSpec {
                        local: "other",
                        namespace: Some("urn:other"),
                        prefix: PrefixPolicy::Automatic,
                    },
                    items: &[
                        ElementItem::Namespace(NamespaceSpec {
                            prefix: None,
                            uri: "",
                        }),
                        ElementItem::Attribute(AttributeSpec {
                            name: NameSpec {
                                local: "a",
                                namespace: Some("urn:a"),
                                prefix: PrefixPolicy::Exact(Some("ns1")),
                            },
                            value: "v",
                        }),
                    ],
                },
            )
            .unwrap();
        let other = editor.document().node(other).unwrap();
        assert_eq!(other.name().unwrap().qualified, "ns2:other");
        assert_eq!(
            other
                .attributes()
                .map(|a| a.name.qualified)
                .collect::<Vec<_>>(),
            ["xmlns", "ns1:a", "xmlns:ns2", "xmlns:ns1"]
        );
    }

    #[test]
    fn duplicate_attribute_failure_preserves_document_and_counters() {
        let mut document = OwnedDocument::create(root_spec(), EditLimits::default()).unwrap();
        let root = document.root_element().id();
        let before = (
            document.nodes.len(),
            document.attributes.len(),
            document.pool.0.clone(),
        );
        let mut editor = document.edit(EditLimits::default()).unwrap();
        let result = editor.append_element(
            root,
            ElementSpec {
                name: NameSpec {
                    local: "a",
                    namespace: None,
                    prefix: PrefixPolicy::Exact(None),
                },
                items: &[
                    ElementItem::Attribute(AttributeSpec {
                        name: NameSpec {
                            local: "id",
                            namespace: None,
                            prefix: PrefixPolicy::Exact(None),
                        },
                        value: "one",
                    }),
                    ElementItem::Attribute(AttributeSpec {
                        name: NameSpec {
                            local: "id",
                            namespace: None,
                            prefix: PrefixPolicy::Exact(None),
                        },
                        value: "two",
                    }),
                ],
            },
        );
        assert_eq!(
            result.unwrap_err(),
            EditError::located(
                EditErrorKind::DuplicateAttribute,
                EditLocation::AttributeItem {
                    index: 1,
                    component: None,
                    byte_offset: None
                }
            )
        );
        assert_eq!(
            (
                editor.document().nodes.len(),
                editor.document().attributes.len(),
                editor.document().pool.0.clone()
            ),
            before
        );
        assert_eq!(editor.document().edit_created_nodes, 1);
        assert_eq!(editor.document().edit_created_attributes, 0);
    }
}
