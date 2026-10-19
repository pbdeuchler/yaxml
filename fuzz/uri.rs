//! URI parsing, serialization, escaping and resolution on arbitrary text.
//! Input is up to three newline-separated lines: reference, base, escape list.

use yaxml::Uri;
use yaxml::uri::{canonic_path, escape, normalize_path, relative, resolve, unescape};

pub(super) fn exercise(input: &[u8]) {
    let Ok(text) = std::str::from_utf8(input) else {
        return;
    };
    let mut lines = text.splitn(3, '\n');
    let mut line = || lines.next().unwrap_or("");
    let (reference, base, list) = (line(), line(), line());
    // Raw components are ASCII and serialize to something that parses again.
    if let Ok(uri) = Uri::parse_raw(reference) {
        let saved = uri.to_string();
        assert!(saved.is_ascii(), "{reference:?} saved as {saved:?}");
        assert!(
            Uri::parse_raw(&saved).is_ok(),
            "{reference:?} saved as {saved:?}"
        );
    }
    let _ = Uri::parse(reference).map(|uri| uri.to_string());
    let _ = (resolve(reference, base), relative(reference, base));
    let once = normalize_path(reference);
    assert_eq!(normalize_path(&once), once);
    // Escaping outside a list without '%' is exactly reversible, up to a NUL.
    let escaped = escape(reference, &list.replace('%', ""));
    assert!(escaped.is_ascii());
    assert!(canonic_path(reference).is_ascii() || !reference.contains("://"));
    let kept = reference.split('\0').next().unwrap_or("");
    assert_eq!(unescape(&escaped).as_deref(), Ok(kept));
}

#[cfg(test)]
mod tests {
    use super::exercise;

    #[test]
    fn token_soup_keeps_every_invariant() {
        const TOKENS: &[&str] = &[
            "http://", "x:", "//", "/", "..", ".", "a", "%41", "%2F", "%zz", "%", "%00", "%FF",
            "?", "#", "@", "[", "]", "::1", ":80", ":", "\u{e9}", "\n", " ", "~", ";", "d:", "\\",
            "h",
        ];
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        for _ in 0..20_000 {
            let mut text = String::new();
            for _ in 0..1 + state % 9 {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                text.push_str(TOKENS[(state >> 33) as usize % TOKENS.len()]);
            }
            exercise(text.as_bytes());
        }
    }
}
