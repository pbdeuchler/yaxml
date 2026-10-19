//! The two datatype libraries libxml2 registers: RELAX NG's built-in
//! `string` and `token`, and XML Schema datatypes with their facets.

use crate::schema_types::{Context, Facet, Type, Value, WhiteSpace, normalize};

const XSD: &str = "http://www.w3.org/2001/XMLSchema-datatypes";
/// libxml2 also files the built-in library under the RELAX NG namespace.
const BUILTIN: &str = super::RNG_NS;

#[derive(Debug)]
pub(super) enum Datatype {
    String,
    Token,
    Xsd(Type, Vec<Facet>),
}

/// Why a `data` or `value` element names no usable datatype.
#[derive(Debug, Eq, PartialEq)]
pub(super) enum Problem {
    UnknownLibrary,
    UnknownType,
    ParamsForbidden,
    BadParam(String),
    BadValue,
}

impl Datatype {
    pub(super) fn new(
        library: &str,
        name: &str,
        params: &[(&str, &str)],
        cx: &dyn Context,
    ) -> Result<Self, Problem> {
        match library {
            "" | BUILTIN => {
                let datatype = match name {
                    "string" => Datatype::String,
                    "token" => Datatype::Token,
                    _ => return Err(Problem::UnknownType),
                };
                match params.is_empty() {
                    true => Ok(datatype),
                    false => Err(Problem::ParamsForbidden),
                }
            }
            XSD => {
                let ty = Type::from_name(name).ok_or(Problem::UnknownType)?;
                let facets = params
                    .iter()
                    .map(|&(param, value)| match param {
                        "enumeration" | "whiteSpace" => Err(Problem::BadParam(param.into())),
                        _ => Facet::new(param, value, ty, Some(cx))
                            .map_err(|_| Problem::BadParam(param.into())),
                    })
                    .collect::<Result<_, _>>()?;
                Ok(Datatype::Xsd(ty, facets))
            }
            _ => Err(Problem::UnknownLibrary),
        }
    }

    pub(super) fn name(&self) -> &'static str {
        match self {
            Datatype::String => "string",
            Datatype::Token => "token",
            Datatype::Xsd(ty, _) => ty.name(),
        }
    }

    fn parse(&self, text: &str, cx: &dyn Context) -> Option<Value> {
        let Datatype::Xsd(ty, facets) = self else {
            return None;
        };
        let value = ty.parse(text, Some(cx)).ok()?;
        let normalized = normalize(text, ty.whitespace());
        facets
            .iter()
            .all(|facet| facet.check(&value, &normalized))
            .then_some(value)
    }

    pub(super) fn allows(&self, text: &str, cx: &dyn Context) -> bool {
        matches!(self, Datatype::String | Datatype::Token) || self.parse(text, cx).is_some()
    }
}

/// A `value` pattern: its datatype, lexical form and, for XML Schema
/// types, the value computed in the schema's namespace context.
#[derive(Debug)]
pub(super) struct ValueDef {
    pub(super) datatype: Datatype,
    lexical: Box<str>,
    value: Option<Value>,
}

impl ValueDef {
    pub(super) fn new(
        datatype: Datatype,
        lexical: &str,
        cx: &dyn Context,
    ) -> Result<Self, Problem> {
        let value = match datatype {
            Datatype::Xsd(..) => Some(datatype.parse(lexical, cx).ok_or(Problem::BadValue)?),
            _ => None,
        };
        Ok(Self {
            datatype,
            lexical: lexical.into(),
            value,
        })
    }

    pub(super) fn matches(&self, text: &str, cx: &dyn Context) -> bool {
        match (&self.datatype, &self.value) {
            (Datatype::String, _) => *text == *self.lexical,
            (Datatype::Token, _) => {
                normalize(text, WhiteSpace::Collapse)
                    == normalize(&self.lexical, WhiteSpace::Collapse)
            }
            (_, Some(expected)) => self
                .datatype
                .parse(text, cx)
                .is_some_and(|value| value.equals(expected)),
            (_, None) => false,
        }
    }
}
