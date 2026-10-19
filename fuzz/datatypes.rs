//! Built-in datatypes on structured bytes: a type byte (bit 7 selects a
//! document context), a second type byte and a facet byte, then up to three
//! NUL-separated texts: a value, a second value and a facet value.

use yaxml::schema_types::{Comparison, Context, Facet, Type, normalize};

const FACETS: [&str; 12] = [
    "length",
    "minLength",
    "maxLength",
    "pattern",
    "enumeration",
    "whiteSpace",
    "minInclusive",
    "maxInclusive",
    "minExclusive",
    "maxExclusive",
    "totalDigits",
    "fractionDigits",
];

struct Fixed;

impl Context for Fixed {
    fn namespace(&self, prefix: &str) -> Option<&str> {
        matches!(prefix, "" | "p").then_some("urn:p")
    }

    fn has_notation(&self, name: &str) -> bool {
        name.len().is_multiple_of(2)
    }

    fn has_unparsed_entity(&self, name: &str) -> bool {
        name.starts_with('e')
    }
}

/// Whether libxml2's ordering of this primitive is a total order.
fn ordered(ty: Type) -> bool {
    matches!(
        ty.primitive(),
        Type::Decimal | Type::Boolean | Type::HexBinary | Type::Base64Binary
    )
}

pub(super) fn exercise(input: &[u8]) {
    let [first, second, facet, rest @ ..] = input else {
        return;
    };
    let Ok(text) = std::str::from_utf8(rest) else {
        return;
    };
    let types: Vec<Type> = Type::all().collect();
    let (xt, yt) = (
        types[usize::from(*first & 0x7f) % types.len()],
        types[usize::from(*second) % types.len()],
    );
    let context = (first & 0x80 != 0).then_some(&Fixed as &dyn Context);
    let mut texts = text.splitn(3, '\0');
    let mut next = || texts.next().unwrap_or("");
    let (x, y, facet_value) = (next(), next(), next());
    let normalized = normalize(x, xt.whitespace());
    let parsed = xt.parse(x, context);
    // Normalizing again changes neither the text nor the verdict.
    assert_eq!(normalize(&normalized, xt.whitespace()), normalized);
    assert_eq!(
        xt.parse(&normalized, context).is_ok(),
        parsed.is_ok(),
        "{xt:?} {x:?}"
    );
    if let Some(item) = xt.item().filter(|_| parsed.is_ok()) {
        for token in normalized.split(' ').filter(|t| !t.is_empty()) {
            assert!(item.parse(token, context).is_ok(), "{xt:?} {x:?}");
        }
    }
    let Ok(xv) = parsed else {
        return;
    };
    assert_eq!(xv.ty(), xt);
    // A value equals itself, except that lists are unordered and dates with
    // years beyond `i64::MAX / 366` are incomparable in libxml2.
    let reflexive = xv.compare(&xv);
    let date = matches!(
        xt,
        Type::DateTime
            | Type::Date
            | Type::Time
            | Type::GYear
            | Type::GYearMonth
            | Type::GMonth
            | Type::GMonthDay
            | Type::GDay
    );
    let incomparable = xt.item().is_some() || date;
    assert!(
        reflexive == Comparison::Equal || (incomparable && reflexive == Comparison::Incomparable),
        "{xt:?} {x:?}: {reflexive:?}"
    );
    assert_eq!(
        xv.equals(&xv),
        reflexive == Comparison::Equal || xt.item().is_some()
    );
    if let Ok(yv) = yt.parse(y, context) {
        let (forward, backward) = (xv.compare(&yv), yv.compare(&xv));
        if ordered(xt) && ordered(yt) && xt.primitive() == yt.primitive() {
            let reversed = match backward {
                Comparison::Less => Comparison::Greater,
                Comparison::Greater => Comparison::Less,
                other => other,
            };
            assert_eq!(forward, reversed, "{xt:?} {x:?} vs {yt:?} {y:?}");
        }
        assert_eq!(
            xv.equals(&yv),
            yv.equals(&xv),
            "{xt:?} {x:?} vs {yt:?} {y:?}"
        );
    }
    let name = FACETS[usize::from(*facet) % FACETS.len()];
    match Facet::new(name, facet_value, xt, context) {
        Ok(Facet::Enumeration(allowed)) => {
            let same = Facet::Enumeration(xv.clone());
            assert_eq!(same.check(&xv, &normalized), xv.equals(&xv));
            let _ = Facet::Enumeration(allowed).check(&xv, &normalized);
        }
        Ok(facet @ Facet::WhiteSpace(_)) => assert!(facet.check(&xv, &normalized)),
        Ok(facet) => {
            let _ = facet.check(&xv, &normalized);
        }
        Err(error) => assert!(!error.to_string().is_empty()),
    }
}

#[cfg(test)]
mod tests {
    use super::exercise;

    #[test]
    fn seeds_keep_every_invariant() {
        for input in [
            &b"\x12\x13\x06 1.50 \x002\x001"[..],
            b"\x23\x23\x03\x002001-10-26T21:32:52Z\x002001-10-26T23:32:52+02:00\x00",
            b"\x8e\x8e\x04p:a\x00a\x00p:a",
            b"\x22\x22\x0bP1Y2M\x00P400D\x00",
            b"\x05\x05\x00 a  b \x00a\x002",
            b"\x2b\x2c\x01Zm9v\x000F\x003",
            b"\x8c\x8d\x00e1\x00e1 e2\x00",
        ] {
            exercise(input);
        }
    }
}
