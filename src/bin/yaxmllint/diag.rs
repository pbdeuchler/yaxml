//! libxml2-style diagnostics: `file:line: kind : message`, the source line and
//! a caret, with the line and column derived from a byte offset.

pub fn report(file: &str, input: &[u8], offset: Option<usize>, kind: &str, message: &str) {
    report_with(true, file, input, offset, kind, message);
}

/// As [`report`]; without `context` the source line and caret are left out, as
/// in libxml2's reader diagnostics.
pub fn report_with(
    context: bool,
    file: &str,
    input: &[u8],
    offset: Option<usize>,
    kind: &str,
    message: &str,
) {
    let Some(offset) = offset.map(|offset| offset.min(input.len())) else {
        return eprintln!("{file}: {kind} : {message}");
    };
    let line = 1 + input[..offset].iter().filter(|&&b| b == b'\n').count();
    if !context {
        return eprintln!("{file}:{line}: {kind} : {message}");
    }
    let start = input[..offset]
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |i| i + 1);
    let end = input[offset..]
        .iter()
        .position(|&b| b == b'\n')
        .map_or(input.len(), |i| offset + i);
    let column = String::from_utf8_lossy(&input[start..offset])
        .chars()
        .count();
    let context = String::from_utf8_lossy(&input[start..end]);
    eprintln!(
        "{file}:{line}: {kind} : {message}\n{context}\n{}^",
        " ".repeat(column)
    );
}
