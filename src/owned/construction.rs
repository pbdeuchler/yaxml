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

pub(super) struct Work {
    pub(super) used: usize,
    pub(super) limit: usize,
}

impl Work {
    pub(super) fn charge(&mut self, amount: usize) -> Result<(), EditError> {
        let next = self.used.saturating_add(amount);
        if next > self.limit {
            return Err(namespace_work_error());
        }
        self.used = next;
        Ok(())
    }

    fn compare(&mut self, left: &str, right: &str) -> Result<bool, EditError> {
        self.charge(left.len() + right.len())?;
        Ok(left == right)
    }
}

pub(super) fn allocation_error() -> EditError {
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
    let next = supplied.saturating_add(value.len());
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
    let next = supplied.saturating_add(value.len());
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

pub(super) fn validate_part(
    value: &str,
    component: EditComponent,
    item: Option<(usize, bool)>,
    is_name: bool,
) -> Result<(), EditError> {
    if let Some(offset) = crate::escape::invalid_char(value) {
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
        crate::name::validate_name(value, 0, false).map_err(|error| match item {
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
                if let Some(offset) = crate::escape::invalid_char(attribute.value) {
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
            return Err(namespace_error(item_location(index, true, None, None)));
        }
        if invalid_uri {
            return Err(namespace_error(item_location(index, true, None, None)));
        }
    }
    Ok(())
}

fn reserve_vec<T>(items: &mut Vec<T>, amount: usize) -> Result<(), EditError> {
    #[cfg(test)]
    if items
        .len()
        .checked_add(amount)
        .is_some_and(|needed| needed > items.capacity())
    {
        inject_edit_reservation_failure(EditReservation::Scratch)?;
    }
    items
        .try_reserve_exact(amount)
        .map_err(|_| allocation_error())
}

fn push_string(target: &mut String, value: &str) -> Result<(), EditError> {
    #[cfg(test)]
    if !value.is_empty() && target.len().saturating_add(value.len()) > target.capacity() {
        inject_edit_reservation_failure(EditReservation::Scratch)?;
    }
    target
        .try_reserve_exact(value.len())
        .map_err(|_| allocation_error())?;
    target.push_str(value);
    Ok(())
}

pub(super) fn clone_string(value: &str) -> Result<String, EditError> {
    let mut result = String::new();
    push_string(&mut result, value)?;
    Ok(result)
}

fn declaration_prefix<'a>(name: &'a Name<'_>) -> Option<Option<&'a str>> {
    // Only `xmlns` and `xmlns:*` names are bound to the XMLNS namespace.
    (name.namespace() == Some(XMLNS)).then(|| name.prefix().map(|_| name.local()))
}

fn lookup<'a, A: Allocator>(
    document: &'a OwnedDocument<A>,
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
        let record = &document.nodes[index];
        let Content::Element { attributes, .. } = &record.content else {
            break;
        };
        work.charge(1)?;
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
                return Ok(Some(uri));
            }
        }
        current = record.parent.get().map(|node| node.index());
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
            return Ok(Some(&binding.uri));
        }
    }
    Ok(None)
}

#[allow(clippy::too_many_arguments)]
fn ensure_binding<A: Allocator>(
    document: &OwnedDocument<A>,
    parent: Option<usize>,
    local: &[LocalBinding<'_>],
    generated: &mut Vec<GeneratedBinding>,
    prefix: Option<&str>,
    uri: &str,
    work: &mut Work,
    error_location: EditLocation,
    supplied_items: usize,
    max_attributes: usize,
) -> Result<(), EditError> {
    if let Some(current) = lookup_explicit(local, prefix, work)? {
        if work.compare(current, uri)? {
            return Ok(());
        }
        return Err(namespace_error(error_location));
    }
    if let Some(current) = lookup_generated(generated, prefix, work)? {
        if work.compare(current, uri)? {
            return Ok(());
        }
        return Err(namespace_error(error_location));
    }
    if let Some(current) = lookup(document, parent, prefix, local, generated, work)? {
        // An exact prefix rebinds the inherited one; a generated prefix
        // was chosen unbound.
        if work.compare(current, uri)? {
            return Ok(());
        }
    }
    if supplied_items + generated.len() + 1 > max_attributes {
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
        if let ElementItem::Attribute(attribute) = item
            && check(attribute.name.prefix)?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn exact_prefix_is_compatible(
    spec: &ElementSpec<'_>,
    candidate: &str,
    uri: &str,
    work: &mut Work,
) -> Result<bool, EditError> {
    let mut check = |name: NameSpec<'_>| -> Result<bool, EditError> {
        let PrefixPolicy::Exact(Some(prefix)) = name.prefix else {
            return Ok(true);
        };
        work.charge(1)?;
        if !work.compare(prefix, candidate)? {
            return Ok(true);
        }
        match name.namespace {
            Some(exact_uri) => work.compare(exact_uri, uri),
            None => {
                work.charge(uri.len())?;
                Ok(uri.is_empty())
            }
        }
    };
    if !check(spec.name)? {
        return Ok(false);
    }
    for item in spec.items {
        if let ElementItem::Attribute(attribute) = item
            && !check(attribute.name)?
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn generated_prefix_available<A: Allocator>(
    document: &OwnedDocument<A>,
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

#[allow(clippy::too_many_arguments)]
fn next_prefix<A: Allocator>(
    document: &OwnedDocument<A>,
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
    #[cfg(test)]
    inject_edit_reservation_failure(EditReservation::Scratch)?;
    candidate
        .try_reserve_exact(22)
        .map_err(|_| allocation_error())?;
    loop {
        candidate.clear();
        candidate.push_str("ns");
        use std::fmt::Write as _;
        write!(&mut candidate, "{number}").expect("a String accepts every write");
        if candidate.len() > limits.max_name_bytes {
            return Err(EditError::located(
                EditErrorKind::LimitExceeded(EditLimit::NameBytes),
                prefix_limit_location(error_location, limits.max_name_bytes),
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

#[allow(clippy::too_many_arguments)]
fn find_matching_prefix<'a, A: Allocator>(
    document: &'a OwnedDocument<A>,
    parent: Option<usize>,
    spec: &ElementSpec<'_>,
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
        if work.compare(binding.uri, uri)?
            && (binding.prefix.is_none()
                || exact_prefix_is_compatible(spec, binding.prefix.unwrap_or(""), uri, work)?)
        {
            return binding
                .prefix
                .map(|prefix| selected_prefix(prefix, limits, work, error_location))
                .transpose();
        }
    }
    for binding in generated {
        work.charge(1)?;
        if binding.prefix.is_none() && !include_default {
            continue;
        }
        if work.compare(&binding.uri, uri)?
            && (binding.prefix.is_none()
                || exact_prefix_is_compatible(
                    spec,
                    binding.prefix.as_deref().unwrap_or(""),
                    uri,
                    work,
                )?)
        {
            return binding
                .prefix
                .as_deref()
                .map(|prefix| selected_prefix(prefix, limits, work, error_location))
                .transpose();
        }
    }
    let mut current = parent;
    while let Some(index) = current {
        let record = &document.nodes[index];
        let Content::Element { attributes, .. } = &record.content else {
            break;
        };
        work.charge(1)?;
        for attribute_index in attributes.clone() {
            work.charge(1)?;
            let attribute = &document.attributes[attribute_index];
            let name = attribute.name.view(&document.pool.0);
            let Some(prefix) = declaration_prefix(&name) else {
                continue;
            };
            work.charge(1)?;
            if prefix.is_none() && !include_default {
                continue;
            }
            if let Some(effective) = lookup(document, parent, prefix, local, generated, work)?
                && work.compare(effective, uri)?
                && exact_prefix_is_compatible(spec, prefix.unwrap_or(""), uri, work)?
            {
                return prefix
                    .map(|value| selected_prefix(value, limits, work, error_location))
                    .transpose();
            }
        }
        current = record.parent.get().map(|node| node.index());
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
        return Err(EditError::located(
            EditErrorKind::LimitExceeded(EditLimit::NameBytes),
            prefix_limit_location(error_location, limits.max_name_bytes),
        ));
    }
    work.charge(prefix.len())?;
    clone_string(prefix)
}

fn prefix_limit_location(location: EditLocation, offset: usize) -> EditLocation {
    match location {
        EditLocation::ElementName { .. } => EditLocation::ElementName {
            component: EditComponent::Prefix,
            byte_offset: Some(offset),
        },
        EditLocation::NamespaceItem { index, .. } => EditLocation::NamespaceItem {
            index,
            component: Some(EditComponent::Prefix),
            byte_offset: Some(offset),
        },
        EditLocation::AttributeItem { index, .. } => EditLocation::AttributeItem {
            index,
            component: Some(EditComponent::Prefix),
            byte_offset: Some(offset),
        },
    }
}

fn namespace_error(loc: EditLocation) -> EditError {
    let location = match loc {
        EditLocation::ElementName { .. } => loc,
        EditLocation::NamespaceItem { index, .. } => item_location(index, true, None, None),
        EditLocation::AttributeItem { index, .. } => item_location(index, false, None, None),
    };
    EditError::located(EditErrorKind::Namespace, location)
}

fn put_string(scratch: &mut String, value: &str) -> Range<usize> {
    let start = scratch.len();
    scratch.push_str(value);
    start..scratch.len()
}

fn ensure_scratch(
    scratch: &mut String,
    additional: usize,
    maximum: usize,
) -> Result<(), EditError> {
    let required = scratch.len().saturating_add(additional);
    if required > maximum {
        return Err(namespace_work_error());
    }
    if required > scratch.capacity() {
        #[cfg(test)]
        inject_edit_reservation_failure(EditReservation::Scratch)?;
    }
    scratch
        .try_reserve_exact(additional)
        .map_err(|_| allocation_error())
}

fn make_name(
    scratch: &mut String,
    local_name: &str,
    namespace: Option<&str>,
    prefix: Option<&str>,
    scratch_limit: usize,
) -> Result<StagedName, EditError> {
    let required = prefix
        .map_or(0, |prefix| prefix.len().saturating_add(1))
        .saturating_add(local_name.len())
        .saturating_add(namespace.map_or(0, str::len));
    ensure_scratch(scratch, required, scratch_limit)?;
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
    scratch_limit: usize,
) -> Result<StagedAttribute, EditError> {
    let required = (if prefix.is_some() { 6usize } else { 5usize })
        .saturating_add(prefix.map_or(0, str::len))
        .saturating_add(XMLNS.len())
        .saturating_add(uri.len());
    ensure_scratch(scratch, required, scratch_limit)?;
    let start = scratch.len();
    if let Some(prefix) = prefix {
        scratch.push_str("xmlns:");
        scratch.push_str(prefix);
    } else {
        scratch.push_str("xmlns");
    }
    let qualified = start..scratch.len();
    let prefix_length = prefix.and_then(|_| NonZeroUsize::new(5));
    let local = if prefix.is_some() {
        start + 6..qualified.end
    } else {
        qualified.clone()
    };
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

fn semantic_name_location(
    item_index: Option<usize>,
    element_component: EditComponent,
) -> EditLocation {
    match item_index {
        Some(index) => item_location(index, false, None, None),
        None => location(element_component, None),
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_name<A: Allocator>(
    document: &OwnedDocument<A>,
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
    scratch_limit: usize,
) -> Result<StagedName, EditError> {
    let component_location = semantic_name_location(item_index, EditComponent::Prefix);
    let namespace_location = semantic_name_location(item_index, EditComponent::NamespaceUri);
    let namespace = name.namespace;
    if namespace.is_some_and(|uri| uri.is_empty() || uri == XMLNS) {
        return Err(namespace_error(namespace_location));
    }
    let xml_namespace = namespace == Some(XML);
    let exact_xml_prefix = match name.prefix {
        PrefixPolicy::Exact(Some(prefix)) => prefix == "xml",
        _ => false,
    };
    if matches!(name.prefix, PrefixPolicy::Exact(_)) && xml_namespace && !exact_xml_prefix {
        return Err(namespace_error(semantic_name_location(
            item_index,
            EditComponent::Prefix,
        )));
    }
    let prefix = match (name.prefix, namespace) {
        (PrefixPolicy::Exact(None) | PrefixPolicy::Automatic, None) => {
            if is_element {
                let explicit_default = declaration_exists(local, None, work)?;
                if let Some(current) = lookup(document, parent, None, local, generated, work)?
                    && !work.compare(current, "")?
                {
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
                        work,
                        namespace_location,
                        spec.items.len(),
                        limits.max_attributes,
                    )?;
                }
            }
            None
        }
        (PrefixPolicy::Exact(prefix), namespace) => {
            // validate_spec already rejected an empty prefix.
            if prefix == Some("xmlns") {
                return Err(namespace_error(component_location));
            }
            match (namespace, prefix) {
                (None, Some("xml")) => return Err(namespace_error(namespace_location)),
                // No namespace with no prefix resolved above.
                (None, _) => return Err(namespace_error(component_location)),
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
        (PrefixPolicy::Automatic, Some(uri)) => match uri {
            _ if xml_namespace => Some(clone_string("xml")?),
            // An element may take the default namespace; attributes need a prefix.
            uri => {
                if is_element
                    && let Some(bound) = lookup(document, parent, None, local, generated, work)?
                    && work.compare(bound, uri)?
                {
                    return make_name(scratch, name.local, namespace, None, scratch_limit);
                }
                if is_element && !declaration_exists(local, None, work)? {
                    ensure_binding(
                        document,
                        parent,
                        local,
                        generated,
                        None,
                        uri,
                        work,
                        namespace_location,
                        spec.items.len(),
                        limits.max_attributes,
                    )?;
                    None
                } else if let Some(prefix) = find_matching_prefix(
                    document,
                    parent,
                    spec,
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
    let unprefixed_xmlns_attribute = if !is_element && namespace.is_none() && prefix.is_none() {
        name.local == "xmlns"
    } else {
        false
    };
    if unprefixed_xmlns_attribute {
        return Err(namespace_error(semantic_name_location(
            item_index,
            EditComponent::LocalName,
        )));
    }
    make_name(
        scratch,
        name.local,
        namespace,
        prefix.as_deref(),
        scratch_limit,
    )
}

fn stage<A: Allocator>(
    document: &OwnedDocument<A>,
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
        + 1;
    let output_capacity = spec.items.len() + generated_bound;
    let mut local: Vec<LocalBinding<'_>> = Vec::new();
    let mut generated = Vec::new();
    let mut attributes = Vec::new();
    reserve_vec(&mut local, spec.items.len())?;
    reserve_vec(&mut generated, generated_bound)?;
    reserve_vec(&mut attributes, output_capacity)?;
    let declaration_overhead = output_capacity.saturating_mul(6 + XMLNS.len());
    // Supplied bytes account for up to four copies in staged names, values,
    // generated bindings, and declaration values. A chosen inherited prefix
    // is copied into a temporary and final qualified name, bounded by charged
    // namespace work. Saturation preserves valid operations when callers use
    // very large limits; no memory is reserved from this ceiling itself.
    let scratch_limit = supplied_bytes
        .saturating_mul(4)
        .saturating_add(limits.max_namespace_work.saturating_mul(2))
        .saturating_add(generated_bound.saturating_mul(22 + 6 + XMLNS.len()))
        .saturating_add(declaration_overhead)
        .saturating_add(128);
    let string_capacity = supplied_bytes
        .saturating_mul(2)
        .saturating_add(generated_bound.saturating_mul(22 + 6 + XMLNS.len()))
        .saturating_add(declaration_overhead)
        .saturating_add(128);
    let mut strings = String::new();
    #[cfg(test)]
    inject_edit_reservation_failure(EditReservation::Scratch)?;
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
        scratch_limit,
    )?;
    for (index, item) in spec.items.iter().enumerate() {
        match item {
            ElementItem::Namespace(declaration) => {
                attributes.push(make_declaration(
                    &mut strings,
                    declaration.prefix,
                    declaration.uri,
                    scratch_limit,
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
                    scratch_limit,
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
                    if namespace_matches
                        && work.compare(
                            &strings[previous.name.local.clone()],
                            &strings[name.local.clone()],
                        )?
                    {
                        return Err(located_item(
                            EditErrorKind::DuplicateAttribute,
                            index,
                            false,
                            None,
                            None,
                        ));
                    }
                }
                ensure_scratch(&mut strings, attribute.value.len(), scratch_limit)?;
                let value = put_string(&mut strings, attribute.value);
                attributes.push(StagedAttribute {
                    name,
                    value,
                    is_declaration: false,
                });
            }
        }
    }
    for declaration in &generated {
        attributes.push(make_declaration(
            &mut strings,
            declaration.prefix.as_deref(),
            &declaration.uri,
            scratch_limit,
        )?);
    }
    debug_assert!(strings.len() <= strings.capacity());
    Ok(StagedElement {
        strings,
        name: resolved_element,
        attributes,
    })
}

/// The bytes `staged` stores: disjoint ranges of its scratch string.
fn payload_size(staged: &StagedElement) -> usize {
    let name =
        |name: &StagedName| name.qualified.len() + name.namespace.as_ref().map_or(0, Range::len);
    let attributes = staged.attributes.iter();
    name(&staged.name)
        + attributes
            .map(|a| name(&a.name) + a.value.len())
            .sum::<usize>()
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

fn store_attributes<A: Allocator>(
    pool: &mut String,
    scratch: &str,
    staged: &[StagedAttribute],
    output: &mut allocator::Vec<StoredAttribute, A>,
) {
    for attribute in staged {
        output.push(StoredAttribute {
            name: store_name(pool, scratch, &attribute.name),
            value: store(pool, &scratch[attribute.value.clone()]),
        });
    }
}

/// The element depth of `parent`, counted up to `maximum`.
fn depth_bounded<A: Allocator>(
    document: &OwnedDocument<A>,
    parent: usize,
    maximum: usize,
) -> usize {
    std::iter::successors(Some(NodeId::new(parent)), |id| {
        document.nodes[id.index()].parent.get()
    })
    .filter(|id| matches!(document.nodes[id.index()].content, Content::Element { .. }))
    .take(maximum)
    .count()
}

pub(super) fn limit_global(limit: EditLimit) -> EditError {
    EditError::new(EditErrorKind::LimitExceeded(limit), None)
}

pub(super) fn create<A: Allocator + Clone>(
    root: ElementSpec<'_>,
    limits: EditLimits,
    allocator: A,
) -> Result<OwnedDocument<A>, EditError> {
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
        nodes: allocator::Vec::new_in(allocator.clone()),
        kept: allocator::Vec::new_in(allocator.clone()),
        attributes: allocator::Vec::new_in(allocator),
        pool: Pool::default(),
        root_element: NodeId::new(0),
        declaration: None,
        edit_owned_bytes: 0,
        edit_created_nodes: 0,
        edit_created_attributes: 0,
        slots_are_preorder: true,
        html: None,
        ids: Vec::new(),
        dtd: None,
        validity: Vec::new(),
        errors: Vec::new(),
        url: None,
    };
    let staged = stage(&staging_document, None, &root, limits)?;
    if limits.max_created_nodes < 1 {
        return Err(limit_global(EditLimit::CreatedNodes));
    }
    let created_attributes = staged.attributes.len();
    if created_attributes > limits.max_created_attributes {
        return Err(limit_global(EditLimit::CreatedAttributes));
    }
    let additional = payload_size(&staged);
    let capacity = staging_document
        .pool
        .edit_capacity(additional, 0, limits.max_owned_bytes)?;

    #[cfg(test)]
    inject_edit_reservation_failure(EditReservation::Node)?;
    staging_document
        .nodes
        .try_reserve_exact(2)
        .map_err(|_| allocation_error())?;
    #[cfg(test)]
    if created_attributes > 0 {
        inject_edit_reservation_failure(EditReservation::Attribute)?;
    }
    staging_document
        .attributes
        .try_reserve_exact(created_attributes)
        .map_err(|_| allocation_error())?;
    staging_document
        .pool
        .reserve_edit(capacity, &mut staging_document.edit_owned_bytes)?;

    let mut document_node = OwnedRecord::root(0);
    document_node.source = None;
    let root_id = NodeId::new(1);
    let root_link = Link::new(root_id);
    let document_link = Link::new(NodeId::new(0));
    let root_name = store_name(&mut staging_document.pool.0, &staged.strings, &staged.name);
    let start = staging_document.attributes.len();
    let expected_end = start + created_attributes;
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

pub(super) fn append<A: Allocator>(
    editor: &mut Editor<'_, A>,
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
    if depth_bounded(editor.document, parent, editor.limits.max_depth) + 1 > editor.limits.max_depth
    {
        return Err(limit_global(EditLimit::Depth));
    }
    if element.items.len() > editor.limits.max_attributes {
        return Err(limit_global(EditLimit::Attributes));
    }
    let staged = stage(editor.document, Some(parent), &element, editor.limits)?;
    let created_nodes = editor.document.edit_created_nodes + 1;
    if created_nodes > editor.limits.max_created_nodes {
        return Err(limit_global(EditLimit::CreatedNodes));
    }
    let created_attributes = editor.document.edit_created_attributes + staged.attributes.len();
    if created_attributes > editor.limits.max_created_attributes {
        return Err(limit_global(EditLimit::CreatedAttributes));
    }
    let additional = payload_size(&staged);
    let capacity = editor.document.pool.edit_capacity(
        additional,
        editor.document.edit_owned_bytes,
        editor.limits.max_owned_bytes,
    )?;

    let node_id = NodeId::new(editor.document.nodes.len());
    #[cfg(test)]
    if editor.document.nodes.len() == editor.document.nodes.capacity() {
        inject_edit_reservation_failure(EditReservation::Node)?;
    }
    editor
        .document
        .nodes
        .try_reserve(1)
        .map_err(|_| allocation_error())?;
    #[cfg(test)]
    if editor
        .document
        .attributes
        .len()
        .checked_add(staged.attributes.len())
        .is_some_and(|needed| needed > editor.document.attributes.capacity())
    {
        inject_edit_reservation_failure(EditReservation::Attribute)?;
    }
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
    let end = start + staged.attributes.len();
    store_attributes(
        &mut editor.document.pool.0,
        &staged.strings,
        &staged.attributes,
        &mut editor.document.attributes,
    );
    let content = Content::Element {
        name,
        attributes: start..end,
    };
    editor
        .document
        .nodes
        .push(OwnedRecord::detached(node_id, content));
    editor.document.attach(node_id.index(), parent, Link::NONE);
    editor.document.edit_created_nodes = created_nodes;
    editor.document.edit_created_attributes = created_attributes;
    Ok(OwnedNodeId {
        document: editor.document.identity,
        slot: node_id.index(),
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

    fn serialized(document: &OwnedDocument) -> Vec<u8> {
        let mut writer = Writer::new(Vec::new());
        writer.write_owned_document(document).unwrap();
        writer.into_inner()
    }

    #[test]
    fn create_and_append_publish_attributes_links_and_complete_xml() {
        let mut document = OwnedDocument::create(root_spec(), EditLimits::default()).unwrap();
        let root = document.root_element().id();
        assert_eq!(document.root().range(), None);
        assert_eq!(document.root_element().range(), None);
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
        assert_eq!(view.range(), None);
        assert_eq!(view.name().unwrap().qualified(), "s:item");
        assert_eq!(
            view.attributes()
                .map(|a| (a.name.qualified().to_owned(), a.value.into_owned()))
                .collect::<Vec<_>>(),
            [
                ("xmlns:s".to_owned(), "urn:shop".to_owned()),
                ("id".to_owned(), "A-17".to_owned())
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
                .qualified(),
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
        assert_eq!(other.name().unwrap().qualified(), "ns2:other");
        assert_eq!(
            other
                .attributes()
                .map(|a| a.name.qualified().to_owned())
                .collect::<Vec<_>>(),
            ["xmlns", "ns1:a", "xmlns:ns2", "xmlns:ns1"]
        );
    }

    #[test]
    fn automatic_name_does_not_reuse_prefix_shadowed_by_later_exact_intent() {
        let root_items = [ElementItem::Namespace(NamespaceSpec {
            prefix: Some("p"),
            uri: "urn:old",
        })];
        let root = ElementSpec {
            name: NameSpec {
                local: "root",
                namespace: None,
                prefix: PrefixPolicy::Exact(None),
            },
            items: &root_items,
        };
        let mut document = OwnedDocument::create(root, EditLimits::default()).unwrap();
        let parent = document.root_element().id();
        let items = [
            ElementItem::Attribute(AttributeSpec {
                name: NameSpec {
                    local: "id",
                    namespace: Some("urn:old"),
                    prefix: PrefixPolicy::Automatic,
                },
                value: "1",
            }),
            ElementItem::Attribute(AttributeSpec {
                name: NameSpec {
                    local: "tag",
                    namespace: Some("urn:new"),
                    prefix: PrefixPolicy::Exact(Some("p")),
                },
                value: "x",
            }),
        ];
        let mut editor = document.edit(EditLimits::default()).unwrap();
        let child = editor
            .append_element(
                parent,
                ElementSpec {
                    name: NameSpec {
                        local: "child",
                        namespace: None,
                        prefix: PrefixPolicy::Exact(None),
                    },
                    items: &items,
                },
            )
            .unwrap();
        let element = editor.document().node(child).unwrap();
        assert_eq!(
            element
                .attributes()
                .map(|attribute| attribute.name.qualified().to_owned())
                .collect::<Vec<_>>(),
            ["ns1:id", "p:tag", "xmlns:ns1", "xmlns:p"]
        );
        assert_eq!(
            element
                .attributes()
                .map(|attribute| attribute.value.into_owned())
                .collect::<Vec<_>>(),
            ["1", "x", "urn:old", "urn:new"]
        );
    }

    #[test]
    fn namespace_work_charges_only_scope_steps_and_comparison_operands() {
        let zero_work = EditLimits {
            max_namespace_work: 0,
            ..EditLimits::default()
        };
        assert!(OwnedDocument::create(root_spec(), zero_work).is_ok());

        let mut deep = OwnedDocument::parse("<a><b><c/></b></a>").unwrap();
        let ancestor = deep
            .root_element()
            .first_child()
            .unwrap()
            .first_child()
            .unwrap()
            .id();
        let empty_name = ElementSpec {
            name: NameSpec {
                local: "d",
                namespace: None,
                prefix: PrefixPolicy::Exact(None),
            },
            items: &[],
        };
        {
            let mut editor = deep
                .edit(EditLimits {
                    max_namespace_work: 2,
                    ..EditLimits::default()
                })
                .unwrap();
            assert_eq!(
                editor
                    .append_element(ancestor, empty_name)
                    .unwrap_err()
                    .kind,
                EditErrorKind::LimitExceeded(EditLimit::NamespaceWork)
            );
        }
        assert!(
            deep.edit(EditLimits {
                max_namespace_work: 3,
                ..EditLimits::default()
            })
            .unwrap()
            .append_element(ancestor, empty_name)
            .is_ok()
        );

        let mut wide = OwnedDocument::parse("<r a='1' b='2'><e><f/></e></r>").unwrap();
        let parent = wide
            .root_element()
            .first_child()
            .unwrap()
            .first_child()
            .unwrap()
            .id();
        {
            let mut editor = wide
                .edit(EditLimits {
                    max_namespace_work: 4,
                    ..EditLimits::default()
                })
                .unwrap();
            assert_eq!(
                editor.append_element(parent, empty_name).unwrap_err().kind,
                EditErrorKind::LimitExceeded(EditLimit::NamespaceWork)
            );
        }
        assert!(
            wide.edit(EditLimits {
                max_namespace_work: 5,
                ..EditLimits::default()
            })
            .unwrap()
            .append_element(parent, empty_name)
            .is_ok()
        );

        let root_items = [
            ElementItem::Namespace(NamespaceSpec {
                prefix: None,
                uri: "urn:u",
            }),
            ElementItem::Namespace(NamespaceSpec {
                prefix: Some("p"),
                uri: "urn:u",
            }),
        ];
        let mut document = OwnedDocument::create(
            ElementSpec {
                name: NameSpec {
                    local: "root",
                    namespace: Some("urn:u"),
                    prefix: PrefixPolicy::Automatic,
                },
                items: &root_items,
            },
            EditLimits::default(),
        )
        .unwrap();
        let parent = document.root_element().id();
        let child = ElementSpec {
            name: NameSpec {
                local: "child",
                namespace: Some("urn:u"),
                prefix: PrefixPolicy::Automatic,
            },
            items: &[],
        };
        {
            let limits = EditLimits {
                max_namespace_work: 12,
                ..EditLimits::default()
            };
            let mut editor = document.edit(limits).unwrap();
            let error = editor.append_element(parent, child).unwrap_err();
            assert_eq!(
                error.kind,
                EditErrorKind::LimitExceeded(EditLimit::NamespaceWork)
            );
            assert_eq!(error.location, None);
        }
        let limits = EditLimits {
            max_namespace_work: 13,
            ..EditLimits::default()
        };
        let mut editor = document.edit(limits).unwrap();
        assert!(editor.append_element(parent, child).is_ok());
    }

    #[test]
    fn declaration_name_local_is_a_range_without_extra_scratch_bytes() {
        for (prefix, uri, qualified, local) in [
            (None, "urn:default", "xmlns", "xmlns"),
            (Some("p"), "urn:prefixed", "xmlns:p", "p"),
        ] {
            let required = (if prefix.is_some() { 6 } else { 5 })
                + prefix.map_or(0, str::len)
                + XMLNS.len()
                + uri.len();
            let mut scratch = String::new();
            let staged = make_declaration(&mut scratch, prefix, uri, required).unwrap();
            assert_eq!(scratch.len(), required);
            assert_eq!(&scratch[staged.name.qualified], qualified);
            assert_eq!(&scratch[staged.name.local], local);
            assert_eq!(
                staged.name.namespace.map(|range| &scratch[range]),
                Some(XMLNS)
            );
            assert_eq!(&scratch[staged.value], uri);
        }
    }

    #[test]
    fn append_reservation_failures_leave_no_published_element_or_items() {
        let value = "v".repeat(256);
        let item = ElementItem::Attribute(AttributeSpec {
            name: NameSpec {
                local: "id",
                namespace: None,
                prefix: PrefixPolicy::Exact(None),
            },
            value: &value,
        });
        for reservation in [
            EditReservation::Node,
            EditReservation::Attribute,
            EditReservation::Pool,
        ] {
            let mut document = OwnedDocument::create(root_spec(), EditLimits::default()).unwrap();
            let parent = document.root_element().id();
            let mut editor = document.edit(EditLimits::default()).unwrap();
            let failure = EditReservationFailureGuard::arm(reservation, 0);
            assert_eq!(
                editor.append_element(
                    parent,
                    ElementSpec {
                        name: NameSpec {
                            local: "item",
                            namespace: None,
                            prefix: PrefixPolicy::Exact(None),
                        },
                        items: std::slice::from_ref(&item),
                    }
                ),
                Err(EditError::new(EditErrorKind::Allocation, None))
            );
            assert!(failure.fired(), "{reservation:?}");
            assert_eq!(failure.attempts(), 1, "{reservation:?}");
            assert_eq!(editor.document().nodes.len(), 2, "{reservation:?}");
            assert!(
                editor
                    .document()
                    .root_element()
                    .attributes()
                    .next()
                    .is_none()
            );
            assert_eq!(editor.document().edit_created_nodes, 1);
            assert_eq!(editor.document().edit_created_attributes, 0);
        }
    }

    /// A supplied allocator cannot be capped without unsafe code, so the
    /// failpoint stands in for its failure at each reservation site.
    #[cfg(feature = "allocator_api")]
    #[test]
    fn supplied_allocator_reservation_failures_publish_nothing() {
        let system = std::alloc::System;
        let item = [ElementItem::Attribute(AttributeSpec {
            name: NameSpec {
                local: "id",
                namespace: None,
                prefix: PrefixPolicy::Exact(None),
            },
            value: "1",
        })];
        let spec = ElementSpec {
            name: root_spec().name,
            items: &item,
        };
        let allocation = Err(EditError::new(EditErrorKind::Allocation, None));
        for reservation in [
            EditReservation::Node,
            EditReservation::Attribute,
            EditReservation::Pool,
        ] {
            let failure = EditReservationFailureGuard::arm(reservation, 0);
            let created = OwnedDocument::create_in(spec, EditLimits::default(), &system);
            assert_eq!(created.map(drop), allocation, "{reservation:?}");
            assert!(failure.fired(), "{reservation:?}");
            drop(failure);

            let mut document =
                OwnedDocument::create_in(spec, EditLimits::default(), &system).unwrap();
            let parent = document.root_element().id();
            let mut editor = document.edit(EditLimits::default()).unwrap();
            let failure = EditReservationFailureGuard::arm(reservation, 0);
            let appended = editor.append_element(parent, spec);
            assert_eq!(appended.map(drop), allocation, "{reservation:?}");
            assert!(failure.fired(), "{reservation:?}");
            let document = editor.document();
            assert_eq!(
                (document.nodes.len(), document.attributes.len()),
                (2, 1),
                "{reservation:?}"
            );
            assert!(std::ptr::eq(*document.allocator(), &system));
        }
    }

    #[test]
    fn generated_prefix_path_injects_every_scratch_reservation_atomically() {
        let items = [ElementItem::Attribute(AttributeSpec {
            name: NameSpec {
                local: "id",
                namespace: Some("urn:generated"),
                prefix: PrefixPolicy::Automatic,
            },
            value: "1",
        })];
        let mut failures = 0;
        let mut saw_success = false;
        for skip in 0..32 {
            let mut document = OwnedDocument::create(root_spec(), EditLimits::default()).unwrap();
            let parent = document.root_element().id();
            let mut editor = document.edit(EditLimits::default()).unwrap();
            let before = serialized(editor.document());
            let failure = EditReservationFailureGuard::arm(EditReservation::Scratch, skip);
            let result = editor.append_element(
                parent,
                ElementSpec {
                    name: NameSpec {
                        local: "item",
                        namespace: None,
                        prefix: PrefixPolicy::Exact(None),
                    },
                    items: &items,
                },
            );
            if failure.fired() {
                assert_eq!(result, Err(EditError::new(EditErrorKind::Allocation, None)));
                assert_eq!(failure.attempts(), skip + 1);
                assert_eq!(editor.document().nodes.len(), 2);
                assert!(
                    editor
                        .document()
                        .root_element()
                        .attributes()
                        .next()
                        .is_none()
                );
                assert_eq!(editor.document().edit_created_nodes, 1);
                assert_eq!(editor.document().edit_created_attributes, 0);
                drop(failure);
                assert_eq!(serialized(editor.document()), before);
                let child = editor
                    .append_element(
                        parent,
                        ElementSpec {
                            name: NameSpec {
                                local: "item",
                                namespace: None,
                                prefix: PrefixPolicy::Exact(None),
                            },
                            items: &items,
                        },
                    )
                    .unwrap();
                let child = editor.document().node(child).unwrap();
                assert_eq!(child.name().unwrap().qualified(), "item");
                assert_eq!(child.parent().unwrap().id(), parent);
                assert_eq!(child.attributes().count(), 2);
                assert_eq!(editor.document().nodes.len(), 3);
                assert_eq!(editor.document().edit_created_nodes, 2);
                assert_eq!(editor.document().edit_created_attributes, 2);
                failures += 1;
            } else {
                assert!(result.is_ok());
                assert_eq!(failure.attempts(), skip);
                drop(failure);
                saw_success = true;
                break;
            }
        }
        assert!(failures >= 7, "expected generated prefix and clone sites");
        assert!(saw_success, "scratch failure loop never reached success");
    }

    #[test]
    fn inherited_long_prefix_scratch_growth_is_injectable_before_publication() {
        let prefix = format!("p{}", "a".repeat(1_000));
        let xml = format!("<r xmlns:{prefix}='urn:inherited'/>");
        let items = [ElementItem::Attribute(AttributeSpec {
            name: NameSpec {
                local: "id",
                namespace: Some("urn:inherited"),
                prefix: PrefixPolicy::Automatic,
            },
            value: "1",
        })];
        let mut failures = 0;
        let mut saw_success = false;
        for skip in 0..32 {
            let mut document = OwnedDocument::parse(&xml).unwrap();
            let parent = document.root_element().id();
            let node_count = document.nodes.len();
            let mut editor = document.edit(EditLimits::default()).unwrap();
            let before = serialized(editor.document());
            let failure = EditReservationFailureGuard::arm(EditReservation::Scratch, skip);
            let result = editor.append_element(
                parent,
                ElementSpec {
                    name: NameSpec {
                        local: "item",
                        namespace: None,
                        prefix: PrefixPolicy::Exact(None),
                    },
                    items: &items,
                },
            );
            if failure.fired() {
                assert_eq!(result, Err(EditError::new(EditErrorKind::Allocation, None)));
                assert_eq!(failure.attempts(), skip + 1);
                assert_eq!(editor.document().nodes.len(), node_count);
                assert_eq!(editor.document().edit_created_nodes, 0);
                assert_eq!(editor.document().edit_created_attributes, 0);
                drop(failure);
                assert_eq!(serialized(editor.document()), before);
                let child = editor
                    .append_element(
                        parent,
                        ElementSpec {
                            name: NameSpec {
                                local: "item",
                                namespace: None,
                                prefix: PrefixPolicy::Exact(None),
                            },
                            items: &items,
                        },
                    )
                    .unwrap();
                let child = editor.document().node(child).unwrap();
                assert_eq!(child.name().unwrap().qualified(), "item");
                assert_eq!(child.parent().unwrap().id(), parent);
                assert_eq!(child.attributes().count(), 1);
                assert_eq!(editor.document().nodes.len(), node_count + 1);
                assert_eq!(editor.document().edit_created_nodes, 1);
                assert_eq!(editor.document().edit_created_attributes, 1);
                failures += 1;
            } else {
                assert!(result.is_ok());
                assert_eq!(failure.attempts(), skip);
                drop(failure);
                saw_success = true;
                break;
            }
        }
        assert!(
            failures >= 5,
            "expected inherited prefix clone and growth sites"
        );
        assert!(saw_success, "scratch failure loop never reached success");
    }

    #[test]
    fn create_reservation_failures_return_without_publishing_a_document() {
        let items = [ElementItem::Attribute(AttributeSpec {
            name: NameSpec {
                local: "id",
                namespace: None,
                prefix: PrefixPolicy::Exact(None),
            },
            value: "1",
        })];
        for reservation in [
            EditReservation::Node,
            EditReservation::Attribute,
            EditReservation::Pool,
        ] {
            let failure = EditReservationFailureGuard::arm(reservation, 0);
            let result = OwnedDocument::create(
                ElementSpec {
                    name: NameSpec {
                        local: "root",
                        namespace: None,
                        prefix: PrefixPolicy::Exact(None),
                    },
                    items: &items,
                },
                EditLimits::default(),
            );
            assert_eq!(
                result.unwrap_err(),
                EditError::new(EditErrorKind::Allocation, None),
                "{reservation:?}"
            );
            assert!(failure.fired(), "{reservation:?}");
            assert_eq!(failure.attempts(), 1, "{reservation:?}");
        }

        let mut failures = 0;
        let mut saw_success = false;
        for skip in 0..16 {
            let failure = EditReservationFailureGuard::arm(EditReservation::Scratch, skip);
            let result = OwnedDocument::create(
                ElementSpec {
                    name: NameSpec {
                        local: "root",
                        namespace: None,
                        prefix: PrefixPolicy::Exact(None),
                    },
                    items: &items,
                },
                EditLimits::default(),
            );
            if failure.fired() {
                assert_eq!(
                    result.unwrap_err(),
                    EditError::new(EditErrorKind::Allocation, None)
                );
                assert_eq!(failure.attempts(), skip + 1);
                failures += 1;
            } else {
                assert!(result.is_ok());
                assert_eq!(failure.attempts(), skip);
                drop(failure);
                assert!(
                    OwnedDocument::create(
                        ElementSpec {
                            name: NameSpec {
                                local: "root",
                                namespace: None,
                                prefix: PrefixPolicy::Exact(None),
                            },
                            items: &items,
                        },
                        EditLimits::default(),
                    )
                    .is_ok()
                );
                saw_success = true;
                break;
            }
        }
        assert!(failures >= 4, "expected all create scratch reservations");
        assert!(saw_success, "scratch failure loop never reached success");
    }

    #[test]
    fn saturating_user_ceilings_do_not_reject_a_small_valid_root() {
        let limits = EditLimits {
            max_value_bytes: usize::MAX,
            max_owned_bytes: usize::MAX,
            max_created_nodes: usize::MAX,
            max_name_bytes: usize::MAX,
            max_supplied_bytes: usize::MAX,
            max_attributes: usize::MAX,
            max_depth: usize::MAX,
            max_created_attributes: usize::MAX,
            max_namespace_work: usize::MAX,
        };
        assert!(OwnedDocument::create(root_spec(), limits).is_ok());
    }

    #[test]
    fn creation_checks_created_node_quota_before_created_attributes() {
        let items = [ElementItem::Attribute(AttributeSpec {
            name: NameSpec {
                local: "id",
                namespace: None,
                prefix: PrefixPolicy::Exact(None),
            },
            value: "1",
        })];
        let error = OwnedDocument::create(
            ElementSpec {
                name: NameSpec {
                    local: "root",
                    namespace: None,
                    prefix: PrefixPolicy::Exact(None),
                },
                items: &items,
            },
            EditLimits {
                max_created_nodes: 0,
                max_created_attributes: 0,
                ..EditLimits::default()
            },
        )
        .unwrap_err();
        assert_eq!(
            error.kind,
            EditErrorKind::LimitExceeded(EditLimit::CreatedNodes)
        );
        assert_eq!(error.location, None);
    }

    #[test]
    fn reserved_xml_namespace_errors_point_to_the_semantic_component() {
        for prefix in [PrefixPolicy::Exact(None), PrefixPolicy::Exact(Some("p"))] {
            let error = OwnedDocument::create(
                ElementSpec {
                    name: NameSpec {
                        local: "root",
                        namespace: Some(XML),
                        prefix,
                    },
                    items: &[],
                },
                EditLimits::default(),
            )
            .unwrap_err();
            assert_eq!(error.kind, EditErrorKind::Namespace);
            assert_eq!(
                error.location,
                Some(EditLocation::ElementName {
                    component: EditComponent::Prefix,
                    byte_offset: None,
                })
            );
        }

        for namespace in [None, Some("urn:wrong")] {
            let error = OwnedDocument::create(
                ElementSpec {
                    name: NameSpec {
                        local: "root",
                        namespace,
                        prefix: PrefixPolicy::Exact(Some("xml")),
                    },
                    items: &[],
                },
                EditLimits::default(),
            )
            .unwrap_err();
            assert_eq!(error.kind, EditErrorKind::Namespace);
            assert_eq!(
                error.location,
                Some(EditLocation::ElementName {
                    component: EditComponent::NamespaceUri,
                    byte_offset: None,
                })
            );
        }

        let root = OwnedDocument::create(root_spec(), EditLimits::default()).unwrap();
        let parent = root.root_element().id();
        let items = [ElementItem::Attribute(AttributeSpec {
            name: NameSpec {
                local: "lang",
                namespace: Some("urn:wrong"),
                prefix: PrefixPolicy::Exact(Some("xml")),
            },
            value: "en",
        })];
        let mut root = root;
        let mut editor = root.edit(EditLimits::default()).unwrap();
        let error = editor
            .append_element(
                parent,
                ElementSpec {
                    name: NameSpec {
                        local: "child",
                        namespace: None,
                        prefix: PrefixPolicy::Exact(None),
                    },
                    items: &items,
                },
            )
            .unwrap_err();
        assert_eq!(error.kind, EditErrorKind::Namespace);
        assert_eq!(
            error.location,
            Some(EditLocation::AttributeItem {
                index: 0,
                component: None,
                byte_offset: None,
            })
        );

        let items = [ElementItem::Attribute(AttributeSpec {
            name: NameSpec {
                local: "xmlns",
                namespace: None,
                prefix: PrefixPolicy::Exact(None),
            },
            value: "urn:wrong",
        })];
        let error = editor
            .append_element(
                parent,
                ElementSpec {
                    name: NameSpec {
                        local: "child",
                        namespace: None,
                        prefix: PrefixPolicy::Exact(None),
                    },
                    items: &items,
                },
            )
            .unwrap_err();
        assert_eq!(error.kind, EditErrorKind::Namespace);
        assert_eq!(
            error.location,
            Some(EditLocation::AttributeItem {
                index: 0,
                component: None,
                byte_offset: None,
            })
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
