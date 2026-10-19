//! `xmlSchemaVCheckCVCSimpleType`: values against simple type definitions.

use std::cell::OnceCell;
use std::collections::HashSet;

use crate::schema_types::{self, Facet, Type, WhiteSpace, normalize};

use super::Code;
use super::components::{ANY_TYPE, Names, QName, TypeDef, TypeId, Typed, Variety};

/// The namespaces, notations and unparsed entities a value can refer to.
pub(crate) struct Scope<'a> {
    namespaces: OnceCell<Vec<(&'a str, &'a str)>>,
    load: &'a dyn Fn() -> Vec<(&'a str, &'a str)>,
    names: &'a Names,
    notations: &'a HashSet<QName>,
    entity: &'a dyn Fn(&str) -> bool,
    /// Instance values report an unbound QName prefix separately, as
    /// libxml2's `xmlSchemaValidateQName` does.
    pub(crate) instance: bool,
}

impl<'a> Scope<'a> {
    pub(crate) fn new(
        load: &'a dyn Fn() -> Vec<(&'a str, &'a str)>,
        names: &'a Names,
        notations: &'a HashSet<QName>,
        entity: &'a dyn Fn(&str) -> bool,
    ) -> Self {
        Self {
            namespaces: OnceCell::new(),
            load,
            names,
            notations,
            entity,
            instance: false,
        }
    }
}

impl schema_types::Context for Scope<'_> {
    fn namespace(&self, prefix: &str) -> Option<&str> {
        let namespaces = self.namespaces.get_or_init(self.load);
        namespaces
            .iter()
            .find(|&&(p, _)| p == prefix)
            .map(|&(_, uri)| uri)
            .filter(|uri| !uri.is_empty())
    }

    /// `xmlSchemaValidateNotation`: a declared notation; an unprefixed name
    /// has no namespace.
    fn has_notation(&self, name: &str) -> bool {
        let (ns, local) = match name.split_once(':') {
            Some((prefix, local)) => match self.namespace(prefix) {
                Some(uri) => (self.names.get(uri), local),
                None => return false,
            },
            None => (0, name),
        };
        let local = self.names.get(local);
        self.notations.contains(&QName { ns, local })
    }

    fn has_unparsed_entity(&self, name: &str) -> bool {
        (self.entity)(name)
    }
}

/// Why a value is invalid: libxml2 codes and messages, in report order.
pub(crate) type Failure = Vec<(Code, String)>;

fn describe(types: &[TypeDef], names: &Names, ty: TypeId) -> String {
    let variety = match types[ty].simple().map(|s| &s.variety) {
        Some(Variety::List(_)) => "list",
        Some(Variety::Union(_)) => "union",
        _ if is_builtin_list(ty) => "list",
        _ => "atomic",
    };
    match types[ty].name {
        Some(name) if ty <= Type::all().count() => {
            format!("the {variety} type 'xs:{}'", names.str(name.local))
        }
        Some(name) => format!("the {variety} type '{}'", names.format(name)),
        None => format!("the local {variety} type"),
    }
}

fn is_builtin_list(ty: TypeId) -> bool {
    Type::all()
        .nth(ty.wrapping_sub(1))
        .is_some_and(|ty| ty.item().is_some())
}

/// The whitespace handling of a type (`xmlSchemaGetWhiteSpaceFacetValue`).
pub(crate) fn whitespace(types: &[TypeDef], ty: TypeId) -> WhiteSpace {
    match types[ty].simple() {
        Some(simple) => simple.whitespace,
        None => WhiteSpace::Preserve,
    }
}

/// Validates `lexical` against simple type `ty`.
pub(crate) fn validate(
    types: &[TypeDef],
    names: &Names,
    ty: TypeId,
    lexical: &str,
    scope: &Scope<'_>,
) -> Result<Typed, Failure> {
    let Some(simple) = types[ty].simple() else {
        return Ok(Typed::Atomic(
            Type::AnySimpleType.parse(lexical, None).expect("any"),
        ));
    };
    let invalid = |code: Code, value: &str| {
        vec![(
            code,
            format!(
                "'{value}' is not a valid value of {}.",
                describe(types, names, ty)
            ),
        )]
    };
    match &simple.variety {
        Variety::Atomic => {
            let text = normalize(lexical, simple.whitespace);
            let code = match is_builtin_list(ty) {
                true => Code::CvcDatatypeValid1_2_2,
                false => Code::CvcDatatypeValid1_2_1,
            };
            let value = simple.builtin.parse(&text, Some(scope)).map_err(|_| {
                let unbound = scope.instance
                    && simple.builtin == Type::QName
                    && text.split_once(':').is_some_and(|(prefix, _)| {
                        schema_types::Context::namespace(scope, prefix).is_none() && prefix != "xml"
                    });
                let mut failure = match unbound {
                    true => vec![(
                        Code::CvcDatatypeValid1_2_1,
                        format!("The QName value '{text}' has no corresponding namespace declaration in scope."),
                    )],
                    false => Vec::new(),
                };
                failure.extend(invalid(code, &text));
                failure
            })?;
            let typed = Typed::Atomic(value);
            check_facets(types, names, ty, &typed, &text)?;
            Ok(typed)
        }
        Variety::List(item) => {
            let text = normalize(lexical, WhiteSpace::Collapse);
            let mut items = Vec::new();
            for token in text.split(' ').filter(|token| !token.is_empty()) {
                match validate(types, names, *item, token, scope) {
                    Ok(Typed::Atomic(value)) => items.push(value),
                    Ok(Typed::List(values)) => items.extend(values),
                    Err(mut failure) => {
                        failure.truncate(1);
                        failure.extend(invalid(Code::CvcDatatypeValid1_2_2, &text));
                        return Err(failure);
                    }
                }
            }
            let typed = Typed::List(items);
            check_facets(types, names, ty, &typed, &text).map_err(|mut failure| {
                failure.extend(invalid(Code::CvcDatatypeValid1_2_2, &text));
                failure
            })?;
            Ok(typed)
        }
        Variety::Union(members) => {
            let (member, typed) = members
                .iter()
                .find_map(|&member| {
                    validate(types, names, member, lexical, scope)
                        .ok()
                        .map(|typed| (member, typed))
                })
                .ok_or_else(|| invalid(Code::CvcDatatypeValid1_2_3, lexical))?;
            let text = normalize(lexical, whitespace(types, member));
            check_facets(types, names, ty, &typed, &text).map_err(|mut failure| {
                failure.extend(invalid(Code::CvcDatatypeValid1_2_3, &text));
                failure
            })?;
            Ok(typed)
        }
    }
}

/// The user-derived ancestors of `ty`, itself first.
fn ancestors(types: &[TypeDef], ty: TypeId) -> impl Iterator<Item = TypeId> + '_ {
    let builtins = Type::all().count();
    std::iter::successors(Some(ty), move |&t| Some(types[t].base))
        .take_while(move |&t| t > builtins && t != ANY_TYPE)
}

fn facet_code(facet: &Facet) -> (Code, &'static str) {
    match facet {
        Facet::Length(_) => (Code::CvcLengthValid, "length"),
        Facet::MinLength(_) => (Code::CvcMinLengthValid, "minLength"),
        Facet::MaxLength(_) => (Code::CvcMaxLengthValid, "maxLength"),
        Facet::MinInclusive(_) => (Code::CvcMinInclusiveValid, "minInclusive"),
        Facet::MaxInclusive(_) => (Code::CvcMaxInclusiveValid, "maxInclusive"),
        Facet::MinExclusive(_) => (Code::CvcMinExclusiveValid, "minExclusive"),
        Facet::MaxExclusive(_) => (Code::CvcMaxExclusiveValid, "maxExclusive"),
        Facet::TotalDigits(_) => (Code::CvcTotalDigitsValid, "totalDigits"),
        Facet::FractionDigits(_) => (Code::CvcFractionDigitsValid, "fractionDigits"),
        Facet::Pattern(_) => (Code::CvcPatternValid, "pattern"),
        Facet::Enumeration(_) => (Code::CvcEnumerationValid, "enumeration"),
        Facet::WhiteSpace(_) => (Code::CvcDatatypeValid1_2_1, "whiteSpace"),
    }
}

/// `xmlSchemaValidateFacets`: bounds of `ty` itself, the first enumeration
/// up the derivation chain, and every derivation step's patterns.
fn check_facets(
    types: &[TypeDef],
    names: &Names,
    ty: TypeId,
    typed: &Typed,
    text: &str,
) -> Result<(), Failure> {
    let simple = types[ty].simple().expect("simple");
    let mut failure = Vec::new();
    for facet in &simple.facets {
        let ok = match (facet, typed) {
            (Facet::Length(n), Typed::List(items)) => items.len() as u64 == *n,
            (Facet::MinLength(n), Typed::List(items)) => items.len() as u64 >= *n,
            (Facet::MaxLength(n), Typed::List(items)) => items.len() as u64 <= *n,
            (_, Typed::List(_)) => true,
            (facet, Typed::Atomic(value)) => facet.check(value, text),
        };
        if !ok {
            let (code, name) = facet_code(facet);
            failure.push((
                code,
                format!("[facet '{name}'] The value '{text}' is not accepted by the facet."),
            ));
        }
    }
    let enumeration = ancestors(types, ty).find_map(|t| types[t].simple()?.enumeration.as_ref());
    if let Some(values) = enumeration
        && !values.iter().any(|value| value.equals(typed))
    {
        failure.push((
            Code::CvcEnumerationValid,
            format!(
                "[facet 'enumeration'] The value '{text}' is not an element of the set {{{}}}.",
                names.str(0)
            ),
        ));
    }
    for t in ancestors(types, ty) {
        let patterns = &types[t].simple().expect("simple").patterns;
        if !patterns.is_empty() && !patterns.iter().any(|regex| regex.is_match(text)) {
            failure.push((
                Code::CvcPatternValid,
                format!("[facet 'pattern'] The value '{text}' is not accepted by the pattern."),
            ));
            break;
        }
    }
    match failure.is_empty() {
        true => Ok(()),
        false => Err(failure),
    }
}
